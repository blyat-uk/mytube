#!/usr/bin/env python3
"""Mark mytube videos as watched from your YouTube watch history.

YouTube draws a red progress bar on thumbnails you have partly watched. That
percentage is the only place completion is exposed — Takeout's watch-history.json
records *that* you opened a video but not how far you got, and yt-dlp drops the
overlay entirely. So this reads the bar.

It walks /feed/history through InnerTube using your Firefox cookies, collects
`startPercent` per video, and marks everything at or above THRESHOLD as watched
in the mytube database.

Nothing is written until you confirm: the scan produces a plan, prints stats, and
asks. The plan is cached, so answering later costs no re-scan.

Usage:
    python3 scripts/sync_watched_from_youtube.py            # scan, stats, ask
    python3 scripts/sync_watched_from_youtube.py --rescan   # ignore cached scan
    echo y | python3 scripts/sync_watched_from_youtube.py   # answer from a pipe
"""

from __future__ import annotations

import argparse
import gzip
import hashlib
import itertools
import json
import shutil
import sqlite3
import sys
import tempfile
import time
import urllib.error
import urllib.request
from collections import Counter
from pathlib import Path

THRESHOLD = 90
ORIGIN = "https://www.youtube.com"
DB_PATH = Path.home() / ".config/mytube/mytube.db"
CACHE_DIR = Path.home() / ".cache/mytube-watch-sync"
PLAN_PATH = CACHE_DIR / "scan.json"
UA = "Mozilla/5.0 (X11; Linux x86_64; rv:128.0) Gecko/20100101 Firefox/128.0"

# Sending every youtube.com cookie (226 here) trips HTTP 413. These are the ones
# that actually carry the session; the rest are per-app state.
AUTH_COOKIES = {
    "SID", "HSID", "SSID", "APISID", "SAPISID", "LOGIN_INFO", "PREF", "SOCS",
    "SIDCC", "VISITOR_INFO1_LIVE", "VISITOR_PRIVACY_METADATA",
    "__Secure-1PSID", "__Secure-3PSID", "__Secure-1PAPISID", "__Secure-3PAPISID",
    "__Secure-1PSIDTS", "__Secure-3PSIDTS", "__Secure-1PSIDCC", "__Secure-3PSIDCC",
}


# ---------------------------------------------------------------- parsing --
# Pure functions over InnerTube payloads. These are the fragile part — YouTube
# renames renderers without notice — so they are kept free of I/O and tested
# against captured payloads in test_sync_watched_from_youtube.py.

def iter_key(node, key):
    """Yield every value stored under `key`, at any depth."""
    if isinstance(node, dict):
        for k, v in node.items():
            if k == key:
                yield v
            yield from iter_key(v, key)
    elif isinstance(node, list):
        for v in node:
            yield from iter_key(v, key)


def first_key(node, key, default=None):
    return next(iter_key(node, key), default)


def extract_videos(payload):
    """Pull {video_id: (title, percent)} out of an InnerTube payload.

    `percent` is None when no progress bar is drawn — an unstarted video, or a
    live stream. Note YouTube floors a drawn bar at 10, so a 3% watch reports
    as 10; that only compresses the bottom of the range and cannot manufacture
    a false positive at the 90 end.
    """
    out = {}
    for tile in iter_key(payload, "lockupViewModel"):
        if tile.get("contentType") != "LOCKUP_CONTENT_TYPE_VIDEO":
            continue
        vid = tile.get("contentId")
        if not vid:
            continue
        meta = tile.get("metadata", {}).get("lockupMetadataViewModel", {})
        title = meta.get("title", {}).get("content") or "(untitled)"
        bar = first_key(tile, "thumbnailOverlayProgressBarViewModel")
        out[vid] = (title, bar.get("startPercent") if bar else None)
    return out


def next_token(payload):
    """The continuation token for the following page, or None at the end."""
    for cmd in iter_key(payload, "continuationCommand"):
        if cmd.get("token"):
            return cmd["token"]
    return None


def is_watched(percent, threshold=THRESHOLD):
    return percent is not None and percent >= threshold


# ---------------------------------------------------------------- cookies --

def firefox_profile():
    """The profile with the most recently modified cookie jar."""
    roots = [Path.home() / ".mozilla/firefox", Path.home() / "snap/firefox/common/.mozilla/firefox"]
    jars = [j for r in roots if r.is_dir() for j in r.glob("*/cookies.sqlite")]
    if not jars:
        sys.exit("No Firefox cookies.sqlite found — is Firefox installed?")
    return max(jars, key=lambda p: p.stat().st_mtime)


def load_cookies():
    jar = firefox_profile()
    tmp = Path(tempfile.mkdtemp()) / "cookies.sqlite"
    shutil.copy2(jar, tmp)
    # Recent cookies may still be in the write-ahead log rather than the main db.
    for suffix in ("-wal", "-shm"):
        side = jar.with_name(jar.name + suffix)
        if side.exists():
            shutil.copy2(side, tmp.with_name(tmp.name + suffix))

    con = sqlite3.connect(tmp)
    rows = con.execute(
        "SELECT name, value FROM moz_cookies WHERE host LIKE '%youtube.com'"
    ).fetchall()
    con.close()

    jar_cookies = {n: v for n, v in rows if n in AUTH_COOKIES}
    if "SAPISID" not in jar_cookies and "__Secure-3PAPISID" not in jar_cookies:
        sys.exit(f"Not signed in to YouTube in {jar.parent.name} — log in with Firefox first.")
    print(f"  cookies  : {len(jar_cookies)} auth cookies from {jar.parent.name}")
    return jar_cookies


# -------------------------------------------------------------- innertube --

class InnerTube:
    def __init__(self, cookies):
        self.cookies = cookies
        self.header = "; ".join(f"{k}={v}" for k, v in cookies.items())
        self.requests = 0

    def _auth(self):
        """YouTube's cookie-derived request signature."""
        sapisid = self.cookies.get("SAPISID") or self.cookies["__Secure-3PAPISID"]
        ts = int(time.time())
        digest = hashlib.sha1(f"{ts} {sapisid} {ORIGIN}".encode()).hexdigest()
        return f"SAPISIDHASH {ts}_{digest}"

    def _fetch(self, url, body=None, auth=False):
        headers = {
            "Cookie": self.header, "User-Agent": UA,
            "Accept-Language": "en-US,en;q=0.9", "Accept-Encoding": "gzip",
        }
        if body is not None:
            headers["Content-Type"] = "application/json"
        if auth:
            headers |= {"Authorization": self._auth(), "X-Origin": ORIGIN,
                        "X-Goog-AuthUser": "0"}
        req = urllib.request.Request(url, data=body, headers=headers)
        with urllib.request.urlopen(req, timeout=45) as resp:
            raw = resp.read()
            if resp.headers.get("Content-Encoding") == "gzip":
                raw = gzip.decompress(raw)
        self.requests += 1
        return raw.decode("utf-8", "replace")

    def _retrying(self, url, body=None, auth=False):
        for attempt in range(5):
            try:
                return self._fetch(url, body, auth)
            except (urllib.error.HTTPError, urllib.error.URLError, TimeoutError) as e:
                code = getattr(e, "code", None)
                if code in (400, 401, 403) or attempt == 4:
                    raise
                back = 2 ** attempt
                print(f"\n  retrying after {e} (waiting {back}s)", file=sys.stderr)
                time.sleep(back)
        raise AssertionError("unreachable")

    def bootstrap(self):
        """Fetch the history page for the client version, API key and page one."""
        html = self._fetch(f"{ORIGIN}/feed/history")
        import re
        ver = re.search(r'"INNERTUBE_CLIENT_VERSION":"([^"]+)"', html)
        key = re.search(r'"INNERTUBE_API_KEY":"([^"]+)"', html)
        data = re.search(r"var ytInitialData\s*=\s*(\{.*?\});</script>", html, re.S)
        if not (ver and key and data):
            sys.exit("Could not parse the history page — YouTube markup changed, "
                     "or the session is not signed in.")
        self.version, self.key = ver.group(1), key.group(1)
        print(f"  client   : WEB {self.version}")
        return json.loads(data.group(1))

    def page(self, token):
        body = json.dumps({
            "context": {"client": {"clientName": "WEB", "clientVersion": self.version,
                                   "hl": "en", "gl": "US"}},
            "continuation": token,
        }).encode()
        url = f"{ORIGIN}/youtubei/v1/browse?key={self.key}&prettyPrint=false"
        return json.loads(self._retrying(url, body, auth=True))


# --------------------------------------------------------------- progress --

class Progress:
    """A live bar on a terminal, periodic log lines when piped to a file."""

    FRAMES = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"

    def __init__(self, every=25):
        self.tty = sys.stderr.isatty()
        self.every = every
        self.start = time.time()
        self.spin = itertools.cycle(self.FRAMES)

    def update(self, pages, videos, matched, hits, final=False):
        elapsed = time.time() - self.start
        rate = videos / elapsed if elapsed else 0
        line = (f"pages {pages:>4} │ history {videos:>6} │ in library {matched:>4} "
                f"│ ≥{THRESHOLD}% {hits:>4} │ {elapsed:>5.0f}s │ {rate:>4.0f} vid/s")
        if self.tty:
            end = "\n" if final else ""
            print(f"\r  {'✓' if final else next(self.spin)} {line}",
                  end=end, file=sys.stderr, flush=True)
        elif final or pages % self.every == 0:
            print(f"  {line}", file=sys.stderr, flush=True)


# --------------------------------------------------------------- scanning --

def scan(library):
    """Walk the whole history feed. Returns {video_id: [title, percent]}."""
    print("\nConnecting to YouTube")
    tube = InnerTube(load_cookies())
    payload = tube.bootstrap()

    found, pages, empty_streak = {}, 0, 0
    bar = Progress()
    print("\nScanning watch history (Ctrl-C to stop early and keep what was found)\n")
    try:
        while True:
            batch = extract_videos(payload)
            found.update(batch)
            token = next_token(payload)

            # A continuation that returns nothing is how a failed SAPISIDHASH
            # presents: HTTP 200, empty shell, no error. Silently reporting
            # "0 watched" would be the worst outcome, so treat the first one as
            # fatal and give up after a run of them.
            if pages and not batch:
                empty_streak += 1
                if pages == 1:
                    sys.exit("\nFirst continuation returned no videos — authentication "
                             "failed silently. Re-log in to YouTube in Firefox.")
                if empty_streak >= 3:
                    print("\n  three empty pages in a row — stopping", file=sys.stderr)
                    break
            else:
                empty_streak = 0

            matched = library.keys() & found.keys()
            hits = sum(1 for v in matched if is_watched(found[v][1]))
            bar.update(pages, len(found), len(matched), hits)

            if not token:
                break
            pages += 1
            payload = tube.page(token)
            if pages % 25 == 0:
                save_scan(found)
            time.sleep(0.35)
    except KeyboardInterrupt:
        print("\n  interrupted — keeping what was scanned so far", file=sys.stderr)

    matched = library.keys() & found.keys()
    bar.update(pages, len(found), len(matched),
               sum(1 for v in matched if is_watched(found[v][1])), final=True)
    print(f"\n  {tube.requests} requests in {time.time() - bar.start:.0f}s")
    save_scan(found)
    return found


def save_scan(found):
    CACHE_DIR.mkdir(parents=True, exist_ok=True)
    tmp = PLAN_PATH.with_suffix(".tmp")
    tmp.write_text(json.dumps({"saved_at": int(time.time()), "videos": found}))
    tmp.replace(PLAN_PATH)


def load_scan():
    if not PLAN_PATH.exists():
        return None
    blob = json.loads(PLAN_PATH.read_text())
    age = (time.time() - blob["saved_at"]) / 3600
    print(f"Reusing cached scan of {len(blob['videos'])} videos "
          f"({age:.1f}h old, {PLAN_PATH}). Pass --rescan to refresh.")
    return blob["videos"]


# --------------------------------------------------------------- database --

def read_library(db):
    con = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
    try:
        return {vid: (title, bool(w))
                for vid, title, w in con.execute("SELECT id, title, watched FROM videos")}
    finally:
        con.close()


def backup(db):
    stamp = time.strftime("%Y%m%d-%H%M%S")
    dest = db.with_name(f"{db.name}.{stamp}.bak")
    # VACUUM INTO folds in the write-ahead log, so the copy is self-contained
    # even though the app may have left a 4 MB -wal beside the database.
    con = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
    try:
        con.execute("VACUUM INTO ?", (str(dest),))
    finally:
        con.close()
    return dest


def apply(db, video_ids):
    con = sqlite3.connect(db)
    try:
        now = int(time.time())
        with con:
            con.executemany(
                "UPDATE videos SET watched=1, watched_at=?2 WHERE id=?1 AND watched=0",
                [(v, now) for v in video_ids])
        return con.total_changes
    finally:
        con.close()


# ------------------------------------------------------------------ stats --

def report(found, library):
    """Print the numbers and return the ids to flip."""
    matched = {v: found[v] for v in library.keys() & found.keys()}
    at_threshold = {v: t for v, (t, p) in matched.items() if is_watched(p)}
    already = {v for v in at_threshold if library[v][1]}
    todo = sorted(at_threshold.keys() - already)

    buckets = Counter()
    for _, pct in found.values():
        buckets["no bar" if pct is None else
                "100%" if pct == 100 else
                f"{pct // 10 * 10}–{pct // 10 * 10 + 9}%"] += 1

    def rule(label):
        print(f"\n\033[1m{label}\033[0m\n" + "─" * 58)

    rule("Watch history")
    print(f"  videos seen            {len(found):>6}")
    for key in sorted(buckets, key=lambda k: (k == "no bar", k)):
        share = buckets[key] / len(found) * 100
        print(f"    {key:<20} {buckets[key]:>6}  {'█' * round(share / 2):<25} {share:>4.1f}%")

    rule("Your mytube library")
    print(f"  videos in library      {len(library):>6}")
    print(f"  found in history       {len(matched):>6}"
          f"  ({len(matched) / len(library) * 100:.1f}% of library)")
    print(f"  watched ≥{THRESHOLD}%           {len(at_threshold):>6}")
    print(f"  already marked         {len(already):>6}")
    print(f"  \033[1mto mark as watched     {len(todo):>6}\033[0m")

    if todo:
        rule(f"Sample of what will be marked ({min(10, len(todo))} of {len(todo)})")
        for vid in todo[:10]:
            title, pct = found[vid]
            print(f"  {pct:>3}%  {title[:60]}")
    return todo


# ------------------------------------------------------------------- main --

def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--rescan", action="store_true", help="ignore the cached scan")
    ap.add_argument("--db", type=Path, default=DB_PATH)
    args = ap.parse_args()

    if not args.db.exists():
        sys.exit(f"No mytube database at {args.db}")

    library = read_library(args.db)
    print(f"mytube library: {len(library)} videos, "
          f"{sum(1 for _, w in library.values() if w)} already marked watched")

    found = None if args.rescan else load_scan()
    if found is None:
        found = scan(library)
    if not found:
        sys.exit("Nothing found in history — nothing to do.")

    todo = report(found, library)
    if not todo:
        print("\nNothing to do — every matching video is already marked watched.")
        return

    print()
    try:
        answer = input(f"Apply these {len(todo)} changes? [y/N] ").strip().lower()
    except EOFError:
        # No stdin: the scan is cached, so deciding later is free. Drop --rescan
        # from the suggestion — replaying it would discard that cache.
        cmd = " ".join(a for a in sys.argv if a != "--rescan")
        print(f"No input available — nothing was written.\n"
              f"Re-run to decide (the scan is cached, so it will be instant):\n"
              f"  echo y | python3 {cmd}")
        return

    if answer not in ("y", "yes"):
        print("Discarded — nothing was written.")
        return

    dest = backup(args.db)
    print(f"  backup   : {dest}")
    changed = apply(args.db, todo)
    print(f"  \033[1mupdated  : {changed} videos marked watched\033[0m")
    print("\nRestart mytube to see the change.")


if __name__ == "__main__":
    main()
