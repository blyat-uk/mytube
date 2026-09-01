#!/usr/bin/env python3
"""Files a pre-mytube video archive into mytube's library.

Matches already-downloaded video files against the videos mytube has loaded
into its database, then moves and renames the matches into the layout mytube
itself produces.

Both filename directions go through yt-dlp's own sanitiser rather than a
hand-rolled one, so imported names cannot drift from downloaded ones:

  reading  old names are reproduced by running a database title *forward*
           through sanitize_filename(restricted=True), which is exactly the
           transform that wrote them. Comparison is then string equality,
           not fuzzy scoring.
  writing  destinations come from YoutubeDL.prepare_filename(), the same call
           mytube's probe uses to pick a download path.

Nothing moves until you pick a group at the prompt.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import sqlite3
import sys
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path

try:
    from yt_dlp import YoutubeDL
    from yt_dlp.utils import sanitize_filename
except ImportError:  # pragma: no cover - environment problem, not logic
    sys.exit(
        "yt-dlp's Python module is not importable.\n"
        "The `yt-dlp` binary alone is not enough; install the module, e.g.\n"
        "  pacman -S yt-dlp    (Arch/CachyOS, ships the module)\n"
        "  pip install yt-dlp"
    )

# `~/.config/mytube`, matching config.rs -- deliberately not the Tauri app dir.
CONFIG_DIR = Path.home() / ".config" / "mytube"
SETTINGS_PATH = CONFIG_DIR / "settings.json"
DB_PATH = CONFIG_DIR / "mytube.db"

DEFAULT_TEMPLATE = "%(uploader)s/%(title)s [%(upload_date>%Y-%m-%d)s].%(ext)s"
DEFAULT_DOWNLOAD_DIR = str(Path.home() / "Videos" / "mytube")

VIDEO_EXTS = {".mkv", ".mp4", ".webm", ".m4v", ".mov", ".avi"}

CERTAIN, WARNING, UNIDENTIFIED = "certain", "warning", "unidentified"


# --------------------------------------------------------------------------
# Filename parsing
# --------------------------------------------------------------------------

_DATE = r"(?P<date>\d{4}-\d{2}-\d{2})"

# Every layout ends on a strict anchor -- `(channel)`, `[date]` or ` - date` --
# so the greedy title group cannot swallow it. Restricted titles contain no
# spaces, which is what makes " - " a safe separator here.
#
# Order matters: the channel-first variants must be tried before the bare
# `title - date` one, which would otherwise swallow a leading "(Channel) " into
# its title. The last layout has no channel group, so parse_stem yields None
# for it and the match falls back to a library-wide title search.
LAYOUTS = (
    re.compile(rf"^(?P<title>.+) \[{_DATE}\] \((?P<channel>[^()]*)\)$"),
    re.compile(rf"^\((?P<channel>[^()]*)\) (?P<title>.+) \[{_DATE}\]$"),
    re.compile(rf"^\((?P<channel>[^()]*)\) (?P<title>.+) - {_DATE}$"),
    re.compile(rf"^(?P<title>.+) - {_DATE}$"),
)

# Named for the tests that assert the bracket layouts stay mutually disjoint.
LAYOUT_TITLE_FIRST, LAYOUT_CHANNEL_FIRST = LAYOUTS[0], LAYOUTS[1]


def parse_stem(stem: str) -> tuple[str, str, str | None] | None:
    """Splits a legacy filename stem into (title, YYYY-MM-DD, channel-or-None).

    The date is checked against the calendar, not just the digit shape: a
    title ending in something like `[2026-13-45]` is a title, not a date, and
    accepting it would invent a match out of a coincidence.
    """
    for pattern in LAYOUTS:
        m = pattern.match(stem)
        if m and _is_real_date(m["date"]):
            channel = m.groupdict().get("channel")
            return m["title"], m["date"], channel
    return None


def _is_real_date(text: str) -> bool:
    try:
        datetime.strptime(text, "%Y-%m-%d")
        return True
    except ValueError:
        return False


def restrict(text: str) -> str:
    """The lossy form `--restrict-filenames` writes: spaces and punctuation
    collapse to underscores. Used only for comparison, never for output."""
    return sanitize_filename(text, restricted=True)


def utc_date(epoch: int | None) -> str | None:
    if epoch is None:
        return None
    return datetime.fromtimestamp(epoch, tz=timezone.utc).strftime("%Y-%m-%d")


def is_bucketed(epoch: int | None) -> bool:
    """True when a database date is a rounded bucket rather than a real one.

    mytube backfills dates with `--extractor-args youtubetab:approximate_date`
    (ytdlp.rs), because a channel listing exposes only "1 month ago" / "3 weeks
    ago". yt-dlp turns those into midnight-UTC timestamps measured back from
    the poll, so hundreds of unrelated videos collapse onto one value -- 364 of
    them onto 2026-07-30 in a real library. RSS later overwrites the recent
    window with true timestamps, which carry a time of day.

    Midnight exactly is therefore the signature of a date that means "roughly
    this long ago", not "uploaded on this day".
    """
    return epoch is not None and epoch % 86400 == 0


# --------------------------------------------------------------------------
# Scanning
# --------------------------------------------------------------------------

@dataclass
class Scanned:
    path: Path
    title_token: str | None = None      # restricted title from the filename
    date: str | None = None             # YYYY-MM-DD as the old download saw it
    channel_token: str | None = None    # restricted channel from the filename
    sidecar: dict | None = None         # parsed .info.json, when one exists
    layout_known: bool = True           # False when the stem matched no layout

    @property
    def ext(self) -> str:
        return self.path.suffix.lstrip(".")

    @classmethod
    def read(cls, path: Path, sidecar: dict | None = None) -> "Scanned":
        """Builds a Scanned from a path, applying the layout fallback."""
        item = cls(path=path, sidecar=sidecar)
        parsed = parse_stem(path.stem)
        if parsed:
            item.title_token, item.date, item.channel_token = parsed
        else:
            # No recognised layout. The stem is still a restricted title, so
            # let the library-wide search try it -- but with neither a date
            # nor a channel to corroborate, it can never be CERTAIN.
            item.title_token, item.layout_known = path.stem, False
        return item


def scan(source: Path, download_dir: Path) -> list[Scanned]:
    """Walks `source` for video files, reading a .info.json sidecar when present.

    Files already inside the mytube output tree are skipped: they are the
    library, not the archive.
    """
    found: list[Scanned] = []
    for path in sorted(source.rglob("*")):
        if not path.is_file() or path.suffix.lower() not in VIDEO_EXTS:
            continue
        if _is_within(path, download_dir):
            continue

        sidecar = None
        sidecar_path = path.with_suffix(".info.json")
        if sidecar_path.is_file():
            try:
                sidecar = json.loads(sidecar_path.read_text(encoding="utf-8"))
            except (OSError, ValueError):
                sidecar = None

        found.append(Scanned.read(path, sidecar))
    return found


def _is_within(path: Path, parent: Path) -> bool:
    try:
        path.resolve().relative_to(parent.resolve())
        return True
    except (ValueError, OSError):
        return False


# --------------------------------------------------------------------------
# The mytube database
# --------------------------------------------------------------------------

@dataclass
class Library:
    videos_by_id: dict[str, sqlite3.Row]
    # restricted channel title -> restricted video title -> rows
    by_channel_token: dict[str, dict[str, list[sqlite3.Row]]]
    channel_ids: dict[str, str]   # channel id -> restricted channel title
    channel_sizes: dict[str, int]
    # Tokens that two differently-named channels both collapse onto. Their
    # videos share one pool, so a match inside them cannot be CERTAIN.
    blurred_channels: set[str]

    @classmethod
    def load(cls, conn: sqlite3.Connection) -> "Library":
        rows = conn.execute(
            """
            SELECT v.id, v.title, v.published_at, v.download_state, v.file_path,
                   c.id AS channel_id, c.title AS channel_title
            FROM videos v JOIN channels c ON c.id = v.channel_id
            """
        ).fetchall()

        by_id: dict[str, sqlite3.Row] = {}
        by_channel: dict[str, dict[str, list[sqlite3.Row]]] = {}
        channel_ids: dict[str, str] = {}
        sizes: dict[str, int] = {}
        names_per_token: dict[str, set[str]] = {}

        for row in rows:
            by_id[row["id"]] = row
            ch_token = restrict(row["channel_title"])
            channel_ids[row["channel_id"]] = ch_token
            sizes[ch_token] = sizes.get(ch_token, 0) + 1
            names_per_token.setdefault(ch_token, set()).add(row["channel_title"])
            by_channel.setdefault(ch_token, {}).setdefault(
                restrict(row["title"]), []
            ).append(row)

        blurred = {tok for tok, names in names_per_token.items() if len(names) > 1}
        return cls(by_id, by_channel, channel_ids, sizes, blurred)


# --------------------------------------------------------------------------
# Matching
# --------------------------------------------------------------------------

@dataclass
class Result:
    item: Scanned
    group: str
    reason: str
    video: sqlite3.Row | None = None
    dest: Path | None = None
    candidates: list[sqlite3.Row] = field(default_factory=list)
    upload_date: str | None = None
    uploader: str | None = None


def classify(item: Scanned, lib: Library, network) -> Result:
    """Decides which group a scanned file belongs to.

    Confidence comes from *how* the match was made, never from a similarity
    score. A file is CERTAIN only when its identity is pinned by a video id,
    or by a title that is unique within its channel and a date that agrees.
    """
    # 1. A sidecar carries the real video id -- nothing to infer.
    if item.sidecar and item.sidecar.get("id"):
        vid = item.sidecar["id"]
        row = lib.videos_by_id.get(vid)
        if row is None:
            return Result(item, UNIDENTIFIED, f"sidecar id {vid} is not loaded in mytube")
        return Result(
            item, CERTAIN, f"sidecar id {vid}",
            video=row,
            upload_date=_sidecar_date(item) or item.date or utc_date(row["published_at"]),
            uploader=item.sidecar.get("uploader") or row["channel_title"],
        )

    if item.channel_token is None:
        # The filename names no channel, so the whole library is the pool.
        candidates = [row
                      for titles in lib.by_channel_token.values()
                      for row in titles.get(item.title_token, [])]
        scope, pool = "library", len(lib.videos_by_id)
    else:
        titles = lib.by_channel_token.get(item.channel_token)
        if titles is None:
            return Result(item, UNIDENTIFIED,
                          f"channel '{item.channel_token}' is not in mytube")
        candidates = titles.get(item.title_token, [])
        scope, pool = "channel", lib.channel_sizes.get(item.channel_token, 0)

    if not candidates:
        note = "" if item.layout_known else " · filename matches no known layout"
        return Result(
            item, UNIDENTIFIED,
            f"no title match among {pool} loaded video(s) for this {scope}{note}",
        )

    if len(candidates) == 1:
        row = candidates[0]
        db_date = utc_date(row["published_at"])
        common = dict(video=row, upload_date=item.date or db_date,
                      uploader=row["channel_title"])
        if item.date is None:
            # With no date in the name, the destination has to fall back to the
            # database date -- and if that is a bucket, the new filename would
            # carry a date the video was never uploaded on. Say so plainly.
            note = (f", so the new name would use the rounded bucket {db_date}"
                    if is_bucketed(row["published_at"]) else "")
            return Result(item, WARNING,
                          f"title unique in {scope} · filename carries no date{note}",
                          **common)
        if db_date == item.date:
            return _certain_unless_blurred(
                item, lib, f"title unique in {scope} · date exact", common)
        if is_bucketed(row["published_at"]):
            # The database date is "roughly this long ago", so it cannot
            # contradict the filename -- which carries yt-dlp's real
            # upload_date, captured when the file was downloaded.
            return _certain_unless_blurred(
                item, lib,
                f"title unique in {scope} · db date {db_date} is a rounded "
                f"bucket, ignored", common)
        if db_date is None:
            return Result(item, WARNING,
                          f"title unique in {scope} · db has no date", **common)
        # An exact database date that disagrees is a real conflict.
        return Result(
            item, WARNING,
            f"title unique in {scope} · file {item.date} vs db {db_date} (both exact)",
            **common,
        )

    if item.date is None:
        return Result(item, UNIDENTIFIED,
                      f"ambiguous: {len(candidates)} candidates share this title "
                      f"and the filename carries no date to separate them")

    # Several loaded videos share this exact title -- the channel publishes
    # story parts under one title, so the upload date is the identity, not
    # noise. Try the database dates first; they are exact for the recent RSS
    # window and only approximate for backfilled rows.
    # Only genuine dates may separate candidates: a bucketed date that happens
    # to equal the filename's date would otherwise "resolve" a tie by accident.
    exact = [r for r in candidates
             if not is_bucketed(r["published_at"])
             and utc_date(r["published_at"]) == item.date]
    if len(exact) == 1:
        row = exact[0]
        return Result(
            item, CERTAIN,
            f"{len(candidates)} share this title · date picks one",
            video=row, upload_date=item.date, uploader=row["channel_title"],
        )

    # Database dates could not separate them. Ask YouTube for the true upload
    # date of just the tied ids -- a handful, once.
    if network is not None:
        true_dates = network.upload_dates([r["id"] for r in candidates])
        if true_dates:
            hits = [r for r in candidates if true_dates.get(r["id"]) == item.date]
            if len(hits) == 1:
                row = hits[0]
                return Result(
                    item, CERTAIN,
                    f"{len(candidates)} share this title · true upload date picks one",
                    video=row, upload_date=item.date, uploader=row["channel_title"],
                )

    dates = " / ".join(sorted({utc_date(r["published_at"]) or "unknown" for r in candidates}))
    return Result(
        item, UNIDENTIFIED,
        f"ambiguous: {len(candidates)} candidates share this title, dates {dates}",
        candidates=candidates,
    )


def _certain_unless_blurred(item: Scanned, lib: Library, reason: str,
                            common: dict) -> Result:
    """CERTAIN, unless two channel names collapse onto this one token -- then
    the channel is not really pinned down and the match stays a warning."""
    if item.channel_token in lib.blurred_channels:
        return Result(item, WARNING,
                      f"{reason}, but two channels share the name "
                      f"'{item.channel_token}'", **common)
    return Result(item, CERTAIN, reason, **common)


def _sidecar_date(item: Scanned) -> str | None:
    raw = (item.sidecar or {}).get("upload_date")
    if isinstance(raw, str) and len(raw) == 8 and raw.isdigit():
        return f"{raw[:4]}-{raw[4:6]}-{raw[6:]}"
    return None


class Network:
    """Fetches true upload dates for tie-breaking only. Configured like
    mytube's own yt-dlp calls so it succeeds in the same situations."""

    def __init__(self) -> None:
        self._cache: dict[str, str] = {}

    def upload_dates(self, video_ids: list[str]) -> dict[str, str]:
        missing = [v for v in video_ids if v not in self._cache]
        for vid in missing:
            self._cache[vid] = self._fetch(vid) or ""
        return {v: self._cache[v] for v in video_ids if self._cache.get(v)}

    def _fetch(self, video_id: str) -> str | None:
        url = f"https://www.youtube.com/watch?v={video_id}"
        for opts in ({"cookiesfrombrowser": ("firefox",)}, {}):
            try:
                with YoutubeDL({"quiet": True, "no_warnings": True,
                                "skip_download": True, "noprogress": True, **opts}) as ydl:
                    info = ydl.extract_info(url, download=False)
                raw = (info or {}).get("upload_date")
                if isinstance(raw, str) and len(raw) == 8:
                    return f"{raw[:4]}-{raw[4:6]}-{raw[6:]}"
            except Exception:
                continue
        return None


# --------------------------------------------------------------------------
# Destinations
# --------------------------------------------------------------------------

class Namer:
    """Resolves destination paths through yt-dlp itself, so an imported file
    lands exactly where mytube would have downloaded it."""

    def __init__(self, template: str, download_dir: Path) -> None:
        self._ydl = YoutubeDL({
            "outtmpl": template,
            "paths": {"home": str(download_dir)},
            # mytube passes --no-windows-filenames and does not restrict.
            "windowsfilenames": False,
            "restrictfilenames": False,
            "quiet": True,
            "no_warnings": True,
        })

    def dest(self, result: Result) -> Path:
        row = result.video
        info = {
            "id": row["id"],
            "title": row["title"],
            # mytube's template keys off %(uploader)s but its database stores
            # %(channel)s; a sidecar's real uploader wins when there is one.
            "uploader": result.uploader or row["channel_title"],
            "channel": row["channel_title"],
            "ext": result.item.ext,
        }
        if result.upload_date:
            info["upload_date"] = result.upload_date.replace("-", "")
        return Path(self._ydl.prepare_filename(info))


def unique_path(intended: Path, taken) -> Path:
    """Inserts ` (N)` before the extension, N from 2 -- mytube's own scheme
    (ytdlp.rs::unique_path), so collisions look the same either way."""
    if not taken(intended):
        return intended
    for n in range(2, 10_000):
        candidate = intended.with_name(f"{intended.stem} ({n}){intended.suffix}")
        if not taken(candidate):
            return candidate
    return intended.with_name(f"{intended.stem} ({os.getpid()}){intended.suffix}")


def resolve_conflicts(results: list[Result]) -> None:
    """Two files cannot claim one video, and a video mytube already has on
    disk is left alone. Certain matches win a contested video over warnings."""
    priority = {CERTAIN: 0, WARNING: 1, UNIDENTIFIED: 2}
    claimed: dict[str, Result] = {}

    for r in sorted(results, key=lambda r: (priority[r.group], str(r.item.path))):
        if r.video is None:
            continue

        existing = r.video["file_path"]
        if r.video["download_state"] == "done" and existing and Path(existing).exists():
            _demote(r, f"mytube already has this video at {existing}")
            continue

        vid = r.video["id"]
        if vid in claimed:
            _demote(r, f"already matched by {claimed[vid].item.path.name}")
            continue
        claimed[vid] = r


def _demote(r: Result, reason: str) -> None:
    r.group, r.video, r.dest, r.reason = UNIDENTIFIED, None, None, reason


def assign_destinations(results: list[Result], namer: Namer) -> None:
    claimed: set[Path] = set()
    for r in results:
        if r.video is None:
            continue
        r.dest = unique_path(
            namer.dest(r), lambda p: p in claimed or p.exists()
        )
        claimed.add(r.dest)


# --------------------------------------------------------------------------
# Report
# --------------------------------------------------------------------------

MARKS = {CERTAIN: "✓", WARNING: "!", UNIDENTIFIED: "?"}


def elide(text: str, width: int) -> str:
    if len(text) <= width:
        return text
    if width <= 1:
        return text[:width]
    keep = width - 1
    head = (keep * 2) // 3
    tail = keep - head
    return text[:head] + "…" + (text[-tail:] if tail else "")


def report(results: list[Result], source: Path, download_dir: Path) -> dict[str, int]:
    width = min(shutil.get_terminal_size((100, 24)).columns, 140)
    counts = {CERTAIN: 0, WARNING: 0, UNIDENTIFIED: 0}
    index = 0

    for group in (CERTAIN, WARNING, UNIDENTIFIED):
        members = [r for r in results if r.group == group]
        counts[group] = len(members)
        print(f"\n{group.upper()} ({len(members)})")
        print("─" * min(width, 70))
        if not members:
            print("  (none)")
            continue

        for r in members:
            index += 1
            src = str(r.item.path.relative_to(source))
            print(f"{index:>4}  {elide(src, width - 6)}")
            if r.dest is not None:
                rel = _relative(r.dest, download_dir)
                print(f"      → {elide(rel, width - 8)}")
            print(f"        {MARKS[group]} {elide(r.reason, width - 10)}")

    total = len(results)
    print(
        f"\n  {counts[CERTAIN]} certain · {counts[WARNING]} warning · "
        f"{counts[UNIDENTIFIED]} unidentified · {total} scanned"
    )
    return counts


def _relative(path: Path, base: Path) -> str:
    try:
        return str(path.relative_to(base))
    except ValueError:
        return str(path)


def prompt(counts: dict[str, int]) -> list[str] | None:
    if not counts[CERTAIN] and not counts[WARNING]:
        print("\nNothing to move.")
        return None
    print(
        f"\nApply which?  [1] certain only ({counts[CERTAIN]})  "
        f"[2] certain + warning ({counts[CERTAIN] + counts[WARNING]})  [n] nothing"
    )
    while True:
        try:
            choice = input("> ").strip().lower()
        except (EOFError, KeyboardInterrupt):
            print()
            return None
        if choice == "1":
            return [CERTAIN]
        if choice == "2":
            return [CERTAIN, WARNING]
        if choice in {"n", "no", "", "q"}:
            return None
        print("Pick 1, 2, or n.")


# --------------------------------------------------------------------------
# Apply
# --------------------------------------------------------------------------

def apply(results: list[Result], groups: list[str], conn: sqlite3.Connection) -> None:
    selected = [r for r in results if r.group in groups and r.dest is not None]
    moved = 0
    failures: list[tuple[Result, str]] = []

    for r in selected:
        try:
            r.dest.parent.mkdir(parents=True, exist_ok=True)
            # shutil.move falls back to copy+unlink across filesystems.
            shutil.move(str(r.item.path), str(r.dest))
        except OSError as exc:
            failures.append((r, str(exc)))
            continue

        conn.execute(
            "UPDATE videos SET download_state = 'done', file_path = ? WHERE id = ?",
            (str(r.dest), r.video["id"]),
        )
        moved += 1

    conn.commit()

    print(f"\nMoved {moved} file(s); {moved} video(s) marked downloaded in mytube.")
    if failures:
        print(f"{len(failures)} failed:")
        for r, err in failures:
            print(f"  {r.item.path.name}\n    {err}")


# --------------------------------------------------------------------------

def load_settings() -> tuple[Path, str]:
    try:
        raw = json.loads(SETTINGS_PATH.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        raw = {}
    return (
        Path(raw.get("download_dir") or DEFAULT_DOWNLOAD_DIR),
        raw.get("filename_template") or DEFAULT_TEMPLATE,
    )


def main() -> int:
    ap = argparse.ArgumentParser(
        description="File an existing video archive into mytube's library.",
    )
    ap.add_argument("source", type=Path, help="directory to scan (searched recursively)")
    ap.add_argument("--dry-run", action="store_true",
                    help="print the table and exit without prompting")
    ap.add_argument("--no-network", action="store_true",
                    help="never contact YouTube; leave title ties unidentified")
    args = ap.parse_args()

    if not args.source.is_dir():
        print(f"Not a directory: {args.source}", file=sys.stderr)
        return 1
    if not DB_PATH.exists():
        print(f"No mytube database at {DB_PATH}", file=sys.stderr)
        return 1

    download_dir, template = load_settings()
    print(f"source      {args.source}")
    print(f"destination {download_dir}")
    print(f"template    {template}")

    conn = sqlite3.connect(DB_PATH)
    conn.row_factory = sqlite3.Row
    conn.execute("PRAGMA busy_timeout = 5000")

    lib = Library.load(conn)
    if not lib.videos_by_id:
        print("\nmytube has no videos loaded, so nothing can be matched.")
        print("Add your subscriptions and let them poll first.")
        return 0
    print(f"library     {len(lib.videos_by_id)} videos across "
          f"{len(lib.channel_ids)} channel(s)")

    items = scan(args.source, download_dir)
    if not items:
        print(f"\nNo video files found under {args.source}")
        return 0

    network = None if args.no_network else Network()
    results = [classify(item, lib, network) for item in items]
    resolve_conflicts(results)
    assign_destinations(results, Namer(template, download_dir))

    counts = report(results, args.source, download_dir)

    if args.dry_run:
        return 0

    if counts[CERTAIN] or counts[WARNING]:
        print("\nClose mytube first if it is running: it writes to this same "
              "database and will not notice rows changing under it.")

    groups = prompt(counts)
    if groups is None:
        print("Nothing moved.")
        return 0

    apply(results, groups, conn)
    return 0


if __name__ == "__main__":
    sys.exit(main())
