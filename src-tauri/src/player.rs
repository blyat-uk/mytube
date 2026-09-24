use crate::detect::{macos_app_dirs, Env, Os, RealEnv};
use anyhow::{anyhow, Result};
use std::path::Path;
use std::process::{Command, Stdio};

/// Splits a configured player command so that both `smplayer` and
/// `mpv --fullscreen --no-terminal` work, and quoted paths survive: shell-style
/// on unix, and by the rules in [`split_windows`] on Windows.
pub fn split_command(cmd: &str) -> Result<(String, Vec<String>)> {
    split_for(Os::current(), cmd, &RealEnv)
}

pub(crate) fn split_for(os: Os, cmd: &str, env: &dyn Env) -> Result<(String, Vec<String>)> {
    let parts = match os {
        Os::Windows => split_windows(cmd, env)?,
        _ => shell_words::split(cmd.trim()).map_err(|e| anyhow!("Player command is not valid: {e}"))?,
    };
    let mut it = parts.into_iter();
    let bin = it.next().ok_or_else(|| anyhow!("Player command is empty"))?;
    Ok((bin, it.collect()))
}

/// Windows command lines, where shell-words would be wrong twice over: it
/// eats `\` as an escape, so `C:\Program Files\...` comes out as
/// `C:Program Files...`, and people paste paths with spaces unquoted.
///
/// - The whole string naming an existing file is the program, spaces and all.
/// - Otherwise, an unquoted string whose longest whitespace-delimited prefix
///   names an existing file has that prefix as the program and the rest as its
///   arguments: `C:\Program Files\VideoLAN\VLC\vlc.exe --fullscreen` starts
///   VLC with `--fullscreen`, where splitting on spaces would try to run
///   `C:\Program`. Longest first, so a stray `C:\Program` file cannot win over
///   the player. A string opening with a quote has said where its program
///   ends, and is only ever split.
/// - `\` is always literal.
/// - `"…"` groups anywhere in a token, as `CommandLineToArgvW` does.
/// - `'…'` groups too, but only when it opens a token: a Windows path can hold
///   an apostrophe (`C:\Users\O'Brien\…`), and mid-token it stays literal.
fn split_windows(cmd: &str, env: &dyn Env) -> Result<Vec<String>> {
    let cmd = cmd.trim();
    if cmd.is_empty() {
        return Ok(Vec::new());
    }
    if env.is_file(cmd) {
        return Ok(vec![cmd.to_string()]);
    }
    if !cmd.starts_with(['"', '\'']) {
        let ends = cmd
            .char_indices()
            .filter(|&(i, c)| c.is_whitespace() && !cmd[..i].ends_with(char::is_whitespace))
            .map(|(i, _)| i);
        for end in ends.collect::<Vec<_>>().into_iter().rev() {
            let program = &cmd[..end];
            if env.is_file(program) {
                let mut parts = vec![program.to_string()];
                parts.extend(split_windows_tokens(&cmd[end..])?);
                return Ok(parts);
            }
        }
    }
    split_windows_tokens(cmd)
}

/// The tokenising half of [`split_windows`]: quotes and whitespace only.
fn split_windows_tokens(cmd: &str) -> Result<Vec<String>> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    // A token has started even if it is still empty: `""` is an empty argument.
    let mut in_token = false;
    let mut quote: Option<char> = None;
    for c in cmd.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => cur.push(c),
            None if c == '"' || (c == '\'' && !in_token) => {
                quote = Some(c);
                in_token = true;
            }
            None if c.is_whitespace() => {
                if in_token {
                    parts.push(std::mem::take(&mut cur));
                    in_token = false;
                }
            }
            None => {
                cur.push(c);
                in_token = true;
            }
        }
    }
    if let Some(q) = quote {
        return Err(anyhow!("Player command is not valid: missing closing {q}"));
    }
    if in_token {
        parts.push(cur);
    }
    Ok(parts)
}

/// Whether `cmd` would start something on this machine: `""` (the OS default
/// app) always does; otherwise its program must resolve. Used to decide
/// whether an imported archive's `player_command` means anything here.
pub fn command_resolves(cmd: &str) -> bool {
    resolves_for(Os::current(), cmd, &RealEnv)
}

pub(crate) fn resolves_for(os: Os, cmd: &str, env: &dyn Env) -> bool {
    if cmd.trim().is_empty() {
        return true;
    }
    let Ok((bin, args)) = split_for(os, cmd, env) else { return false };
    // `open -a IINA` is a macOS command whatever machine reads it: `open`
    // itself resolves on many Linux boxes (openvt, or an xdg-open alias), so
    // the question that matters is whether the app exists. Off macOS it never
    // does, which is the answer an imported Mac setting should get.
    if bin == "open" || bin == "/usr/bin/open" {
        if let Some(i) = args.iter().position(|a| a == "-a") {
            return args.get(i + 1).is_some_and(|app| macos_app_exists(env, app));
        }
    }
    program_resolves(os, &bin, env)
}

fn program_resolves(os: Os, bin: &str, env: &dyn Env) -> bool {
    let is_path = bin.contains('/') || (os == Os::Windows && (bin.contains('\\') || bin.contains(':')));
    if is_path {
        env.is_file(bin)
    } else {
        env.which(bin).is_some()
    }
}

/// `open -a` takes an app name (`IINA`, `IINA.app`) or a path to the bundle.
fn macos_app_exists(env: &dyn Env, app: &str) -> bool {
    if app.contains('/') {
        return env.is_dir(app);
    }
    let bundle = if app.ends_with(".app") { app.to_string() } else { format!("{app}.app") };
    macos_app_dirs(env).iter().any(|d| env.is_dir(&format!("{d}/{bundle}")))
}

/// Spawns the player detached: stdio is nulled and the child is never awaited,
/// so it keeps running after MyTube exits. An empty command hands the file to
/// whatever the OS opens it with. This is the one process MyTube starts
/// without `proc::command`: it is a foreground app the user asked for, not a
/// background tool, so it should get a window if it wants one.
pub fn launch(player_command: &str, file_path: &str) -> Result<()> {
    if !Path::new(file_path).exists() {
        return Err(anyhow!("File no longer exists: {file_path}"));
    }
    if player_command.trim().is_empty() {
        return tauri_plugin_opener::open_path(file_path, None::<&str>)
            .map_err(|e| anyhow!("Could not open the file with the system default app: {e}"));
    }
    let (bin, args) = split_command(player_command)?;
    Command::new(&bin)
        .args(&args)
        .arg(file_path)
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
        .spawn()
        .map_err(|e| anyhow!("Could not start player '{bin}': {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::fake::FakeEnv;
    use crate::detect::players_for;

    #[test]
    fn a_bare_command_has_no_arguments() {
        let (bin, args) = split_command("smplayer").unwrap();
        assert_eq!(bin, "smplayer");
        assert!(args.is_empty());
    }

    #[test]
    fn flags_and_quoted_paths_are_split_shell_style() {
        let (bin, args) = split_command("mpv --fullscreen --no-terminal").unwrap();
        assert_eq!(bin, "mpv");
        assert_eq!(args, vec!["--fullscreen", "--no-terminal"]);

        let (bin, args) = split_command(r#""/opt/my player/vlc" --one"#).unwrap();
        assert_eq!(bin, "/opt/my player/vlc");
        assert_eq!(args, vec!["--one"]);
    }

    #[test]
    fn an_empty_or_unbalanced_command_is_rejected() {
        assert!(split_command("").is_err());
        assert!(split_command("   ").is_err());
        assert!(split_command(r#"mpv "unclosed"#).is_err());
    }

    #[test]
    fn launching_a_missing_file_fails_before_spawning() {
        assert!(launch("smplayer", "/definitely/not/here.mkv").is_err());
        // The system default path checks too, rather than asking the OS to
        // open nothing.
        assert!(launch("", "/definitely/not/here.mkv").is_err());
    }

    #[test]
    fn launching_a_missing_binary_reports_the_binary_name() {
        let f = std::env::temp_dir().join("mytube-player-test.mkv");
        std::fs::write(&f, b"x").unwrap();
        let err = launch("mytube-no-such-player-xyz", f.to_str().unwrap()).unwrap_err().to_string();
        assert!(err.contains("mytube-no-such-player-xyz"), "got: {err}");
        std::fs::remove_file(&f).ok();
    }

    // -- Windows splitting, exercised on any host through a fake filesystem

    const VLC: &str = r"C:\Program Files\VideoLAN\VLC\vlc.exe";

    fn win() -> FakeEnv {
        FakeEnv::new(Os::Windows, r"C:\Users\u").var("ProgramFiles", r"C:\Program Files")
    }
    fn wsplit(cmd: &str, env: &FakeEnv) -> (String, Vec<String>) {
        split_for(Os::Windows, cmd, env).unwrap()
    }

    #[test]
    fn windows_an_unquoted_path_to_an_existing_file_is_the_whole_program() {
        let env = win().file(VLC);
        assert_eq!(wsplit(VLC, &env), (VLC.to_string(), vec![]));
        assert_eq!(wsplit(&format!("  {VLC}  "), &env), (VLC.to_string(), vec![]));
    }

    #[test]
    fn windows_an_unquoted_path_to_an_existing_file_can_take_arguments() {
        let env = win().file(VLC);
        assert_eq!(
            wsplit(&format!("{VLC} --fullscreen"), &env),
            (VLC.to_string(), vec!["--fullscreen".to_string()])
        );
        let (bin, args) = wsplit(&format!("{VLC}   --fullscreen --meta-title=\"my film\" x\\y"), &env);
        assert_eq!(bin, VLC);
        assert_eq!(args, vec!["--fullscreen", "--meta-title=my film", r"x\y"]);
        assert!(resolves_for(Os::Windows, &format!("{VLC} --fullscreen"), &env));
    }

    #[test]
    fn windows_the_longest_existing_prefix_is_the_program() {
        // A stray `C:\Program` file is exactly what makes unquoted paths with
        // spaces dangerous; it must not be picked over the real player.
        let env = win().file(r"C:\Program").file(VLC);
        assert_eq!(wsplit(&format!("{VLC} --fs"), &env), (VLC.to_string(), vec!["--fs".to_string()]));
        // With no player there, `C:\Program` is all that exists.
        let env = win().file(r"C:\Program");
        let (bin, args) = wsplit(&format!("{VLC} --fs"), &env);
        assert_eq!(bin, r"C:\Program");
        assert_eq!(args, vec![r"Files\VideoLAN\VLC\vlc.exe", "--fs"]);
    }

    #[test]
    fn windows_with_no_existing_prefix_it_is_plain_splitting() {
        let (bin, args) = wsplit(&format!("{VLC} --fullscreen"), &win());
        assert_eq!(bin, r"C:\Program");
        assert_eq!(args, vec![r"Files\VideoLAN\VLC\vlc.exe", "--fullscreen"]);
        assert!(!resolves_for(Os::Windows, &format!("{VLC} --fullscreen"), &win()));
    }

    #[test]
    fn windows_a_quoted_command_is_never_prefix_matched() {
        // The quotes say where the program ends, even if a longer file exists.
        let env = win().file(r"C:\mpv.exe --fs");
        let (bin, args) = wsplit(r#""C:\mpv.exe" --fs --x"#, &env);
        assert_eq!(bin, r"C:\mpv.exe");
        assert_eq!(args, vec!["--fs", "--x"]);
    }

    #[test]
    fn windows_double_quotes_group_and_backslashes_stay_literal() {
        let (bin, args) = wsplit(r#""C:\Program Files\mpv\mpv.exe" --fs"#, &win());
        assert_eq!(bin, r"C:\Program Files\mpv\mpv.exe");
        assert_eq!(args, vec!["--fs"]);

        let (bin, args) = wsplit(r#"C:\tools\mpv.exe --title="my film" --x=a\b"#, &win());
        assert_eq!(bin, r"C:\tools\mpv.exe");
        assert_eq!(args, vec!["--title=my film", r"--x=a\b"]);
    }

    #[test]
    fn windows_single_quotes_group_only_at_the_start_of_a_token() {
        let (bin, args) = wsplit(r"'C:\Program Files\VLC\vlc.exe' --one", &win());
        assert_eq!(bin, r"C:\Program Files\VLC\vlc.exe");
        assert_eq!(args, vec!["--one"]);

        let (bin, args) = wsplit(r"C:\Users\O'Brien\mpv.exe --fs", &win());
        assert_eq!(bin, r"C:\Users\O'Brien\mpv.exe");
        assert_eq!(args, vec!["--fs"]);

        let (bin, _) = wsplit(r#""C:\Users\O'Brien\mpv.exe""#, &win());
        assert_eq!(bin, r"C:\Users\O'Brien\mpv.exe");
    }

    #[test]
    fn windows_empty_quotes_are_an_empty_argument() {
        let (_, args) = wsplit(r#"mpv.exe "" --fs"#, &win());
        assert_eq!(args, vec!["", "--fs"]);
    }

    #[test]
    fn windows_an_empty_or_unbalanced_command_is_rejected() {
        assert!(split_for(Os::Windows, "  ", &win()).is_err());
        assert!(split_for(Os::Windows, r#""C:\Program Files\vlc.exe --fs"#, &win()).is_err());
    }

    #[test]
    fn windows_detection_output_round_trips_through_split() {
        // The files exist for detection and are then hidden from the splitter,
        // so the quoting alone has to carry the path through.
        let files = [
            VLC,
            r"C:\Program Files\MPC-BE x64\mpc-be64.exe",
            r"C:\Program Files (x86)\DAUM\PotPlayer\PotPlayerMini.exe",
            r"C:\Users\u\scoop\apps\mpv\current\mpv.exe",
        ];
        let mut env = win().var("ProgramFiles(x86)", r"C:\Program Files (x86)");
        for f in files {
            env = env.file(f);
        }
        let players = players_for(Os::Windows, &env);
        let bare = win();
        for f in files {
            let p = players.iter().find(|p| p.command.contains(f)).unwrap_or_else(|| panic!("{f}"));
            assert_eq!(split_for(Os::Windows, &p.command, &bare).unwrap(), (f.to_string(), vec![]));
            assert!(resolves_for(Os::Windows, &p.command, &env), "{}", p.command);
        }
    }

    #[test]
    fn unix_detection_output_round_trips_through_split() {
        let env = FakeEnv::new(Os::MacOs, "/Users/u")
            .dir("/Applications/IINA.app")
            .file("/opt/homebrew/bin/mpv");
        for p in players_for(Os::MacOs, &env).into_iter().skip(1) {
            assert!(split_for(Os::MacOs, &p.command, &env).is_ok(), "{}", p.command);
            assert!(resolves_for(Os::MacOs, &p.command, &env), "{}", p.command);
        }
    }

    // -- command_resolves

    #[test]
    fn the_system_default_always_resolves() {
        assert!(resolves_for(Os::Linux, "", &FakeEnv::new(Os::Linux, "/home/u")));
        assert!(resolves_for(Os::Windows, "  ", &win()));
        assert!(command_resolves(""));
    }

    #[test]
    fn a_bare_program_resolves_through_path_and_a_path_through_the_filesystem() {
        let env = FakeEnv::new(Os::Linux, "/home/u")
            .on_path("mpv", "/usr/bin/mpv")
            .file("/opt/vlc/vlc");
        assert!(resolves_for(Os::Linux, "mpv --fs", &env));
        assert!(!resolves_for(Os::Linux, "smplayer", &env));
        assert!(resolves_for(Os::Linux, "/opt/vlc/vlc --one", &env));
        assert!(!resolves_for(Os::Linux, "/opt/mpv/mpv", &env));
        assert!(!resolves_for(Os::Linux, r#"mpv "unclosed"#, &env));
    }

    #[test]
    fn a_windows_path_resolves_only_where_that_file_exists() {
        let cmd = format!("\"{VLC}\" --fullscreen");
        assert!(resolves_for(Os::Windows, &cmd, &win().file(VLC)));
        assert!(!resolves_for(Os::Windows, &cmd, &win()));
        // The same Windows setting imported onto Linux.
        assert!(!resolves_for(Os::Linux, &cmd, &FakeEnv::new(Os::Linux, "/home/u")));
    }

    #[test]
    fn open_a_resolves_only_where_the_app_exists() {
        let mac = FakeEnv::new(Os::MacOs, "/Users/u")
            .on_path("open", "/usr/bin/open")
            .dir("/Users/u/Applications/IINA.app")
            .dir("/Applications/VLC.app");
        assert!(resolves_for(Os::MacOs, "open -a IINA", &mac));
        assert!(resolves_for(Os::MacOs, "open -a VLC.app", &mac));
        assert!(resolves_for(Os::MacOs, "open -a /Applications/VLC.app", &mac));
        assert!(!resolves_for(Os::MacOs, "open -a mpv", &mac));
        assert!(!resolves_for(Os::MacOs, "open -a", &mac));
        // A Linux box where `open` is openvt must not accept a Mac setting.
        let linux = FakeEnv::new(Os::Linux, "/home/u").on_path("open", "/usr/bin/open");
        assert!(!resolves_for(Os::Linux, "open -a IINA", &linux));
    }

    #[cfg(unix)]
    #[test]
    fn a_real_program_on_this_machine_resolves() {
        assert!(command_resolves("sh -c true"));
        assert!(!command_resolves("mytube-no-such-player-xyz --fs"));
    }
}
