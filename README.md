# MyTube

**Your feed. Your files.** MyTube follows the channels you pick, downloads what they post and plays it in your own player. No recommendations, and nothing sent back to YouTube.

[![MyTube](.github/readme/website.png)](https://mytube.blyat.uk/)

**Website: [mytube.blyat.uk](https://mytube.blyat.uk/)**

[![Latest release](https://img.shields.io/github/v/release/blyat-uk/mytube?label=release&color=2563eb)](https://github.com/blyat-uk/mytube/releases/latest)
[![CI](https://github.com/blyat-uk/mytube/actions/workflows/ci.yml/badge.svg)](https://github.com/blyat-uk/mytube/actions/workflows/ci.yml)
![Windows · macOS · Linux](https://img.shields.io/badge/platform-Windows%20%C2%B7%20macOS%20%C2%B7%20Linux-555)
[![MIT](https://img.shields.io/github/license/blyat-uk/mytube?color=555)](LICENSE)
[![Docs](https://img.shields.io/badge/docs-wiki-2563eb)](https://github.com/blyat-uk/mytube/wiki)

## Why MyTube

- 📺 **Just the channels you follow.** Newest first. No recommendations, no Shorts, no algorithm.
- ⬇️ **Real files, your player.** Every video lands on your disk at the quality you choose and plays in mpv, VLC or whatever you already use.
- 🔁 **Runs itself.** Checks for new uploads every 30 minutes, and can download a channel's new videos as they arrive.
- ⭐ **Members-only videos too.** Mark the channels you have joined and their members-only uploads arrive with the rest.
- 🧩 **Series on one card.** Part 1, Part 2, Part 3 collapse into one card, and *Continue watching* shows the ones you are partway through.
- 🔒 **Private sync.** Your library on all your computers, end-to-end encrypted, no account.
- 🧰 **Zero setup.** yt-dlp, ffmpeg and deno are fetched, checked and kept up to date for you.

## 🔒 New: sync your computers, privately

> [!TIP]
> Your subscriptions, memberships and every video you watched, hid or linked, the same on every computer. **Settings → Sync → Turn on sync.**

- **End-to-end encrypted.** Your library is encrypted on your computer, with a key made from your sync code, before it is sent. The server only stores data it cannot read.
- **No account.** No email, no password. One sync code (`MYT1-…`) pairs up to five computers.
- **Off until you turn it on.** Settings, downloads and thumbnails never leave the computer. The [server is open source](sync-server/), and you can run your own.

On the first computer choose **Turn on sync**; on the others paste the code under **Have a code from another computer?** → **Join**. Treat the code like a password.

**→ [How sync works and what it protects](https://github.com/blyat-uk/mytube/wiki/Cloud-Sync)**

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

## Documentation

Everything lives in the **[wiki](https://github.com/blyat-uk/mytube/wiki)**. New here? Start with the first two.

| Page | Covers |
|---|---|
| 🚀 **[Getting started](https://github.com/blyat-uk/mytube/wiki/Getting-Started)** | Install, first run, adding your channels |
| 💻 **[Platform notes](https://github.com/blyat-uk/mytube/wiki/Platform-Notes)** | First open on Windows and macOS, macOS 27 browser access, Linux AppImage and tray |
| 📺 [Using MyTube](https://github.com/blyat-uk/mytube/wiki/Using-MyTube) | Filters, search, series, quality, auto-download, members-only, shortcuts |
| 🔑 [YouTube sign-in](https://github.com/blyat-uk/mytube/wiki/YouTube-Sign-In) | Browsers, cookies files, members-only access |
| 🔒 [Cloud sync](https://github.com/blyat-uk/mytube/wiki/Cloud-Sync) | Setup, encryption, limits, running your own server |
| 📦 [Backup & transfer](https://github.com/blyat-uk/mytube/wiki/Backup-and-Transfer) | Export and import a library, including over SSH |
| 🧰 [Tools](https://github.com/blyat-uk/mytube/wiki/Tools) | Where yt-dlp, ffmpeg and deno come from, and using your own |
| 🛡️ [Privacy](https://github.com/blyat-uk/mytube/wiki/Privacy) | Every server MyTube talks to, and where your data lives |
| 🩺 [Troubleshooting](https://github.com/blyat-uk/mytube/wiki/Troubleshooting) | Fixes for the usual problems |
| 🗑️ [Uninstalling](https://github.com/blyat-uk/mytube/wiki/Uninstalling) | Removing the app and everything it stored |
| 🛠️ [Building from source](https://github.com/blyat-uk/mytube/wiki/Building-from-Source) | Run, test and package MyTube yourself |

## License

[MIT](LICENSE)
