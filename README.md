# MyTube

A desktop YouTube subscription feed and downloader. MyTube follows channels
over RSS without a Google account, downloads videos with `yt-dlp`, hands
playback to the video player you already use, and keeps track of what you have
watched.

MyTube never embeds a player, never streams video itself, and never writes
anything back to YouTube. It runs on **Linux x86_64**, **Windows 10/11 x64** and
**Apple Silicon Macs** (macOS 11+).

- **Stack:** Tauri 2 (Rust) + React 19 + TypeScript + Vite, SQLite via `rusqlite`
- **Package manager:** [Bun](https://bun.sh) — `tauri.conf.json` invokes `bun run`
  directly, so npm/pnpm will not drive the Tauri build without editing that file

## Install

Download the file for your OS from the
[Releases page](https://github.com/blyat-uk/mytube/releases/latest), and check
it against that release's `SHA256SUMS.txt` if you like.

| OS | File | Notes |
| --- | --- | --- |
| Windows | `mytube-vX.Y.Z-windows-x64-setup.exe` | Installs for your user only, no admin rights. Not code-signed: SmartScreen may ask you to confirm (More info → Run anyway). |
| macOS | `mytube-vX.Y.Z-macos-arm64.dmg` | Drag MyTube to Applications. Not notarized: open it with right-click → Open, allow it under System Settings → Privacy & Security, or run `xattr -dr com.apple.quarantine /Applications/mytube.app`. |
| Debian / Ubuntu 22.04+ | `mytube-vX.Y.Z-linux-x86_64.deb` | `sudo apt install ./mytube-…deb` pulls in WebKitGTK. |
| Fedora / openSUSE | `mytube-vX.Y.Z-linux-x86_64.rpm` | `sudo dnf install ./mytube-…rpm` |
| Other Linux | `mytube-vX.Y.Z-linux-x86_64.AppImage` | `chmod +x` and run. Needs FUSE 2 (`libfuse2`), or run it with `--appimage-extract-and-run`. |

On Linux the tray icon needs a StatusNotifierItem host — built into KDE
Plasma; GNOME needs the AppIndicator extension. Without one, closing the window
quits the app instead of hiding it.

To install from source into your home directory instead, see
[packaging/README.md](packaging/README.md).

## First run

MyTube relies on three programs it does not ship:

| Tool | What for |
| --- | --- |
| `yt-dlp` | Reads channel listings and video metadata, and performs every download. |
| `ffmpeg` | Merges the separate video and audio streams into one `.mkv`. |
| `deno` | The JavaScript runtime `yt-dlp` needs to get past YouTube's player challenges. |

The first time it starts, MyTube downloads whichever of them it cannot find on
your system — about 150 MB in all, straight from each publisher's own releases,
each checked against its published SHA-256 before it is installed — into your
local data folder:

| OS | Managed tools |
| --- | --- |
| Linux | `~/.local/share/mytube/bin` |
| macOS | `~/Library/Application Support/mytube/bin` |
| Windows | `%LOCALAPPDATA%\mytube\bin` |

A `yt-dlp` of MyTube's own is preferred over one on your `PATH`, because it is
kept current: MyTube checks for a new release once a day (nightly channel by
default, switchable to stable). For `ffmpeg` and `deno` a copy already on your
system wins. **Settings → Tools** shows where each one came from, its version,
and any error, with a button to check for updates now. To force a particular
binary, set `ytdlp_path`, `ffmpeg_path` or `deno_path` in `settings.json`.

## YouTube cookies

Downloads go out with the YouTube cookies of a browser you are signed in to —
that is what reaches members-only videos and keeps YouTube's bot checks at bay.
Choose the source under **Settings → YouTube cookies**:

- **Automatic** (the default) uses Firefox when it finds a Firefox profile, and
  no cookies otherwise.
- **Firefox is the recommendation** on every OS.
- **Chrome, Edge, Brave and other Chromium browsers do not work on Windows**:
  their cookies are app-bound encrypted and `yt-dlp` cannot read them. On
  macOS they work but ask for Keychain access; Safari needs Full Disk Access.
- **A `cookies.txt` file** (Netscape format, exported by a browser extension)
  works everywhere and takes precedence over any browser.

## Playing videos

**Settings → Player** lists the players MyTube found (mpv, VLC, IINA, MPC-HC,
SMPlayer, Celluloid…) plus "System default", which opens the file with whatever
your OS associates with `.mkv`. "Custom command…" takes any command line; the
file path is appended to it.

## Where your data lives

Settings, the library database and cached thumbnails live in `mytube/` under
your config directory — `~/.config/mytube` on Linux,
`~/Library/Application Support/mytube` on macOS, `%APPDATA%\mytube` on Windows.
Deleting that folder resets MyTube to a clean state. Downloads go to
`~/Videos/mytube` (`~/Movies/mytube` on macOS) unless you choose another folder.
**Settings → Backup & transfer** exports the whole library to a zip that another
MyTube, on any OS, can import.

## Building from source

|           |                                                                   |
| --------- | ----------------------------------------------------------------- |
| Rust      | stable toolchain via [rustup](https://rustup.rs)                  |
| Bun       | 1.x                                                               |
| Linux     | WebKitGTK 4.1 plus the usual GTK build headers (below)            |
| Windows   | Microsoft C++ Build Tools; WebView2 (preinstalled on Windows 10/11) |
| macOS     | Xcode Command Line Tools (`xcode-select --install`)               |

The Linux system libraries are Tauri's standard prerequisites. No
appindicator library is needed: the tray speaks StatusNotifierItem itself.

```sh
# Arch / CachyOS
sudo pacman -S --needed webkit2gtk-4.1 base-devel curl wget file openssl librsvg

# Debian / Ubuntu
sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file \
  libxdo-dev libssl-dev librsvg2-dev

# Fedora
sudo dnf install webkit2gtk4.1-devel openssl-devel curl wget file \
  librsvg2-devel && sudo dnf group install "C Development Tools and Libraries"
```

### Running in development

```sh
bun install
bun run tauri dev
```

That builds the Rust binary in debug mode, starts Vite on
`http://localhost:1420`, and opens the app window. The frontend hot-reloads on
save; changing Rust triggers a rebuild and restart. The first run compiles the
whole dependency tree and takes a few minutes.

> `bun run dev` alone serves only the web frontend, which is not useful on its
> own — every Tauri command it calls will fail. Use `bun run tauri dev`.

### Building a release

```sh
bun run tauri build --no-bundle                  # just the binary
bun run tauri build --bundles deb,rpm            # Linux packages
bun run tauri build --bundles nsis               # Windows installer
bun run tauri build --bundles app,dmg            # macOS app and disk image
```

The binary lands in `src-tauri/target/release/` and each bundle under
`src-tauri/target/release/bundle/<kind>/`. `bundle.targets` is `"all"`, so a
plain `bun run tauri build` makes every bundle the host OS can — on Linux that
includes an AppImage, which downloads `linuxdeploy` first.

The release binary also answers two command-line flags, which CI runs on every
build:

```sh
mytube --version              # prints "mytube X.Y.Z"
mytube --self-test <dir>      # downloads all three tools fresh into <dir>, runs
                              # each, writes the report to <dir>/self-test.txt
```

After any new major feature, or if the user asks you to "release", make sure to build the binary and install it.

### Releases

Pushing a `vX.Y.Z` tag runs `.github/workflows/release.yml`: it checks the tag
against the version in `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml` and
`package.json` (bump all three together), builds on Ubuntu 22.04, Windows and
macOS, runs `--version` and `--self-test` on each, and publishes the bundles
with `SHA256SUMS.txt`. Running the workflow by hand builds and tests without
publishing. `.github/workflows/ci.yml` runs the test suites on all three OSes
for every push to `main` and every pull request.

## Tests

```sh
bun run test                                      # frontend, Vitest
cargo test --manifest-path src-tauri/Cargo.toml   # backend
```

`cargo test` needs `dist/` to exist (`bun run build` makes it). A few backend
tests are ignored by default because they reach outside the test process — the
network, or your real library:

```sh
cargo test --manifest-path src-tauri/Cargo.toml -- --ignored
```

The `scripts/` directory has its own standalone suites, run directly with
`python3` (no pytest):

```sh
python3 scripts/test_import_existing.py
python3 scripts/test_sync_watched_from_youtube.py
```

## Notes

**Wayland.** WebKitGTK's DMA-BUF renderer makes GDK abort during startup on some
compositors (`Error 71 (Protocol error) dispatching to Wayland display`,
reproduced on KDE Plasma / kwin_wayland). `main()` disables that renderer on
Linux unless you have already set `WEBKIT_DISABLE_DMABUF_RENDERER` yourself, so
the app keeps native Wayland and needs no wrapper script.

## Licences of the downloaded tools

MyTube downloads `yt-dlp`, `ffmpeg` and `deno` as their publishers build them;
it does not modify or redistribute them. Each comes under its publisher's
licence: the `yt-dlp` standalone executables and the `ffmpeg` builds (from
[yt-dlp/FFmpeg-Builds](https://github.com/yt-dlp/FFmpeg-Builds) and
[martin-riedl.de](https://ffmpeg.martin-riedl.de)) are GPL, and `deno` is MIT.
