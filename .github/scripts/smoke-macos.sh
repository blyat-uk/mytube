#!/usr/bin/env bash
# The macOS build's smoke tests, run by release.yml straight after `tauri
# build`, from the repository root. $VERSION is the version the tag names;
# $RUNNER_TEMP is the runner's. Bash 3.2 is all a macOS runner has.
#
# Everything runs from inside the .app: that binary, not target/release's, is
# what a user gets.
#
# Each check is a collapsed group in the log. A failure becomes an annotation
# naming the check, since job logs need a signed-in viewer and annotations do
# not.
set -euo pipefail

check=""
reported=0
fail() {
  echo "::error title=Smoke test failed: $check::$*"
  reported=1
  exit 1
}
# Any other way out -- a command that failed under `set -e` -- is reported
# here, against the check it happened in.
on_exit() {
  local status=$?
  if (( status != 0 && reported == 0 )); then
    echo "::error title=Smoke test failed: $check::exited with $status"
  fi
}
trap on_exit EXIT

begin() { check=$1; echo "::group::$1"; }
end() { echo "::endgroup::"; }

bundle=src-tauri/target/release/bundle
app=$bundle/macos/mytube.app

begin "the .app"
codesign --verify --deep --strict --verbose=2 "$app"
out=$("$app/Contents/MacOS/mytube" --version)
echo "$out"
[[ "$out" == "mytube $VERSION" ]] || fail "expected 'mytube $VERSION', got '$out'"
hdiutil verify "$bundle"/dmg/mytube_"$VERSION"_*.dmg
end

# The real first-run path: yt-dlp, ffmpeg and deno downloaded fresh from their
# publishers, each checksum verified, each one run. The report is printed
# whatever the outcome.
begin "self-test"
dir="$RUNNER_TEMP/tools"
code=0
"$app/Contents/MacOS/mytube" --self-test "$dir" || code=$?
echo "--- $dir/self-test.txt"
cat "$dir/self-test.txt" || echo "::error::self-test.txt was not written"
(( code == 0 )) || fail "mytube --self-test exited with $code"
end
