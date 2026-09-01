#!/usr/bin/env python3
"""Tests for sync_watched_from_youtube.py.

Run: python3 scripts/test_sync_watched_from_youtube.py

The payload parsing is the fragile half — YouTube renames renderers without
notice — so it is exercised against testdata/history_page.json, a real
InnerTube continuation response with the ids and titles scrubbed.
"""

import contextlib
import io
import json
import sqlite3
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

import sync_watched_from_youtube as sync  # noqa: E402

FIXTURE = json.loads((Path(__file__).parent / "testdata/history_page.json").read_text())

SCHEMA = """
CREATE TABLE videos (
  id TEXT PRIMARY KEY, channel_id TEXT NOT NULL, title TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'ready',
  watched INTEGER NOT NULL DEFAULT 0, watched_at INTEGER,
  first_seen_at INTEGER NOT NULL DEFAULT 0);
"""


def tile(vid, title="T", percent=None, content_type="LOCKUP_CONTENT_TYPE_VIDEO"):
    """A minimal lockup shaped like the real one."""
    lockup = {
        "contentId": vid,
        "contentType": content_type,
        "metadata": {"lockupMetadataViewModel": {"title": {"content": title}}},
        "contentImage": {"thumbnailViewModel": {"overlays": []}},
    }
    if percent is not None:
        lockup["contentImage"]["thumbnailViewModel"]["overlays"].append(
            {"thumbnailOverlayBadgeViewModel": {},
             "thumbnailOverlayProgressBarViewModel": {"startPercent": percent}})
    return {"lockupViewModel": lockup}


class ParsesRealPayload(unittest.TestCase):
    """Guards against YouTube renaming the renderers out from under us."""

    def test_extracts_every_video_from_a_real_page(self):
        got = sync.extract_videos(FIXTURE)
        self.assertEqual(len(got), 6)

    def test_reads_the_real_progress_bar_values(self):
        percents = sorted((p for _, p in sync.extract_videos(FIXTURE).values()),
                          key=lambda p: (p is None, p))
        self.assertEqual(percents, [17, 100, 100, 100, 100, None])

    def test_reads_titles_and_ids(self):
        got = sync.extract_videos(FIXTURE)
        self.assertIn("vid00000001", got)
        self.assertEqual(got["vid00000001"][0], "Example Video 1")

    def test_finds_the_continuation_token(self):
        self.assertEqual(sync.next_token(FIXTURE), "NEXT_TOKEN_ABC")


class Parsing(unittest.TestCase):
    def test_missing_progress_bar_is_none_not_zero(self):
        # A video with no bar is unstarted, which must never be confused with
        # 0% — both are "not watched", but only None means "no data".
        got = sync.extract_videos({"c": [tile("a")]})
        self.assertEqual(got["a"], ("T", None))

    def test_ignores_non_video_lockups(self):
        payload = {"c": [tile("a", percent=100),
                         tile("p", content_type="LOCKUP_CONTENT_TYPE_PLAYLIST")]}
        self.assertEqual(list(sync.extract_videos(payload)), ["a"])

    def test_tolerates_a_missing_title(self):
        broken = {"lockupViewModel": {"contentId": "a",
                                      "contentType": "LOCKUP_CONTENT_TYPE_VIDEO"}}
        self.assertEqual(sync.extract_videos({"c": [broken]})["a"], ("(untitled)", None))

    def test_no_continuation_at_the_end_of_the_feed(self):
        self.assertIsNone(sync.next_token({"contents": [tile("a")]}))

    def test_empty_payload_yields_nothing(self):
        self.assertEqual(sync.extract_videos({}), {})


class Threshold(unittest.TestCase):
    def test_ninety_is_watched_and_eightynine_is_not(self):
        self.assertFalse(sync.is_watched(89))
        self.assertTrue(sync.is_watched(90))

    def test_hundred_is_watched(self):
        self.assertTrue(sync.is_watched(100))

    def test_no_bar_is_never_watched(self):
        self.assertFalse(sync.is_watched(None))

    def test_the_ten_percent_floor_is_not_watched(self):
        # YouTube floors a drawn bar at 10, so 10 means "anything up to 10%".
        self.assertFalse(sync.is_watched(10))


def select(found, library):
    """report() prints a stats block; the tests only care about its return."""
    with contextlib.redirect_stdout(io.StringIO()):
        return sync.report(found, library)


class Selection(unittest.TestCase):
    """report() decides what gets written, so its filtering is load-bearing."""

    def setUp(self):
        self.found = {
            "keep":    ("Finished",      100),
            "keep2":   ("Nearly done",    92),
            "partial": ("Half watched",   50),
            "already": ("Seen before",   100),
            "nobar":   ("Never started", None),
            "foreign": ("Not in library", 100),
        }
        self.library = {
            "keep": ("Finished", False), "keep2": ("Nearly done", False),
            "partial": ("Half watched", False), "already": ("Seen before", True),
            "nobar": ("Never started", False), "unrelated": ("Never watched", False),
        }

    def test_selects_only_unmarked_library_videos_at_or_above_threshold(self):
        self.assertEqual(select(self.found, self.library), ["keep", "keep2"])

    def test_excludes_videos_already_marked_watched(self):
        self.assertNotIn("already", select(self.found, self.library))

    def test_excludes_history_videos_absent_from_the_library(self):
        self.assertNotIn("foreign", select(self.found, self.library))

    def test_returns_nothing_when_everything_is_already_marked(self):
        library = {"already": ("Seen before", True)}
        self.assertEqual(select({"already": ("Seen before", 100)}, library), [])


class Database(unittest.TestCase):
    def setUp(self):
        self.db = Path(tempfile.mkdtemp()) / "mytube.db"
        con = sqlite3.connect(self.db)
        con.executescript(SCHEMA)
        con.executemany(
            "INSERT INTO videos (id, channel_id, title, watched) VALUES (?,?,?,?)",
            [("a", "UC1", "A", 0), ("b", "UC1", "B", 0), ("c", "UC1", "C", 1)])
        con.commit()
        con.close()

    def rows(self):
        con = sqlite3.connect(self.db)
        try:
            return dict(con.execute("SELECT id, watched FROM videos").fetchall())
        finally:
            con.close()

    def test_marks_the_requested_videos(self):
        sync.apply(self.db, ["a", "b"])
        self.assertEqual(self.rows(), {"a": 1, "b": 1, "c": 1})

    def test_leaves_untouched_videos_alone(self):
        sync.apply(self.db, ["a"])
        self.assertEqual(self.rows()["b"], 0)

    def test_sets_a_watched_timestamp(self):
        sync.apply(self.db, ["a"])
        con = sqlite3.connect(self.db)
        stamp = con.execute("SELECT watched_at FROM videos WHERE id='a'").fetchone()[0]
        con.close()
        self.assertIsNotNone(stamp)

    def test_does_not_rewrite_already_watched_rows(self):
        # The WHERE watched=0 guard keeps an existing watched_at intact.
        self.assertEqual(sync.apply(self.db, ["c"]), 0)

    def test_is_idempotent(self):
        self.assertEqual(sync.apply(self.db, ["a", "b"]), 2)
        self.assertEqual(sync.apply(self.db, ["a", "b"]), 0)

    def test_reads_the_library_with_watched_flags(self):
        self.assertEqual(sync.read_library(self.db),
                         {"a": ("A", False), "b": ("B", False), "c": ("C", True)})

    def test_backup_is_a_readable_standalone_copy(self):
        dest = sync.backup(self.db)
        self.assertTrue(dest.exists())
        con = sqlite3.connect(f"file:{dest}?mode=ro", uri=True)
        try:
            self.assertEqual(con.execute("SELECT COUNT(*) FROM videos").fetchone()[0], 3)
        finally:
            con.close()

    def test_backup_predates_the_change(self):
        dest = sync.backup(self.db)
        sync.apply(self.db, ["a"])
        con = sqlite3.connect(f"file:{dest}?mode=ro", uri=True)
        try:
            self.assertEqual(
                con.execute("SELECT watched FROM videos WHERE id='a'").fetchone()[0], 0)
        finally:
            con.close()


if __name__ == "__main__":
    unittest.main(verbosity=2)
