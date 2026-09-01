# MyTube

A local-first desktop app for following YouTube channels over RSS, downloading
videos with `yt-dlp`, and keeping track of what you have watched.

MyTube never embeds a player, never streams video itself, and never writes
anything back to YouTube. Playback is handed to a system video player of your
choosing. Linux only.

- **Stack:** Tauri 2 (Rust) + React 19 + TypeScript + Vite, SQLite via `rusqlite`
- **Package manager:** [Bun](https://bun.sh) — `tauri.conf.json` invokes `bun run`
  directly, so npm/pnpm will not drive the Tauri build without editing that file
- **Data lives in** `~/.config/mytube/` (database, settings, cached thumbnails)

## Requirements

### To build

|           |                                                                       |
| --------- | --------------------------------------------------------------------- |
| Rust      | stable toolchain via [rustup](https://rustup.rs) (built against 1.90) |
| Bun       | 1.x (built against 1.3.11)                                            |
| WebKitGTK | 4.1 (built against 2.52.6) plus the usual GTK build headers           |

The system libraries are Tauri's standard Linux prerequisites:

```sh
# Arch / CachyOS
sudo pacman -S --needed webkit2gtk-4.1 base-devel curl wget file openssl \
  appmenu-gtk-module libappindicator-gtk3 librsvg

# Debian / Ubuntu
sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file \
  libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev

# Fedora
sudo dnf install webkit2gtk4.1-devel openssl-devel curl wget file \
  libappindicator-gtk3-devel librsvg2-devel && sudo dnf group install "C Development Tools and Libraries"
```

### To run

`yt-dlp`, `ffmpeg`, a Firefox profile logged in to YouTube, and a video player
(`smplayer` by default). See
[packaging/README.md](packaging/README.md#runtime-requirements) for what each one
is for — those are runtime dependencies, so they are needed even if you never
build from source.

## Running in development

```sh
bun install
bun run tauri dev
```

That builds the Rust binary in debug mode, starts Vite on
`http://localhost:1420`, and opens the app window. The frontend hot-reloads on
save; changing Rust triggers a rebuild and restart.

The first run compiles the whole dependency tree and takes a few minutes.
Afterwards it is incremental.

> `bun run dev` alone serves only the web frontend, which is not useful on its
> own — every Tauri command it calls will fail. Use `bun run tauri dev`.

## Building a release

```sh
bun run tauri build --no-bundle
```

This type-checks and bundles the frontend (`tsc && vite build`), compiles the
Rust binary with optimisations, and produces:

| Artifact       | Path                                                            |
| -------------- | --------------------------------------------------------------- |
| Binary         | `src-tauri/target/release/mytube`                               |
| Debian package | `src-tauri/target/release/bundle/deb/mytube_0.1.0_amd64.deb`    |
| RPM package    | `src-tauri/target/release/bundle/rpm/mytube-0.1.0-1.x86_64.rpm` |

Only `deb` and `rpm` are built. AppImage is deliberately disabled — it requires a
`linuxdeploy` download that fails in this environment, and the app ships through
the `.desktop` launcher instead.

A release build is considerably slower than a debug one. To check that
everything compiles without producing bundles, `cargo build --release
--manifest-path src-tauri/Cargo.toml` is faster.

## Installing

The release binary is self-contained but is not placed anywhere by the build.
To install it into your home directory — no `sudo` at any point:

```sh
./packaging/install.sh
cp packaging/mytube.desktop ~/.local/share/applications/
```

[packaging/README.md](packaging/README.md) covers this properly, including PATH
setup, why the `.desktop` file uses the `sh -c` form, and how to uninstall.

After any new major feature, or if the user asks you to "release", make sure to build the binary and install it.

## Tests

```sh
bun run test                                  # frontend — 44 tests, Vitest
cargo test --manifest-path src-tauri/Cargo.toml   # backend — 118 tests
```

Two backend tests are ignored by default because they reach outside the test
process — one hits the network, one reads your real library:

```sh
cargo test --manifest-path src-tauri/Cargo.toml -- --ignored
```

The `scripts/` directory has its own standalone suites, run directly — no pytest,
no runner, just `python3`:

```sh
python3 scripts/test_import_existing.py            # 55 tests
python3 scripts/test_sync_watched_from_youtube.py  # 25 tests
```

## Notes

**Wayland.** WebKitGTK's DMA-BUF renderer makes GDK abort during startup on some
compositors (`Error 71 (Protocol error) dispatching to Wayland display`,
reproduced on KDE Plasma / kwin_wayland). `main()` disables that renderer on
Linux unless you have already set `WEBKIT_DISABLE_DMABUF_RENDERER` yourself, so
the app keeps native Wayland and needs no wrapper script. Nothing to configure —
this is only here so the environment variable is not a surprise.

**Resetting.** Deleting `~/.config/mytube/` returns the app to a clean state.
