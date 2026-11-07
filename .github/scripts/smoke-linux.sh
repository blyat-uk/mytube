#!/usr/bin/env bash
# The Linux build's smoke tests, run by release.yml as root inside the
# ubuntu:22.04 container straight after `tauri build`, from the repository
# root. $VERSION is the version the tag names; $RUNNER_TEMP is the runner's.
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

expect_version() {
  echo "$1"
  [[ "$1" == "mytube $VERSION" ]] || fail "expected 'mytube $VERSION', got '$1'"
}

bundle=src-tauri/target/release/bundle

begin "version"
out=$(src-tauri/target/release/mytube --version)
expect_version "$out"
end

# The real first-run path: yt-dlp, ffmpeg and deno downloaded fresh from their
# publishers, each checksum verified, each one run. The report is printed
# whatever the outcome.
begin "self-test"
dir="$RUNNER_TEMP/tools"
code=0
src-tauri/target/release/mytube --self-test "$dir" || code=$?
echo "--- $dir/self-test.txt"
cat "$dir/self-test.txt" || echo "::error::self-test.txt was not written"
(( code == 0 )) || fail "mytube --self-test exited with $code"
end

# A container has no /dev/fuse, so the AppImage cannot mount itself.
begin "AppImage starts"
appimage=$(ls "$bundle"/appimage/mytube_"$VERSION"_*.AppImage)
chmod +x "$appimage"
"$appimage" --appimage-extract-and-run --version
end

begin ".deb installs and runs"
deb=$(ls "$bundle"/deb/mytube_"$VERSION"_*.deb)
# Root in the container already, and no sudo there to call.
DEBIAN_FRONTEND=noninteractive apt-get install -y "./$deb"
out=$(/usr/bin/mytube --version)
expect_version "$out"
end
