//! Command-line actions handled before any window exists: `--version`,
//! `--help`, `--self-test <dir>`, which CI runs against every release build,
//! and `export` / `import`, Settings → Backup & transfer for a terminal (see
//! `library`).
//!
//! All of them run before Tauri, GTK or the single-instance plugin are
//! touched, so they work on a headless runner, over SSH with no display, and
//! alongside an already-running MyTube. Any argument list this module does not
//! recognise starts the app normally — a stray argument (a file manager passing
//! a path, macOS's old `-psn_…`) must never turn a launch into an error. Once
//! the first word *is* recognised, though, a mistake after it is a usage error:
//! `mytube import` asked for the terminal, and a window is no answer to a typo.

mod library;
mod terminal;

use std::io::Write;
use std::path::{Path, PathBuf};

/// What the arguments asked for.
#[derive(Debug, PartialEq, Eq)]
enum Action {
    Version,
    /// `--help`, or a subcommand's: the text to print.
    Help(&'static str),
    SelfTest(PathBuf),
    Export(library::ExportArgs),
    Import(library::ImportArgs),
    /// Arguments a CLI action cannot run with: a usage error, not an app launch
    /// — CI asked for a test, or someone typed `mytube import`, and neither
    /// must get a window instead.
    Usage(String),
}

const USAGE: &str = concat!(
    "MyTube ",
    env!("CARGO_PKG_VERSION"),
    " — a desktop YouTube subscription feed and downloader

Usage:
  mytube                    Start the app, or show the window of the one running
  mytube export [PATH]      Write the library to an archive
  mytube import FILE        Read an archive into the library
  mytube --version          Print the version
  mytube --self-test DIR    Download yt-dlp, ffmpeg and deno into DIR/bin and check them

export and import are Settings → Backup & transfer for a terminal, such as an
SSH session to the machine MyTube runs on (Linux and macOS). See
`mytube export --help` and `mytube import --help`."
);

/// Windows' release build is a GUI-subsystem program, which cmd and PowerShell
/// do not wait for: a question would be answered by whatever the shell read
/// next. Checked at run time rather than compiled out, so the parsing and the
/// questions stay one code path on every OS.
const NOT_ON_WINDOWS: &str = "mytube export and mytube import are not available on Windows. \
Use Settings → Backup & transfer in the app instead.";

const SELF_TEST_FILE: &str = "self-test.txt";

/// `args` are the arguments *after* the program name. `Some(exit code)` when
/// they asked for a CLI action (the caller exits with it); `None` to start the
/// app normally.
pub fn run(args: &[String]) -> Option<i32> {
    let action = parse(args)?;
    attach_parent_console();
    Some(match action {
        Action::Version => {
            say(&format!("mytube {}", env!("CARGO_PKG_VERSION")));
            0
        }
        Action::Help(text) => {
            say(text);
            0
        }
        Action::SelfTest(dir) => self_test(&dir),
        Action::Export(_) | Action::Import(_) if cfg!(windows) => {
            warn(NOT_ON_WINDOWS);
            2
        }
        Action::Export(args) => library::export(args),
        Action::Import(args) => library::import(args),
        Action::Usage(msg) => {
            warn(&msg);
            2
        }
    })
}

fn parse(args: &[String]) -> Option<Action> {
    match args {
        [flag] if flag == "--version" || flag == "-V" => Some(Action::Version),
        [flag] if flag == "--help" || flag == "-h" => Some(Action::Help(USAGE)),
        [cmd, rest @ ..] if cmd == "export" => Some(library::parse_export(rest)),
        [cmd, rest @ ..] if cmd == "import" => Some(library::parse_import(rest)),
        [flag, dir] if flag == "--self-test" && !dir.is_empty() => {
            Some(Action::SelfTest(PathBuf::from(dir)))
        }
        [flag] if flag.strip_prefix("--self-test=").is_some_and(|d| !d.is_empty()) => Some(
            Action::SelfTest(PathBuf::from(&flag["--self-test=".len()..])),
        ),
        [flag, ..] if flag == "--self-test" || flag.starts_with("--self-test=") => Some(
            Action::Usage("usage: mytube --self-test <dir>".to_string()),
        ),
        _ => None,
    }
}

/// Provisions the three tools into `dir` and reports. The report — or the
/// error — goes to stdout *and* `<dir>/self-test.txt`: a Windows release build
/// is a GUI-subsystem program, and a runner that starts it may see no stdout at
/// all, so the file is what CI prints.
fn self_test(dir: &Path) -> i32 {
    finish(dir, run_self_test(dir))
}

/// Writes the outcome to stdout and `<dir>/self-test.txt`; the exit code is 1
/// for a failed test *or* a report that could not be written, since CI reads
/// the file and a missing one would read as nothing at all.
fn finish(dir: &Path, outcome: Result<String, String>) -> i32 {
    let (report, code) = match outcome {
        Ok(report) => (format!("self-test passed\n{report}"), 0),
        Err(why) => (format!("self-test FAILED: {why}"), 1),
    };
    say(&report);
    let file = dir.join(SELF_TEST_FILE);
    let written = std::fs::create_dir_all(dir)
        .and_then(|()| std::fs::write(&file, format!("{report}\n")));
    if let Err(e) = written {
        say(&format!("could not write {}: {e}", file.display()));
        return 1;
    }
    code
}

fn run_self_test(dir: &Path) -> Result<String, String> {
    // Current-thread: the self-test is one sequential job, and it needs no
    // Tauri runtime — `Tools` emits nothing until `set_app`, which is never
    // called here.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("could not start the async runtime: {e}"))?;
    // A panic inside the test would otherwise print only to stderr, which a
    // Windows GUI build does not have; caught, it lands in self-test.txt.
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rt.block_on(crate::tools::self_test(dir))
    }));
    match outcome {
        Ok(Ok(report)) => Ok(report),
        Ok(Err(e)) => Err(format!("{e:#}")),
        Err(panic) => Err(format!("panicked: {}", panic_text(&panic))),
    }
}

fn panic_text(panic: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = panic.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = panic.downcast_ref::<String>() {
        s.clone()
    } else {
        "(no message)".to_string()
    }
}

/// Prints a line, ignoring a closed or missing stdout: `println!` would panic
/// on one, and a CLI action must reach its exit code regardless.
fn say(line: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

/// [`say`] on stderr: a question, a warning or an error, which a script that
/// keeps stdout must not find mixed into the report.
fn warn(line: &str) {
    let mut err = std::io::stderr().lock();
    let _ = writeln!(err, "{line}");
    let _ = err.flush();
}

/// The release build on Windows is `windows_subsystem = "windows"`, so a
/// terminal that starts it gives it no console and `--version` would print
/// into nothing. Attaching to the parent's console fixes that — but only when
/// stdout is not already a handle: a redirect (`> out.txt`, or CI's
/// `Start-Process -RedirectStandardOutput`) hands a GUI program a real handle,
/// and attaching must not trade it for the console. cmd.exe does not wait for
/// a GUI program, so the line can land after the next prompt; that is cosmetic.
#[cfg(windows)]
fn attach_parent_console() {
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Console::{
        AttachConsole, GetStdHandle, ATTACH_PARENT_PROCESS, STD_OUTPUT_HANDLE,
    };
    // SAFETY: both calls take plain values and only read process state; a
    // failure (no parent console) is reported by the return value, ignored
    // because there is then nowhere to print to anyway.
    unsafe {
        let out = GetStdHandle(STD_OUTPUT_HANDLE);
        if out.is_null() || out == INVALID_HANDLE_VALUE {
            AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
}

#[cfg(not(windows))]
fn attach_parent_console() {}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn no_arguments_start_the_app() {
        assert_eq!(parse(&[]), None);
        assert_eq!(run(&[]), None);
    }

    #[test]
    fn version_is_a_cli_action_that_exits_zero() {
        assert_eq!(parse(&args(&["--version"])), Some(Action::Version));
        assert_eq!(parse(&args(&["-V"])), Some(Action::Version));
        assert_eq!(run(&args(&["--version"])), Some(0));
    }

    #[test]
    fn self_test_takes_its_directory_as_the_next_argument_or_after_an_equals_sign() {
        assert_eq!(
            parse(&args(&["--self-test", "/tmp/tools"])),
            Some(Action::SelfTest(PathBuf::from("/tmp/tools")))
        );
        assert_eq!(
            parse(&args(&["--self-test=C:\\runner temp\\tools"])),
            Some(Action::SelfTest(PathBuf::from("C:\\runner temp\\tools")))
        );
    }

    #[test]
    fn self_test_without_a_directory_is_a_usage_error_not_a_window() {
        for bad in [&["--self-test"][..], &["--self-test="], &["--self-test", ""]] {
            assert!(
                matches!(parse(&args(bad)), Some(Action::Usage(_))),
                "{bad:?} should be a usage error"
            );
        }
        assert_eq!(run(&args(&["--self-test"])), Some(2));
        // Extra arguments after the directory are not silently dropped.
        assert!(matches!(
            parse(&args(&["--self-test", "a", "b"])),
            Some(Action::Usage(_))
        ));
    }

    #[test]
    fn help_is_printed_rather_than_opening_a_window() {
        // Over SSH a window is a GTK error, which is no answer to "--help".
        assert_eq!(parse(&args(&["--help"])), Some(Action::Help(USAGE)));
        assert_eq!(parse(&args(&["-h"])), Some(Action::Help(USAGE)));
        assert_eq!(run(&args(&["--help"])), Some(0));
        for line in ["mytube export", "mytube import", "mytube --version", "mytube --self-test"] {
            assert!(USAGE.contains(line), "usage mentions {line}");
        }
    }

    #[test]
    fn export_and_import_are_cli_actions_whatever_follows_them() {
        assert!(matches!(parse(&args(&["export"])), Some(Action::Export(_))));
        assert!(matches!(parse(&args(&["import", "a.zip"])), Some(Action::Import(_))));
        // A mistake after the subcommand is a usage error, never a window.
        assert!(matches!(parse(&args(&["import"])), Some(Action::Usage(_))));
        assert_eq!(run(&args(&["import"])), Some(2));
        assert_eq!(run(&args(&["export", "--help"])), Some(0));
    }

    #[cfg(windows)]
    #[test]
    fn export_and_import_are_refused_on_windows() {
        assert_eq!(run(&args(&["export"])), Some(2));
        assert_eq!(run(&args(&["import", "a.zip"])), Some(2));
    }

    #[test]
    fn anything_unrecognised_starts_the_app_normally() {
        for other in [
            &["/home/me/Videos/clip.mkv"][..],
            &["-psn_0_12345"],
            &["--version", "extra"],
            &["--versions"],
            &["--help", "extra"],
            &["exports"],
        ] {
            assert_eq!(parse(&args(other)), None, "{other:?}");
        }
    }

    #[test]
    fn the_report_lands_in_the_directory_with_the_exit_code_matching_the_outcome() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("tools");
        assert_eq!(finish(&dir, Ok("yt-dlp 2026.09.20".into())), 0);
        let text = std::fs::read_to_string(dir.join(SELF_TEST_FILE)).unwrap();
        assert!(text.starts_with("self-test passed\n"), "{text}");
        assert!(text.contains("yt-dlp 2026.09.20"), "{text}");

        assert_eq!(finish(&dir, Err("deno: checksum mismatch".into())), 1);
        let text = std::fs::read_to_string(dir.join(SELF_TEST_FILE)).unwrap();
        assert!(text.contains("FAILED: deno: checksum mismatch"), "{text}");
    }

    #[test]
    fn a_passing_self_test_whose_report_cannot_be_written_still_fails() {
        // A regular file where the directory should be.
        let tmp = tempfile::tempdir().unwrap();
        let not_a_dir = tmp.path().join("not-a-dir");
        std::fs::write(&not_a_dir, "").unwrap();
        assert_eq!(finish(&not_a_dir, Ok("fine".into())), 1);
    }
}
