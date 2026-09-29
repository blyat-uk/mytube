# MyTube

**Your feed. Your files.** MyTube follows the channels you pick, downloads what they post and plays it in your own player. No recommendations, and nothing sent back to YouTube.

[![MyTube](.github/readme/website.png)](https://mytube.blyat.uk/)

**Website: [mytube.blyat.uk](https://mytube.blyat.uk/)**

## Download

Get the latest build from the [releases page](https://github.com/blyat-uk/mytube/releases/latest):

| OS | File |
|---|---|
| Windows 10/11 (x64) | `-windows-x64-setup.exe` (per-user installer, no admin rights needed) |
| macOS 11+ (Apple Silicon) | `-macos-arm64.dmg` |
| Linux x86_64, Debian / Ubuntu 22.04+ | `-linux-x86_64.deb` |
| Linux x86_64, Fedora / openSUSE | `-linux-x86_64.rpm` |
| Linux x86_64, anything else | `-linux-x86_64.AppImage` |

Each release lists every file and what it is for, plus `SHA256SUMS.txt` to verify them against.

## First run

MyTube uses three programs it does not ship: **yt-dlp**, **ffmpeg** and **deno** (the JavaScript runtime yt-dlp needs for YouTube). The first time it starts it downloads whichever of them it cannot find on your system: about 150 MB, straight from each publisher's own releases, each checked against its published SHA-256, into your local data folder:

| OS | Tools folder |
|---|---|
| Windows | `%LOCALAPPDATA%\mytube\bin` |
| macOS | `~/Library/Application Support/mytube/bin` |
| Linux | `~/.local/share/mytube/bin` |

Unpacked, all three take roughly 500 MB on disk (the static ffmpeg and ffprobe builds are about 350 MB of that). Any tool already on your system is used instead and not downloaded. Settings → Tools shows where each one came from; yt-dlp keeps itself up to date once a day.

Downloads go out with the YouTube cookies of a browser you are signed in to (Settings → YouTube cookies; Automatic picks Firefox when it finds a profile). That is what reaches members-only videos and keeps YouTube's bot checks at bay. Firefox works on every OS and is the recommendation. Chrome, Edge and other Chromium browsers cannot be read on Windows; on macOS they ask for Keychain access. A `cookies.txt` file works everywhere.

## Platform notes

- **Windows:** the installer is not code-signed; SmartScreen may ask you to confirm (More info → Run anyway).
- **macOS:** the app is not notarized. After copying it to Applications, open it with right-click → Open, or allow it under System Settings → Privacy & Security, or run `xattr -dr com.apple.quarantine /Applications/mytube.app`.
- **Linux:** the `.deb` and `.rpm` pull in WebKitGTK 4.1 themselves. If the AppImage does not start, install your distribution's FUSE 2 package (`libfuse2` / `fuse2`) or run it with `--appimage-extract-and-run`. The tray icon needs a StatusNotifierItem host (built into KDE; GNOME needs the AppIndicator extension).

## Uninstalling

Uninstalling removes the app but not your data or the tools it downloaded. Delete these folders as well to remove everything:

| OS | Settings, library and thumbnails | Downloaded tools |
|---|---|---|
| Windows | `%APPDATA%\mytube` | `%LOCALAPPDATA%\mytube\bin` (the uninstaller leaves it) |
| macOS | `~/Library/Application Support/mytube` | `~/Library/Application Support/mytube/bin` |
| Linux | `~/.config/mytube` | `~/.local/share/mytube/bin` |

Downloaded videos stay wherever you saved them.

## Run from source

[Bun](https://bun.sh) and a stable Rust toolchain, plus the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) for your OS.

```bash
git clone https://github.com/blyat-uk/mytube.git
cd mytube
bun install
bun run tauri dev
```

## Develop

```bash
bun run test                                        # frontend tests
cargo test --manifest-path src-tauri/Cargo.toml     # backend tests
bun run tauri build                                 # release build for this OS
```

Pushing a `vX.Y.Z` tag that matches the version in `tauri.conf.json`, `Cargo.toml` and `package.json` builds, tests and publishes a release for all three platforms. Its notes are the commits since the previous tag followed by the downloads table, nothing else, so anything a user needs to know goes in this README.

## License

[MIT](LICENSE)
