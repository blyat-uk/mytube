#!/usr/bin/env python3
"""Tests for import_existing.py.

Run: python3 scripts/test_import_existing.py
"""

import shutil
import sqlite3
import sys
import tempfile
import unittest
from datetime import datetime, timezone
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

import import_existing as imp  # noqa: E402


SCHEMA = """
CREATE TABLE channels (
  id TEXT PRIMARY KEY, title TEXT NOT NULL, handle TEXT, url TEXT NOT NULL,
  thumb_path TEXT, subscribed INTEGER NOT NULL DEFAULT 1,
  added_at INTEGER NOT NULL, last_polled_at INTEGER);
CREATE TABLE videos (
  id TEXT PRIMARY KEY, channel_id TEXT NOT NULL REFERENCES channels(id),
  title TEXT NOT NULL, description TEXT, thumb_url TEXT, thumb_path TEXT,
  published_at INTEGER, sort_at INTEGER, feed_rank INTEGER NOT NULL DEFAULT 0,
  added_manually INTEGER NOT NULL DEFAULT 0, duration_secs INTEGER,
  view_count INTEGER, status TEXT NOT NULL DEFAULT 'pending',
  watched INTEGER NOT NULL DEFAULT 0, watched_at INTEGER,
  download_state TEXT NOT NULL DEFAULT 'none', download_error TEXT,
  file_path TEXT, first_seen_at INTEGER NOT NULL);
"""


def epoch(date: str) -> int:
    """An exact date, as RSS supplies it: midday, so it carries a time of day.

    Midnight exactly is how a backfilled bucket looks, so a test that wants a
    real date must not sit on it -- see bucket() below.
    """
    return int(datetime.strptime(date, "%Y-%m-%d")
               .replace(tzinfo=timezone.utc).timestamp()) + 43200


def bucket(date: str) -> int:
    """A backfilled `approximate_date` bucket: midnight UTC exactly."""
    return int(datetime.strptime(date, "%Y-%m-%d")
               .replace(tzinfo=timezone.utc).timestamp())


def build_library(channels, videos) -> imp.Library:
    conn = sqlite3.connect(":memory:")
    conn.row_factory = sqlite3.Row
    conn.executescript(SCHEMA)
    for cid, title in channels:
        conn.execute(
            "INSERT INTO channels (id,title,url,added_at) VALUES (?,?,'u',0)",
            (cid, title))
    for v in videos:
        conn.execute(
            """INSERT INTO videos (id,channel_id,title,published_at,first_seen_at,
                                   download_state,file_path)
               VALUES (?,?,?,?,0,?,?)""",
            (v["id"], v["channel_id"], v["title"], v.get("published_at"),
             v.get("download_state", "none"), v.get("file_path")))
    return imp.Library.load(conn)


def scanned(name: str, sidecar=None) -> imp.Scanned:
    return imp.Scanned.read(Path("/archive") / name, sidecar)


class ParseStem(unittest.TestCase):
    def test_title_first_layout(self):
        self.assertEqual(
            imp.parse_stem("Some_Story_Part_3 [2026-08-09] (Neon_Toon)"),
            ("Some_Story_Part_3", "2026-08-09", "Neon_Toon"))

    def test_channel_first_layout(self):
        self.assertEqual(
            imp.parse_stem("(Nokoi_Recap) Everyone_Mocked_His_HEALER [2026-06-20]"),
            ("Everyone_Mocked_His_HEALER", "2026-06-20", "Nokoi_Recap"))

    def test_title_may_contain_brackets_and_parens(self):
        self.assertEqual(
            imp.parse_stem("A_[weird]_(title) [2026-01-02] (Chan)"),
            ("A_[weird]_(title)", "2026-01-02", "Chan"))

    def test_title_ending_in_a_period(self):
        # A real file: "...Unlimited_Power. [2026-08-04].mkv"
        self.assertEqual(
            imp.parse_stem("(Exquisite_Comics) Apocalypse_Unlimited_Power. [2026-08-04]"),
            ("Apocalypse_Unlimited_Power.", "2026-08-04", "Exquisite_Comics"))

    def test_channel_first_dash_layout(self):
        self.assertEqual(
            imp.parse_stem("(Voidpage) He_Was_Too_POOR_For_The_ACADEMY - 2026-07-28"),
            ("He_Was_Too_POOR_For_The_ACADEMY", "2026-07-28", "Voidpage"))

    def test_dash_layout_without_a_channel(self):
        self.assertEqual(
            imp.parse_stem("Job_Change_Failed_Drops_SSS-Rank_Artifact - 2026-07-20"),
            ("Job_Change_Failed_Drops_SSS-Rank_Artifact", "2026-07-20", None))

    def test_dash_inside_a_restricted_title_is_not_the_separator(self):
        self.assertEqual(
            imp.parse_stem("(Frieren_Manhwa) He_Trained_Alone_-_Manhwa_Recap - 2025-06-12"),
            ("He_Trained_Alone_-_Manhwa_Recap", "2025-06-12", "Frieren_Manhwa"))

    def test_unknown_layout_is_rejected(self):
        self.assertIsNone(imp.parse_stem("s01.e19041401 - Wild vegetables"))
        self.assertIsNone(imp.parse_stem("Holiday video 2026"))
        self.assertIsNone(imp.parse_stem("Title [2026-13-45] (Chan)"))

    def test_neither_layout_matches_the_other(self):
        title_first = "T [2026-01-02] (C)"
        channel_first = "(C) T [2026-01-02]"
        self.assertIsNone(imp.LAYOUT_CHANNEL_FIRST.match(title_first))
        self.assertIsNone(imp.LAYOUT_TITLE_FIRST.match(channel_first))


class Restrict(unittest.TestCase):
    def test_reproduces_a_real_archived_filename(self):
        # The on-disk file is
        # "(Haru_Senpai) Isekai_d_as_LOOSER_He_Grinds_BASIC_SKILLS_..._OVERPOWERED"
        self.assertEqual(
            imp.restrict("Isekai'd as LOOSER, He Grinds BASIC SKILLS "
                         "Until They Become OVERPOWERED"),
            "Isekai_d_as_LOOSER_He_Grinds_BASIC_SKILLS_Until_They_Become_OVERPOWERED")

    def test_channel_names_collapse_the_same_way(self):
        self.assertEqual(imp.restrict("100 MILLION DOLLARS RECAP"),
                         "100_MILLION_DOLLARS_RECAP")
        self.assertEqual(imp.restrict("Junkie's Manhwa"), "Junkie_s_Manhwa")


class UtcDate(unittest.TestCase):
    def test_none_passes_through(self):
        self.assertIsNone(imp.utc_date(None))

    def test_midday_timestamp_keeps_its_date(self):
        self.assertEqual(imp.utc_date(bucket("2026-07-31") + 43200), "2026-07-31")


class Bucketed(unittest.TestCase):
    def test_midnight_exactly_is_a_bucket(self):
        self.assertTrue(imp.is_bucketed(bucket("2026-07-30")))

    def test_a_timestamp_with_a_time_of_day_is_real(self):
        self.assertFalse(imp.is_bucketed(epoch("2026-07-30")))

    def test_a_missing_date_is_not_a_bucket(self):
        self.assertFalse(imp.is_bucketed(None))


class Classify(unittest.TestCase):
    def setUp(self):
        self.lib = build_library(
            channels=[("UC1", "Haru Senpai"), ("UC2", "Neon Toon")],
            videos=[
                {"id": "aaa", "channel_id": "UC1", "title": "Unique Story",
                 "published_at": epoch("2026-07-31")},
                {"id": "bbb", "channel_id": "UC1", "title": "Shared Title",
                 "published_at": epoch("2026-07-19")},
                {"id": "ccc", "channel_id": "UC1", "title": "Shared Title",
                 "published_at": epoch("2026-07-31")},
                {"id": "ddd", "channel_id": "UC2", "title": "No Date Video",
                 "published_at": None},
            ])

    def test_sidecar_id_is_certain(self):
        r = imp.classify(scanned("whatever.mkv", sidecar={"id": "aaa"}), self.lib, None)
        self.assertEqual(r.group, imp.CERTAIN)
        self.assertEqual(r.video["id"], "aaa")

    def test_sidecar_id_not_in_library_is_unidentified(self):
        r = imp.classify(scanned("x.mkv", sidecar={"id": "zzz"}), self.lib, None)
        self.assertEqual(r.group, imp.UNIDENTIFIED)
        self.assertIn("not loaded", r.reason)

    def test_sidecar_uploader_wins_over_channel_title(self):
        r = imp.classify(
            scanned("x.mkv", sidecar={"id": "aaa", "uploader": "Haru Senpai Official"}),
            self.lib, None)
        self.assertEqual(r.uploader, "Haru Senpai Official")

    def test_unique_title_with_exact_date_is_certain(self):
        r = imp.classify(scanned("Unique_Story [2026-07-31] (Haru_Senpai).mkv"),
                         self.lib, None)
        self.assertEqual(r.group, imp.CERTAIN)
        self.assertEqual(r.video["id"], "aaa")

    def test_unique_title_with_disagreeing_date_is_a_warning(self):
        r = imp.classify(scanned("Unique_Story [2026-07-28] (Haru_Senpai).mkv"),
                         self.lib, None)
        self.assertEqual(r.group, imp.WARNING)
        self.assertEqual(r.video["id"], "aaa")
        self.assertIn("2026-07-28", r.reason)
        self.assertIn("2026-07-31", r.reason)

    def test_unique_title_with_no_database_date_is_a_warning(self):
        r = imp.classify(scanned("No_Date_Video [2026-01-01] (Neon_Toon).mkv"),
                         self.lib, None)
        self.assertEqual(r.group, imp.WARNING)
        self.assertIn("no date", r.reason)

    def test_shared_title_is_split_by_exact_date(self):
        r = imp.classify(scanned("Shared_Title [2026-07-19] (Haru_Senpai).mkv"),
                         self.lib, None)
        self.assertEqual(r.group, imp.CERTAIN)
        self.assertEqual(r.video["id"], "bbb")

        r = imp.classify(scanned("Shared_Title [2026-07-31] (Haru_Senpai).mkv"),
                         self.lib, None)
        self.assertEqual(r.group, imp.CERTAIN)
        self.assertEqual(r.video["id"], "ccc")

    def test_shared_title_with_no_date_hit_is_unidentified_offline(self):
        r = imp.classify(scanned("Shared_Title [2026-07-25] (Haru_Senpai).mkv"),
                         self.lib, None)
        self.assertEqual(r.group, imp.UNIDENTIFIED)
        self.assertIn("ambiguous: 2 candidates", r.reason)

    def test_shared_title_is_split_by_true_upload_date(self):
        class FakeNetwork:
            def upload_dates(self, ids):
                return {"bbb": "2026-07-25", "ccc": "2026-08-02"}

        r = imp.classify(scanned("Shared_Title [2026-07-25] (Haru_Senpai).mkv"),
                         self.lib, FakeNetwork())
        self.assertEqual(r.group, imp.CERTAIN)
        self.assertEqual(r.video["id"], "bbb")
        self.assertIn("true upload date", r.reason)

    def test_a_bucketed_db_date_cannot_contradict_the_filename(self):
        # 364 real videos shared the 2026-07-30 bucket; such a date says
        # "roughly a month ago", not "uploaded that day", so a disagreement
        # with the filename carries no information.
        lib = build_library(
            channels=[("UC1", "Haru Senpai")],
            videos=[{"id": "aaa", "channel_id": "UC1", "title": "Unique Story",
                     "published_at": bucket("2026-07-30")}])
        r = imp.classify(scanned("Unique_Story [2026-07-18] (Haru_Senpai).mkv"),
                         lib, None)
        self.assertEqual(r.group, imp.CERTAIN)
        self.assertEqual(r.video["id"], "aaa")
        self.assertIn("rounded bucket", r.reason)

    def test_the_filename_date_is_what_gets_written(self):
        lib = build_library(
            channels=[("UC1", "Haru Senpai")],
            videos=[{"id": "aaa", "channel_id": "UC1", "title": "Unique Story",
                     "published_at": bucket("2026-07-30")}])
        r = imp.classify(scanned("Unique_Story [2026-07-18] (Haru_Senpai).mkv"),
                         lib, None)
        self.assertEqual(r.upload_date, "2026-07-18")

    def test_an_exact_db_date_that_disagrees_stays_a_warning(self):
        r = imp.classify(scanned("Unique_Story [2026-07-28] (Haru_Senpai).mkv"),
                         self.lib, None)
        self.assertEqual(r.group, imp.WARNING)
        self.assertIn("both exact", r.reason)

    def test_a_bucketed_date_must_not_break_a_tie(self):
        # Two videos share a title; one carries a bucket that happens to equal
        # the filename's date. Resolving on that would be a coincidence.
        lib = build_library(
            channels=[("UC1", "Haru Senpai")],
            videos=[
                {"id": "bbb", "channel_id": "UC1", "title": "Shared Title",
                 "published_at": bucket("2026-07-30")},
                {"id": "ccc", "channel_id": "UC1", "title": "Shared Title",
                 "published_at": bucket("2026-06-30")},
            ])
        r = imp.classify(scanned("Shared_Title [2026-07-30] (Haru_Senpai).mkv"),
                         lib, None)
        self.assertEqual(r.group, imp.UNIDENTIFIED)
        self.assertIn("ambiguous", r.reason)

    def test_unknown_channel_is_unidentified(self):
        r = imp.classify(scanned("Unique_Story [2026-07-31] (Nobody).mkv"),
                         self.lib, None)
        self.assertEqual(r.group, imp.UNIDENTIFIED)
        self.assertIn("not in mytube", r.reason)

    def test_unknown_title_is_unidentified(self):
        r = imp.classify(scanned("Never_Downloaded [2026-07-31] (Haru_Senpai).mkv"),
                         self.lib, None)
        self.assertEqual(r.group, imp.UNIDENTIFIED)
        self.assertIn("no title match", r.reason)

    def test_unparseable_filename_falls_back_to_a_library_title_search(self):
        r = imp.classify(scanned("random holiday clip.mkv"), self.lib, None)
        self.assertEqual(r.group, imp.UNIDENTIFIED)
        self.assertIn("no title match", r.reason)
        self.assertIn("no known layout", r.reason)

    def test_channelless_name_matches_library_wide_but_only_as_warning(self):
        # The database date here is exact and disagrees -- a real conflict.
        r = imp.classify(scanned("Unique_Story - 2026-07-28.mkv"), self.lib, None)
        self.assertEqual(r.group, imp.WARNING)
        self.assertEqual(r.video["id"], "aaa")
        self.assertIn("library", r.reason)

    def test_channelless_name_with_exact_date_is_certain(self):
        r = imp.classify(scanned("Unique_Story - 2026-07-31.mkv"), self.lib, None)
        self.assertEqual(r.group, imp.CERTAIN)
        self.assertEqual(r.video["id"], "aaa")

    def test_bare_title_with_no_date_is_never_certain(self):
        r = imp.classify(scanned("Unique_Story.mkv"), self.lib, None)
        self.assertEqual(r.group, imp.WARNING)
        self.assertIn("carries no date", r.reason)

    def test_bare_title_sharing_a_name_cannot_be_separated(self):
        r = imp.classify(scanned("Shared_Title.mkv"), self.lib, None)
        self.assertEqual(r.group, imp.UNIDENTIFIED)
        self.assertIn("no date to separate", r.reason)


class BlurredChannels(unittest.TestCase):
    """Two channel names that collapse to one restricted token share a pool,
    so a match inside them is never presented as certain."""

    def setUp(self):
        self.lib = build_library(
            channels=[("UC1", "Nokoi Recap"), ("UC2", "Nokoi  Recap")],
            videos=[
                {"id": "aaa", "channel_id": "UC1", "title": "Only Here",
                 "published_at": epoch("2026-07-31")},
                {"id": "bbb", "channel_id": "UC2", "title": "Other Video",
                 "published_at": epoch("2026-07-31")},
            ])

    def test_the_collision_is_detected(self):
        self.assertEqual(self.lib.blurred_channels, {"Nokoi_Recap"})

    def test_an_otherwise_certain_match_is_demoted_to_warning(self):
        r = imp.classify(scanned("Only_Here [2026-07-31] (Nokoi_Recap).mkv"),
                         self.lib, None)
        self.assertEqual(r.group, imp.WARNING)
        self.assertEqual(r.video["id"], "aaa")
        self.assertIn("two channels share the name", r.reason)

    def test_distinct_channel_names_are_not_blurred(self):
        lib = build_library(
            channels=[("UC1", "Nokoi Recap"), ("UC2", "Neon Toon")],
            videos=[{"id": "aaa", "channel_id": "UC1", "title": "Only Here",
                     "published_at": epoch("2026-07-31")}])
        self.assertEqual(lib.blurred_channels, set())
        r = imp.classify(scanned("Only_Here [2026-07-31] (Nokoi_Recap).mkv"), lib, None)
        self.assertEqual(r.group, imp.CERTAIN)


class ResolveConflicts(unittest.TestCase):
    def setUp(self):
        self.lib = build_library(
            channels=[("UC1", "Chan")],
            videos=[{"id": "aaa", "channel_id": "UC1", "title": "T",
                     "published_at": epoch("2026-07-31")}])

    def test_certain_beats_warning_for_a_contested_video(self):
        certain = imp.classify(scanned("T [2026-07-31] (Chan).mkv"), self.lib, None)
        warning = imp.classify(scanned("T [2026-07-30] (Chan).mp4"), self.lib, None)
        self.assertEqual(certain.group, imp.CERTAIN)
        self.assertEqual(warning.group, imp.WARNING)

        imp.resolve_conflicts([warning, certain])

        self.assertEqual(certain.group, imp.CERTAIN)
        self.assertEqual(warning.group, imp.UNIDENTIFIED)
        self.assertIn("already matched by", warning.reason)

    def test_a_video_mytube_already_has_is_left_alone(self):
        existing = Path(__file__)  # any path that exists
        lib = build_library(
            channels=[("UC1", "Chan")],
            videos=[{"id": "aaa", "channel_id": "UC1", "title": "T",
                     "published_at": epoch("2026-07-31"),
                     "download_state": "done", "file_path": str(existing)}])
        r = imp.classify(scanned("T [2026-07-31] (Chan).mkv"), lib, None)
        self.assertEqual(r.group, imp.CERTAIN)

        imp.resolve_conflicts([r])

        self.assertEqual(r.group, imp.UNIDENTIFIED)
        self.assertIn("already has this video", r.reason)


class UniquePath(unittest.TestCase):
    def test_free_path_is_returned_unchanged(self):
        p = Path("/out/Chan/Video [2026-01-01].mkv")
        self.assertEqual(imp.unique_path(p, lambda _: False), p)

    def test_collision_appends_n_before_the_extension(self):
        p = Path("/out/Chan/Video [2026-01-01].mkv")
        taken = {p}
        self.assertEqual(imp.unique_path(p, lambda c: c in taken),
                         Path("/out/Chan/Video [2026-01-01] (2).mkv"))

    def test_numbering_continues_past_the_first_free_slot(self):
        p = Path("/out/v.mkv")
        taken = {p, Path("/out/v (2).mkv"), Path("/out/v (3).mkv")}
        self.assertEqual(imp.unique_path(p, lambda c: c in taken),
                         Path("/out/v (4).mkv"))

    def test_dots_inside_the_stem_survive(self):
        p = Path("/out/s01.e19041401 - Wild.mkv")
        taken = {p}
        self.assertEqual(imp.unique_path(p, lambda c: c in taken),
                         Path("/out/s01.e19041401 - Wild (2).mkv"))


class Naming(unittest.TestCase):
    def setUp(self):
        self.lib = build_library(
            channels=[("UC1", "Haru Senpai")],
            videos=[{"id": "aaa", "channel_id": "UC1",
                     "title": "Isekai'd as LOOSER: part 1/2",
                     "published_at": epoch("2026-07-31")}])
        self.namer = imp.Namer(imp.DEFAULT_TEMPLATE, Path("/out"))

    def test_destination_uses_the_real_title_not_the_restricted_one(self):
        r = imp.classify(
            scanned("Isekai_d_as_LOOSER_-_part_1_2 [2026-07-31] (Haru_Senpai).mkv"),
            self.lib, None)
        self.assertEqual(r.group, imp.CERTAIN)
        # yt-dlp maps "/" to U+29F8 and keeps ":" (mytube uses
        # --no-windows-filenames), so the punctuation comes back.
        self.assertEqual(
            self.namer.dest(r),
            Path("/out/Haru Senpai/Isekai'd as LOOSER: part 1⧸2 [2026-07-31].mkv"))

    def test_source_extension_is_preserved(self):
        r = imp.classify(
            scanned("Isekai_d_as_LOOSER_-_part_1_2 [2026-07-31] (Haru_Senpai).mp4"),
            self.lib, None)
        self.assertEqual(self.namer.dest(r).suffix, ".mp4")


class Elide(unittest.TestCase):
    def test_short_text_is_untouched(self):
        self.assertEqual(imp.elide("short", 20), "short")

    def test_long_text_keeps_head_and_tail(self):
        out = imp.elide("A" * 60 + "TAIL", 20)
        self.assertEqual(len(out), 20)
        self.assertTrue(out.startswith("A"))
        self.assertTrue(out.endswith("TAIL"))
        self.assertIn("…", out)


class ApplyEndToEnd(unittest.TestCase):
    """Exercises the real filesystem + database path, on throwaway files."""

    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp())
        self.source = self.tmp / "archive"
        self.out = self.tmp / "library"
        self.source.mkdir()
        self.out.mkdir()

        self.conn = sqlite3.connect(self.tmp / "test.db")
        self.conn.row_factory = sqlite3.Row
        self.conn.executescript(SCHEMA)
        self.conn.execute(
            "INSERT INTO channels (id,title,url,added_at) VALUES ('UC1','Haru Senpai','u',0)")
        for vid, title, date in [
            ("aaa", "Isekai'd as LOOSER!", "2026-07-31"),
            ("bbb", "Second: Video/Two", "2026-08-01"),
        ]:
            self.conn.execute(
                """INSERT INTO videos (id,channel_id,title,published_at,first_seen_at)
                   VALUES (?,?,?,?,0)""", (vid, "UC1", title, epoch(date)))
        self.conn.commit()

    def tearDown(self):
        self.conn.close()
        shutil.rmtree(self.tmp, ignore_errors=True)

    def _run(self, groups=(imp.CERTAIN,)):
        lib = imp.Library.load(self.conn)
        items = imp.scan(self.source, self.out)
        results = [imp.classify(i, lib, None) for i in items]
        imp.resolve_conflicts(results)
        imp.assign_destinations(results, imp.Namer(imp.DEFAULT_TEMPLATE, self.out))
        imp.apply(results, list(groups), self.conn)
        return results

    def test_a_certain_match_is_moved_and_recorded(self):
        src = self.source / "Isekai_d_as_LOOSER [2026-07-31] (Haru_Senpai).mkv"
        src.write_bytes(b"video-bytes")

        self._run()

        dest = self.out / "Haru Senpai" / "Isekai'd as LOOSER! [2026-07-31].mkv"
        self.assertTrue(dest.is_file(), f"not moved; tree = {list(self.out.rglob('*'))}")
        self.assertEqual(dest.read_bytes(), b"video-bytes")
        self.assertFalse(src.exists(), "source file was left behind")

        row = self.conn.execute("SELECT download_state, file_path FROM videos "
                                "WHERE id='aaa'").fetchone()
        self.assertEqual(row["download_state"], "done")
        self.assertEqual(row["file_path"], str(dest))

    def test_a_warning_is_left_alone_when_only_certain_is_chosen(self):
        src = self.source / "Isekai_d_as_LOOSER [2026-07-28] (Haru_Senpai).mkv"
        src.write_bytes(b"x")

        results = self._run(groups=(imp.CERTAIN,))

        self.assertEqual(results[0].group, imp.WARNING)
        self.assertTrue(src.exists(), "a warning was moved despite choosing certain only")
        row = self.conn.execute("SELECT download_state FROM videos WHERE id='aaa'").fetchone()
        self.assertEqual(row["download_state"], "none")

    def test_choosing_certain_plus_warning_moves_both(self):
        (self.source / "Isekai_d_as_LOOSER [2026-07-28] (Haru_Senpai).mkv").write_bytes(b"x")

        self._run(groups=(imp.CERTAIN, imp.WARNING))

        self.assertTrue((self.out / "Haru Senpai" /
                         "Isekai'd as LOOSER! [2026-07-28].mkv").is_file())

    def test_an_occupied_destination_gets_a_numbered_suffix(self):
        (self.source / "Isekai_d_as_LOOSER [2026-07-31] (Haru_Senpai).mkv").write_bytes(b"new")
        squatter = self.out / "Haru Senpai" / "Isekai'd as LOOSER! [2026-07-31].mkv"
        squatter.parent.mkdir(parents=True)
        squatter.write_bytes(b"already here")

        self._run()

        self.assertEqual(squatter.read_bytes(), b"already here", "clobbered an existing file")
        moved = self.out / "Haru Senpai" / "Isekai'd as LOOSER! [2026-07-31] (2).mkv"
        self.assertEqual(moved.read_bytes(), b"new")

    def test_title_punctuation_is_restored_including_the_slash_glyph(self):
        (self.source / "Second_-_Video_Two [2026-08-01] (Haru_Senpai).mkv").write_bytes(b"x")

        self._run()

        # yt-dlp maps "/" to U+29F8 and keeps ":" under --no-windows-filenames.
        dest = self.out / "Haru Senpai" / "Second: Video\u29f8Two [2026-08-01].mkv"
        self.assertTrue(dest.is_file(), f"tree = {list(self.out.rglob('*'))}")

    def test_two_files_cannot_claim_the_same_video(self):
        a = self.source / "Isekai_d_as_LOOSER [2026-07-31] (Haru_Senpai).mkv"
        b = self.source / "Isekai_d_as_LOOSER [2026-07-31] (Haru_Senpai).mp4"
        a.write_bytes(b"a")
        b.write_bytes(b"b")

        results = self._run(groups=(imp.CERTAIN, imp.WARNING))

        groups = sorted(r.group for r in results)
        self.assertEqual(groups, [imp.CERTAIN, imp.UNIDENTIFIED])
        loser = next(r for r in results if r.group == imp.UNIDENTIFIED)
        self.assertIn("already matched by", loser.reason)
        self.assertTrue(loser.item.path.exists(), "the losing file should stay put")


if __name__ == "__main__":
    unittest.main(verbosity=2)
