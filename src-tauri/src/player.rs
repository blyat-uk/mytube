use anyhow::{anyhow, Result};
use std::path::Path;
use std::process::{Command, Stdio};

/// Splits a configured player command shell-style so that both `smplayer` and
/// `mpv --fullscreen --no-terminal` work, and quoted paths survive.
pub fn split_command(cmd: &str) -> Result<(String, Vec<String>)> {
    let parts = shell_words::split(cmd.trim())
        .map_err(|e| anyhow!("Player command is not valid: {e}"))?;
    let mut it = parts.into_iter();
    let bin = it.next().ok_or_else(|| anyhow!("Player command is empty"))?;
    Ok((bin, it.collect()))
}

/// Whether `cmd` would start something on this machine: `""` (the OS default
/// app) always does; otherwise its program must resolve. Used to decide
/// whether an imported archive's `player_command` means anything here.
///
/// FOUNDATION STUB: the detect task implements this.
pub fn command_resolves(cmd: &str) -> bool {
    let _ = cmd;
    todo!("detect task")
}

/// Spawns the player detached: stdio is nulled and the child is never awaited,
/// so it keeps running after MyTube exits.
pub fn launch(player_command: &str, file_path: &str) -> Result<()> {
    if !Path::new(file_path).exists() {
        return Err(anyhow!("File no longer exists: {file_path}"));
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
    }

    #[test]
    fn launching_a_missing_binary_reports_the_binary_name() {
        let f = std::env::temp_dir().join("mytube-player-test.mkv");
        std::fs::write(&f, b"x").unwrap();
        let err = launch("mytube-no-such-player-xyz", f.to_str().unwrap()).unwrap_err().to_string();
        assert!(err.contains("mytube-no-such-player-xyz"), "got: {err}");
        std::fs::remove_file(&f).ok();
    }
}
