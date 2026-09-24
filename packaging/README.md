# Packaging MyTube for Linux

**Most people want a release instead.** Ready-made builds for Linux (`.deb`,
`.rpm`, AppImage), Windows and macOS are on the
[Releases page](https://github.com/blyat-uk/mytube/releases/latest); the
[top-level README](../README.md#install) says which file to pick. This page is
for building from source and installing the result into your home directory,
which is how a development checkout stays launchable.

A home install lives entirely under your home directory. Nothing here needs `sudo`.

## Install

1. **Build the release binary** from the repository root:

   ```sh
   bun run tauri build --no-bundle
   ```

   This produces `src-tauri/target/release/mytube`.

2. **Install the binary and icon:**

   ```sh
   ./packaging/install.sh
   ```

   It copies:

   - `src-tauri/target/release/mytube` → `~/.local/bin/mytube` (mode 755)
   - `src-tauri/icons/128x128.png` → `~/.local/share/icons/hicolor/128x128/apps/mytube.png` (mode 644)

   and refreshes the icon cache if `gtk-update-icon-cache` is available. The
   script can be run from any directory — it resolves its own location. If the
   release binary is missing it stops and tells you to build first.

3. **Install the launcher entry** yourself (the script deliberately does not
   touch your applications directory):

   ```sh
   cp packaging/mytube.desktop ~/.local/share/applications/
   ```

4. **If MyTube does not show up in your application menu,** refresh the desktop
   database:

   ```sh
   update-desktop-database ~/.local/share/applications
   ```

   Some desktop environments also need a logout/login or a shell restart before
   a new entry appears.

## PATH

`~/.local/bin` must be on your `PATH` if you want to launch MyTube by typing
`mytube` in a terminal. Add this to `~/.bashrc` / `~/.zshrc` if it is not
already there:

```sh
export PATH="$HOME/.local/bin:$PATH"
```

This is **not** required for the desktop launcher, which resolves an absolute
path of its own:

```ini
Exec=sh -c "exec \\"\\$HOME/.local/bin/mytube\\""
```

`$HOME` is expanded by the shell at launch time, so there is no hardcoded
username, no `sudo`, and no dependence on the session's `PATH`. (`%h` is a
_systemd_ unit specifier, not a Desktop Entry field code — the Desktop Entry
spec only defines `%f %F %u %U %i %c %k`, and GLib/GIO refuses to load an entry
whose `Exec` contains an unrecognised field code, which makes the launcher
vanish from the menu entirely. Hence the `sh -c` form.)

## Runtime requirements

These are not bundled, and most of them no longer need installing by hand.

| Requirement    | Why                                                                                                                                                                                   |
| -------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `yt-dlp`       | Fetches video metadata and performs all downloads. MyTube downloads and updates its own copy in `~/.local/share/mytube/bin` and prefers it over a system one, so YouTube fixes arrive. |
| `ffmpeg`       | Required by `yt-dlp` to merge the separate video and audio streams into the `.mkv` output. A system copy is used if present; otherwise MyTube downloads one.                           |
| `deno`         | The JavaScript runtime `yt-dlp` needs for YouTube. A system copy (2.3 or newer) is used if present; otherwise MyTube downloads one.                                                    |
| YouTube cookies | Settings → YouTube cookies. The default, Automatic, reads Firefox's cookies when a Firefox profile exists; a `cookies.txt` file works too.                                          |
| A video player | Used to play downloaded files. Settings → Player lists the ones installed (mpv, SMPlayer, VLC, Celluloid, Haruna, flatpak exports…), or "System default" for your desktop's choice. |

See [the top-level README](../README.md#first-run) for where the managed tools
come from and how to override them.

## Configuration

MyTube keeps everything under `~/.config/mytube/`:

- `settings.json` — player command, cookie source, download directory, concurrency, and so on
- `mytube.db` — the SQLite database of channels, videos, and watch history
- `thumbs/` — cached channel and video thumbnails

Removing that directory resets the application to a clean state.

## Uninstall

```sh
rm -f ~/.local/bin/mytube
rm -f ~/.local/share/icons/hicolor/128x128/apps/mytube.png
rm -f ~/.local/share/applications/mytube.desktop
update-desktop-database ~/.local/share/applications
```

That removes the app only. Delete `~/.config/mytube/` as well to remove your
settings, library and thumbnails, and `~/.local/share/mytube/` to remove the
tools MyTube downloaded into `~/.local/share/mytube/bin` — roughly 500 MB when
it had to fetch all three, most of it the static `ffmpeg` and `ffprobe`.
