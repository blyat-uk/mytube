//! Command-line flags handled before any window exists: `--version` and
//! `--self-test <dir>`, which CI runs against every release build.
//!
//! FOUNDATION STUB: the CI task implements this.

/// `Some(exit code)` when the arguments asked for a CLI action (the caller
/// exits with it); `None` to start the app normally.
pub fn run(args: &[String]) -> Option<i32> {
    let _ = args;
    None
}
