# Packaging MyTube for Linux

MyTube installs entirely into your home directory. Nothing here needs `sudo`.

## Install

1. **Build the release binary** from the repository root:

   ```sh
   bun run tauri build
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
*systemd* unit specifier, not a Desktop Entry field code — the Desktop Entry
spec only defines `%f %F %u %U %i %c %k`, and GLib/GIO refuses to load an entry
whose `Exec` contains an unrecognised field code, which makes the launcher
vanish from the menu entirely. Hence the `sh -c` form.)

## Runtime requirements

These are not bundled; install them with your distribution's package manager.

| Requirement | Why |
| --- | --- |
| `yt-dlp` | Fetches video metadata and performs all downloads. Keep it current — YouTube changes break old versions. |
| `ffmpeg` | Required by `yt-dlp` to merge the separate video and audio streams into the `.mkv` output and to embed thumbnails. |
| Firefox | Downloads run with `--cookies-from-browser firefox`, so a Firefox profile with your YouTube cookies must exist on the machine. |
| A video player | Used to play downloaded files. The default is `smplayer`; change it in Settings (`player_command`) if you prefer `mpv`, `vlc`, or anything else. |

## Configuration

MyTube keeps everything under `~/.config/mytube/`:

- `settings.json` — player command, download directory, concurrency, and so on
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

Delete `~/.config/mytube/` as well to remove your data.
