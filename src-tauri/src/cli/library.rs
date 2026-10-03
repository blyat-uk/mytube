//! `mytube export` and `mytube import`: Settings → Backup & transfer, for a
//! machine you can only reach over SSH.
//!
//! The same archive, the same rules and the same code underneath —
//! `library_transfer` is shared with `commands.rs`, so a terminal import and a
//! dialog import cannot disagree about what a Replace removes. What is here is
//! only the terminal's half: the arguments, the questions and the words.
//!
//! Every choice has a flag, and a question is asked only for a choice the flags
//! left open, and only when there is a terminal to ask on. Without one — cron,
//! `ssh host mytube export`, a pipe — each open choice takes the dialog's own
//! default: thumbnails in, Merge, every channel, settings applied. Replace is
//! the exception, because it is the one choice that removes rows: unattended it
//! needs `--yes` as well as `--replace`, the terminal's version of the dialog
//! asking twice.
//!
//! Neither command does anything the running app has to be told about first.
//! The database is SQLite in WAL mode with a busy timeout, and opening it runs
//! nothing but the migrations — in particular not `reset_stale_downloads`,
//! which under a running MyTube would wipe the state of its live downloads. An
//! import lands in one transaction; a running window shows it after its next
//! poll, which is when it refetches anyway.

use super::terminal::{ProgressLine, Terminal};
use super::{say, Action};
use crate::config;
use crate::db::Db;
use crate::library_transfer;
use crate::models::{ArchiveChannel, ArchiveSummary, ImportMode, ImportReport, TransferEstimate};
use crate::transfer;
use std::path::{Path, PathBuf};

#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct ExportArgs {
    /// As typed, so a trailing separator can still say "a directory".
    pub target: Option<String>,
    pub thumbs: Option<bool>,
    pub yes: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct ImportArgs {
    pub file: PathBuf,
    pub mode: Option<ImportMode>,
    pub settings: Option<bool>,
    pub yes: bool,
}

pub(super) const EXPORT_HELP: &str = "\
Usage: mytube export [PATH] [--thumbs | --no-thumbs] [-y]

Writes this machine's library to an archive that `mytube import`, or Settings
→ Backup & transfer → Import… on another machine, reads back: channels, every
video's watched and hidden state, hand-marked series, and the settings worth
sharing. Downloaded videos never travel; one the other machine can already see
under its own download folder is found again there.

  PATH          Where to write. A directory, or a path ending in /, gets
                mytube-export-YYYY-MM-DD.zip inside it. The default is that
                name in the current directory.
  --thumbs      Include the cached thumbnails, so the other machine need not
                fetch them again. The default.
  --no-thumbs   Leave them out, for a much smaller file.
  -y, --yes     Overwrite an existing archive without asking.

In a terminal, you are asked about whatever the flags leave open. Without one
(cron, `ssh host mytube export`), thumbnails are included and an existing
archive is overwritten. The archive is written beside its target and renamed
into place, so an export cut off part way leaves the previous one as it was.

Exit status: 0 done, 1 failed, 2 bad arguments, 130 cancelled.";

pub(super) const IMPORT_HELP: &str = "\
Usage: mytube import FILE [--merge | --replace] [--settings | --no-settings] [-y]

Reads an archive written by `mytube export`, or by Settings → Backup &
transfer → Export…, into this machine's library.

  --merge         Add what the archive holds to what is here. Nothing here is
                  lost. The default.
  --replace       Make the imported channels match the archive exactly, and
                  remove the channels it does not mention, with their videos.
                  Database rows only: no file is ever deleted from disk.
  --settings      Also apply the archive's settings. The default. Its download
                  folder is taken only if it exists here.
  --no-settings   Keep this machine's settings.
  -y, --yes       Do not ask for confirmation.

In a terminal, you are shown what the archive holds, asked about whatever the
flags leave open (including which channels to import), and asked to confirm.
Without one, every channel is imported, and --replace needs --yes as well.
If MyTube is running on this machine, its window shows the import after its
next poll.

Exit status: 0 done, 1 failed, 2 bad arguments, 130 cancelled.";

/// Flags and positionals, apart. Everything after `--` is a positional, so a
/// path that starts with a dash can still be named.
fn split(args: &[String]) -> (Vec<&str>, Vec<&str>) {
    let (mut flags, mut words) = (Vec::new(), Vec::new());
    let mut only_words = false;
    for a in args {
        if only_words {
            words.push(a.as_str());
        } else if a == "--" {
            only_words = true;
        } else if a.starts_with('-') {
            flags.push(a.as_str());
        } else {
            words.push(a.as_str());
        }
    }
    (flags, words)
}

/// A pair of opposing flags: `Some(true)` for `on`, `Some(false)` for `off`,
/// `None` for neither, and an error for both.
fn either(flags: &[&str], on: &str, off: &str) -> Result<Option<bool>, String> {
    match (flags.contains(&on), flags.contains(&off)) {
        (true, true) => Err(format!("{on} and {off} contradict each other")),
        (true, false) => Ok(Some(true)),
        (false, true) => Ok(Some(false)),
        (false, false) => Ok(None),
    }
}

fn usage(cmd: &str, why: &str) -> Action {
    Action::Usage(usage_text(cmd, why))
}

pub(super) fn usage_text(cmd: &str, why: &str) -> String {
    format!("mytube {cmd}: {why}\nRun `mytube {cmd} --help` for usage.")
}

/// Help first, so `--help` answers even beside a mistake; then any flag this
/// command does not know.
fn check_flags(cmd: &str, flags: &[&str], known: &[&str], help: &'static str) -> Result<(), Action> {
    if flags.iter().any(|f| matches!(*f, "-h" | "--help")) {
        return Err(Action::Help(help));
    }
    match flags.iter().find(|f| !known.contains(f)) {
        Some(bad) => Err(usage(cmd, &format!("unknown option {bad}"))),
        None => Ok(()),
    }
}

pub(super) fn parse_export(args: &[String]) -> Action {
    let (flags, words) = split(args);
    if let Err(stop) = check_flags("export", &flags, &["--thumbs", "--no-thumbs", "-y", "--yes"], EXPORT_HELP) {
        return stop;
    }
    let thumbs = match either(&flags, "--thumbs", "--no-thumbs") {
        Ok(t) => t,
        Err(why) => return usage("export", &why),
    };
    let target = match words.as_slice() {
        [] => None,
        [""] => return usage("export", "the path is empty"),
        [one] => Some(one.to_string()),
        [_, extra, ..] => {
            return usage("export", &format!("unexpected {extra:?}: export writes one archive"))
        }
    };
    Action::Export(ExportArgs { target, thumbs, yes: says_yes(&flags) })
}

pub(super) fn parse_import(args: &[String]) -> Action {
    let known = ["--merge", "--replace", "--settings", "--no-settings", "-y", "--yes"];
    let (flags, words) = split(args);
    if let Err(stop) = check_flags("import", &flags, &known, IMPORT_HELP) {
        return stop;
    }
    let mode = match either(&flags, "--replace", "--merge") {
        Ok(m) => m.map(|replace| if replace { ImportMode::Replace } else { ImportMode::Merge }),
        Err(why) => return usage("import", &why),
    };
    let settings = match either(&flags, "--settings", "--no-settings") {
        Ok(s) => s,
        Err(why) => return usage("import", &why),
    };
    let file = match words.as_slice() {
        [] | [""] => return usage("import", "which archive? Give its path: mytube import FILE"),
        [one] => PathBuf::from(one),
        [_, extra, ..] => {
            return usage("import", &format!("unexpected {extra:?}: import reads one archive"))
        }
    };
    Action::Import(ImportArgs { file, mode, settings, yes: says_yes(&flags) })
}

fn says_yes(flags: &[&str]) -> bool {
    flags.iter().any(|f| matches!(*f, "-y" | "--yes"))
}

/// Why a command stopped short of its work. None of them has written anything.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Stop {
    /// Esc or Ctrl-C at a question, or "no" to the last one.
    Cancelled,
    /// Arguments that cannot be acted on as given.
    Usage(String),
    Failed(String),
}

fn failed(e: anyhow::Error) -> Stop {
    Stop::Failed(format!("{e:#}"))
}

/// The questions, behind a trait so the decisions can be tested without a
/// terminal. `cli::terminal` is the real one.
pub(super) trait Ask {
    /// Something the next question depends on, said before it is asked.
    fn note(&mut self, text: &str);
    fn confirm(&mut self, question: &str, default: bool) -> Result<bool, Stop>;
    /// One of `options`, by index. The first is the default.
    fn choose(&mut self, question: &str, options: &[String]) -> Result<usize, Stop>;
    /// At least one of `options`, by index. All of them start ticked.
    fn pick(&mut self, question: &str, options: &[String]) -> Result<Vec<usize>, Stop>;
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct ExportPlan {
    pub dest: PathBuf,
    pub include_thumbs: bool,
}

/// Settles every export choice: from the flags, else by asking, else the
/// dialog's default. `name` is the dated default file name.
pub(super) fn plan_export(
    args: &ExportArgs,
    cwd: &Path,
    name: &str,
    est: &TransferEstimate,
    mut ask: Option<&mut dyn Ask>,
) -> Result<ExportPlan, Stop> {
    let dest = destination(args.target.as_deref(), cwd, name);
    let include_thumbs = match (args.thumbs, ask.as_deref_mut()) {
        (Some(thumbs), _) => thumbs,
        // The size is the whole question: it is what decides whether a few
        // hundred megabytes are worth carrying over a hotel connection.
        (None, Some(ask)) if est.thumb_count > 0 => ask.confirm(
            &format!(
                "Include {} ({})?",
                count(est.thumb_count, "thumbnail"),
                format_bytes(est.thumb_bytes)
            ),
            true,
        )?,
        (None, _) => true,
    };
    // Only asked when someone is there to answer. Unattended, overwriting is
    // the point: a nightly export replaces last night's under the same name.
    if !args.yes && dest.exists() {
        if let Some(ask) = ask {
            let question = format!("{} already exists. Overwrite it?", dest.display());
            if !ask.confirm(&question, false)? {
                return Err(Stop::Cancelled);
            }
        }
    }
    Ok(ExportPlan { dest, include_thumbs })
}

/// A directory — one that exists, or any path typed with a trailing
/// separator — gets the dated name inside it; anything else is the file.
fn destination(target: Option<&str>, cwd: &Path, name: &str) -> PathBuf {
    let Some(target) = target else {
        return cwd.join(name);
    };
    let path = cwd.join(target);
    if target.ends_with(std::path::is_separator) || path.is_dir() {
        path.join(name)
    } else {
        path
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct ImportPlan {
    pub picked: Vec<String>,
    pub mode: ImportMode,
    pub apply_settings: bool,
}

/// Settles every import choice, in the dialog's order: mode, channels,
/// settings, and then the button — which for a Replace asks a second time.
pub(super) fn plan_import(
    args: &ImportArgs,
    s: &ArchiveSummary,
    mut ask: Option<&mut dyn Ask>,
) -> Result<ImportPlan, Stop> {
    if s.channels.is_empty() {
        return Err(Stop::Failed("the archive holds no channels, so there is nothing to import".into()));
    }
    // The same at every step: a channel left unticked is skipped, never
    // removed, so what a Replace takes away is measured against the whole
    // archive and does not move as the checklist is worked through.
    let removal = removal_phrase(s.local_only_channels, s.local_only_videos);

    let mode = match (args.mode, ask.as_deref_mut()) {
        (Some(mode), _) => mode,
        (None, Some(ask)) => {
            let replace = match &removal {
                Some(r) => format!("Replace  make the picked channels match it, and remove {r} it doesn't contain"),
                None => "Replace  make the picked channels match it; nothing here would be removed".into(),
            };
            let options = vec!["Merge    add what the archive holds; nothing here is lost".to_string(), replace];
            match ask.choose("How should it be imported?", &options)? {
                0 => ImportMode::Merge,
                _ => ImportMode::Replace,
            }
        }
        (None, None) => ImportMode::Merge,
    };

    let picked: Vec<&ArchiveChannel> = match ask.as_deref_mut() {
        Some(ask) if s.channels.len() > 1 => {
            // Said out loud for the reason the dialog says it: the checklist
            // comes straight after Replace and would otherwise read as one.
            ask.note("Unticked channels are skipped entirely — not imported, not changed, not removed.");
            let labels: Vec<String> = s.channels.iter().map(channel_label).collect();
            ask.pick("Channels to import", &labels)?.into_iter().map(|i| &s.channels[i]).collect()
        }
        _ => s.channels.iter().collect(),
    };

    let apply_settings = match (args.settings, ask.as_deref_mut()) {
        (Some(apply), _) => apply,
        (None, Some(ask)) => {
            if !s.download_dir_exists {
                ask.note(&format!(
                    "The archive's download folder {} does not exist here, so applying its \
                     settings keeps this machine's own.",
                    s.download_dir
                ));
            }
            ask.confirm("Also apply its settings (download folder, template, player, intervals)?", true)?
        }
        (None, None) => true,
    };

    if !args.yes {
        match ask {
            Some(ask) => {
                let videos: usize = picked.iter().map(|c| c.video_count).sum();
                let channels = count(picked.len(), "channel");
                let go = match mode {
                    ImportMode::Merge => ask.confirm(
                        &format!("Import {channels} ({}) by merging?", count(videos, "video")),
                        true,
                    )?,
                    ImportMode::Replace => {
                        ask.note(&replace_warning(removal.as_deref(), picked.len(), videos));
                        ask.confirm("Replace?", false)?
                    }
                };
                if !go {
                    return Err(Stop::Cancelled);
                }
            }
            None if mode == ImportMode::Replace => {
                return Err(Stop::Usage(
                    "--replace removes rows, so without a terminal to confirm on it needs --yes as well".into(),
                ))
            }
            None => {}
        }
    }

    Ok(ImportPlan {
        picked: picked.iter().map(|c| c.channel_id.clone()).collect(),
        mode,
        apply_settings,
    })
}

/// One checklist row, worded as the dialog's: a bare number would say nothing.
fn channel_label(c: &ArchiveChannel) -> String {
    let mut label = format!("{} · {}", c.title, count(c.video_count, "video"));
    if c.already_here {
        label.push_str(" · already here");
    }
    // An uploader never subscribed to travels only so that a manually added
    // video of theirs has a parent row.
    if !c.subscribed {
        label.push_str(" · not subscribed");
    }
    label
}

/// "3 channels and 120 videos", dropping a zero half; `None` when a Replace
/// would remove nothing at all, which is worth saying in other words.
fn removal_phrase(channels: usize, videos: usize) -> Option<String> {
    let parts: Vec<String> = [(channels, "channel"), (videos, "video")]
        .into_iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, word)| count(n, word))
        .collect();
    (!parts.is_empty()).then(|| parts.join(" and "))
}

/// The dialog's Replace confirmation, word for word where the words still fit.
fn replace_warning(removal: Option<&str>, channels: usize, videos: usize) -> String {
    let picked = count(channels, "picked channel");
    match removal {
        Some(r) => format!(
            "Removes {r} this archive doesn't contain, and makes the {picked} match it \
             exactly — its {} become their library. Unpicked channels are left exactly as \
             they are. Database rows only: no downloaded file and no cached thumbnail is \
             deleted from disk.",
            count(videos, "video")
        ),
        None => format!(
            "Nothing would be removed — the archive contains every channel and video you \
             have. Replacing still overwrites the {picked} with the archive's copy of every \
             row, which is the one thing Merge will not do. Unpicked channels are left \
             exactly as they are, and nothing is deleted from disk."
        ),
    }
}

/// What the archive holds, before anything is asked about it.
fn archive_header(path: &Path, s: &ArchiveSummary) -> Vec<String> {
    let exported = chrono::DateTime::from_timestamp(s.exported_at, 0)
        .map(|t| t.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| "at an unknown time".into());
    let here = s.channels.iter().filter(|c| c.already_here).count();
    let thumbs = if s.includes_thumbs { count(s.thumb_count, "thumbnail") } else { "no thumbnails".into() };
    vec![
        format!("Archive  {}", path.display()),
        format!("         exported {exported} from {} by MyTube {}", s.exported_from, s.app_version),
        format!(
            "         {} ({here} already here) · {} · {thumbs}",
            count(s.channels.len(), "channel"),
            count(s.video_count, "video"),
        ),
    ]
}

pub(super) fn import_report(r: &ImportReport) -> Vec<String> {
    let mut lines = vec![format!(
        "Imported {} and {}{}.",
        count(r.channels_added, "channel"),
        count(r.videos_added, "video"),
        if r.downloads_relinked > 0 {
            format!(", and found {} already downloaded", grouped(r.downloads_relinked))
        } else {
            String::new()
        }
    )];
    if r.channels_updated + r.videos_updated > 0 {
        lines.push(format!(
            "Updated {} and {} that were already here.",
            count(r.channels_updated, "channel"),
            count(r.videos_updated, "video")
        ));
    }
    // Replace's removals are the one thing worth a line of their own: the
    // number is the whole point of the mode.
    if r.channels_removed + r.videos_removed > 0 {
        lines.push(format!(
            "Removed {} and {} from the library. No files were deleted.",
            count(r.videos_removed, "video row"),
            count(r.channels_removed, "channel")
        ));
    }
    match (r.settings_applied, r.download_dir_kept) {
        (true, true) => lines.push(
            "Applied the archive's settings, but kept this machine's download folder: \
             the archive's is not here."
                .into(),
        ),
        (true, false) => lines.push("Applied the archive's settings.".into()),
        _ => {}
    }
    lines
}

pub(super) fn export_report(est: &TransferEstimate, include_thumbs: bool, dest: &Path, bytes: u64) -> String {
    let thumbs = if include_thumbs && est.thumb_count > 0 {
        format!(" with {}", count(est.thumb_count, "thumbnail"))
    } else {
        String::new()
    };
    format!(
        "Exported {} and {}{thumbs} to {} ({}).",
        count(est.channel_count, "channel"),
        count(est.video_count, "video"),
        dest.display(),
        format_bytes(bytes)
    )
}

/// `mytube export`, start to finish. The exit code.
pub(super) fn export(args: ExportArgs) -> i32 {
    finish("export", run_export(&args).map(|line| vec![line]))
}

fn run_export(args: &ExportArgs) -> Result<String, Stop> {
    let db_path = config::db_path();
    // Never created here: an export from a library that does not exist is a
    // wrong user or a wrong machine, and an empty archive would hide that.
    if !db_path.is_file() {
        return Err(Stop::Failed(format!(
            "there is no MyTube library in {}. Has MyTube run on this machine, as this user?",
            config::config_dir().display()
        )));
    }
    let db = Db::open(&db_path).map_err(failed)?;
    let settings = config::load().map_err(failed)?;
    let est = library_transfer::estimate(&db).map_err(failed)?;
    say(&format!(
        "Library  {}\n         {} · {} · {} ({})",
        config::config_dir().display(),
        count(est.channel_count, "channel"),
        count(est.video_count, "video"),
        count(est.thumb_count, "thumbnail"),
        format_bytes(est.thumb_bytes)
    ));

    let cwd = std::env::current_dir().map_err(|e| Stop::Failed(format!("no current directory: {e}")))?;
    let mut terminal = Terminal::if_interactive();
    let plan = plan_export(
        args,
        &cwd,
        &transfer::default_archive_name(),
        &est,
        terminal.as_mut().map(|t| t as &mut dyn Ask),
    )?;

    let line = ProgressLine::new("Exporting");
    let written = library_transfer::export(&db, &settings, &plan.dest, plan.include_thumbs, &|done, total, current| {
        line.update(done, total, current)
    });
    line.finish();
    written.map_err(failed)?;
    let bytes = std::fs::metadata(&plan.dest).map(|m| m.len()).unwrap_or(0);
    Ok(export_report(&est, plan.include_thumbs, &plan.dest, bytes))
}

/// `mytube import`, start to finish. The exit code.
pub(super) fn import(args: ImportArgs) -> i32 {
    finish("import", run_import(&args))
}

fn run_import(args: &ImportArgs) -> Result<Vec<String>, Stop> {
    // Unlike export, a library that is not here yet is created: an import is a
    // fair way to set up a machine.
    config::ensure_dirs().map_err(failed)?;
    let db = Db::open(&config::db_path()).map_err(failed)?;
    let summary = library_transfer::inspect(&db, &args.file).map_err(failed)?;
    for line in archive_header(&args.file, &summary) {
        say(&line);
    }
    say(&format!("Library  {}", config::config_dir().display()));

    let mut terminal = Terminal::if_interactive();
    let plan = plan_import(args, &summary, terminal.as_mut().map(|t| t as &mut dyn Ask))?;

    let line = ProgressLine::new("Importing");
    let imported = library_transfer::import(
        &db,
        &args.file,
        &plan.picked,
        plan.mode,
        plan.apply_settings,
        &|done, total, current| line.update(done, total, current),
    );
    line.finish();
    Ok(import_report(&imported.map_err(failed)?))
}

/// Prints the outcome where it belongs — the report on stdout for a script to
/// keep, everything else on stderr — and returns the exit code.
fn finish(cmd: &str, outcome: Result<Vec<String>, Stop>) -> i32 {
    match outcome {
        Ok(report) => {
            for line in report {
                say(&line);
            }
            0
        }
        Err(Stop::Cancelled) => {
            super::warn("Cancelled. Nothing was written.");
            130
        }
        Err(Stop::Usage(why)) => {
            super::warn(&usage_text(cmd, &why));
            2
        }
        Err(Stop::Failed(why)) => {
            super::warn(&format!("mytube {cmd}: {why}"));
            1
        }
    }
}

/// "1 video", "3,412 videos".
fn count(n: usize, word: &str) -> String {
    format!("{} {word}{}", grouped(n), if n == 1 { "" } else { "s" })
}

fn grouped(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, d) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(d);
    }
    out
}

/// A size as the export tick quotes it: `formatBytes` in src/format.ts, whose
/// reasoning holds here too — three significant figures at most, 1024 to the
/// step, and a promotion a hair early so nothing reads "1024 KB".
fn format_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if n == 0 {
        return "0 B".into();
    }
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1023.95 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        return format!("{} B", value.round());
    }
    if value < 9.95 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{} {}", value.round(), UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn usage_of(a: Action) -> String {
        match a {
            Action::Usage(text) => text,
            other => panic!("expected a usage error, got {other:?}"),
        }
    }

    // ------------------------------------------------------------ arguments

    #[test]
    fn a_bare_export_leaves_every_choice_open() {
        assert_eq!(parse_export(&[]), Action::Export(ExportArgs::default()));
    }

    #[test]
    fn export_takes_a_path_and_its_flags_in_any_order() {
        assert_eq!(
            parse_export(&args(&["--no-thumbs", "~/sync/", "-y"])),
            Action::Export(ExportArgs {
                target: Some("~/sync/".into()),
                thumbs: Some(false),
                yes: true,
            })
        );
        assert_eq!(
            parse_export(&args(&["--thumbs", "--yes"])),
            Action::Export(ExportArgs { target: None, thumbs: Some(true), yes: true })
        );
    }

    #[test]
    fn a_path_that_looks_like_a_flag_is_taken_after_a_double_dash() {
        assert_eq!(
            parse_export(&args(&["--", "-odd.zip"])),
            Action::Export(ExportArgs { target: Some("-odd.zip".into()), ..Default::default() })
        );
    }

    #[test]
    fn export_refuses_what_it_cannot_act_on_and_says_where_help_is() {
        let contradiction = usage_of(parse_export(&args(&["--thumbs", "--no-thumbs"])));
        assert!(contradiction.contains("--thumbs") && contradiction.contains("--no-thumbs"));
        assert!(contradiction.contains("mytube export --help"), "{contradiction}");
        assert!(usage_of(parse_export(&args(&["a.zip", "b.zip"]))).contains("b.zip"));
        assert!(usage_of(parse_export(&args(&["--bogus"]))).contains("--bogus"));
        usage_of(parse_export(&args(&[""])));
    }

    #[test]
    fn help_wins_over_everything_else_on_the_line() {
        assert_eq!(parse_export(&args(&["--bogus", "--help"])), Action::Help(EXPORT_HELP));
        assert_eq!(parse_import(&args(&["-h"])), Action::Help(IMPORT_HELP));
        assert!(EXPORT_HELP.starts_with("Usage: mytube export"));
        assert!(IMPORT_HELP.starts_with("Usage: mytube import"));
    }

    #[test]
    fn import_needs_exactly_one_archive() {
        assert!(usage_of(parse_import(&[])).contains("mytube import FILE"));
        assert!(usage_of(parse_import(&args(&["a.zip", "b.zip"]))).contains("b.zip"));
        assert_eq!(
            parse_import(&args(&["a.zip"])),
            Action::Import(ImportArgs { file: "a.zip".into(), mode: None, settings: None, yes: false })
        );
    }

    #[test]
    fn import_reads_its_mode_and_settings_flags() {
        assert_eq!(
            parse_import(&args(&["--replace", "a.zip", "--no-settings", "-y"])),
            Action::Import(ImportArgs {
                file: "a.zip".into(),
                mode: Some(ImportMode::Replace),
                settings: Some(false),
                yes: true,
            })
        );
        assert_eq!(
            parse_import(&args(&["a.zip", "--merge", "--settings"])),
            Action::Import(ImportArgs {
                file: "a.zip".into(),
                mode: Some(ImportMode::Merge),
                settings: Some(true),
                yes: false,
            })
        );
        usage_of(parse_import(&args(&["a.zip", "--merge", "--replace"])));
        usage_of(parse_import(&args(&["a.zip", "--settings", "--no-settings"])));
        usage_of(parse_import(&args(&["a.zip", "--thumbs"])));
    }

    // ------------------------------------------------------------ questions

    #[derive(Debug)]
    enum Reply {
        Yes,
        No,
        Choose(usize),
        Pick(Vec<usize>),
        Esc,
    }

    /// Answers from a script, and keeps a transcript of what was said and
    /// asked, with each confirmation's default in brackets.
    #[derive(Default)]
    struct Script {
        replies: VecDeque<Reply>,
        said: Vec<String>,
    }

    impl Script {
        fn new(replies: Vec<Reply>) -> Self {
            Self { replies: replies.into(), said: Vec::new() }
        }
        fn next(&mut self, question: &str) -> Reply {
            self.replies.pop_front().unwrap_or_else(|| panic!("unscripted question: {question}"))
        }
        fn asked(&self) -> usize {
            self.said.iter().filter(|s| s.starts_with('?')).count()
        }
    }

    impl Ask for Script {
        fn note(&mut self, text: &str) {
            self.said.push(text.to_string());
        }
        fn confirm(&mut self, question: &str, default: bool) -> Result<bool, Stop> {
            self.said.push(format!("? {question} [{}]", if default { "Y/n" } else { "y/N" }));
            match self.next(question) {
                Reply::Yes => Ok(true),
                Reply::No => Ok(false),
                Reply::Esc => Err(Stop::Cancelled),
                other => panic!("{question}: a yes/no question got {other:?}"),
            }
        }
        fn choose(&mut self, question: &str, options: &[String]) -> Result<usize, Stop> {
            self.said.push(format!("? {question} {options:?}"));
            match self.next(question) {
                Reply::Choose(i) => Ok(i),
                Reply::Esc => Err(Stop::Cancelled),
                other => panic!("{question}: a choice got {other:?}"),
            }
        }
        fn pick(&mut self, question: &str, options: &[String]) -> Result<Vec<usize>, Stop> {
            self.said.push(format!("? {question} {options:?}"));
            match self.next(question) {
                Reply::Pick(i) => Ok(i),
                Reply::Esc => Err(Stop::Cancelled),
                other => panic!("{question}: a checklist got {other:?}"),
            }
        }
    }

    const NAME: &str = "mytube-export-2026-10-02.zip";

    fn est(thumb_count: usize) -> TransferEstimate {
        TransferEstimate {
            channel_count: 42,
            video_count: 3412,
            thumb_count,
            thumb_bytes: 112 * 1024 * 1024,
        }
    }

    #[test]
    fn unattended_export_writes_the_dated_name_here_with_thumbnails() {
        let cwd = Path::new("/home/me");
        let plan = plan_export(&ExportArgs::default(), cwd, NAME, &est(3100), None).unwrap();
        assert_eq!(plan, ExportPlan { dest: cwd.join(NAME), include_thumbs: true });
    }

    #[test]
    fn a_directory_target_gets_the_dated_name_inside_it() {
        let tmp = tempfile::tempdir().unwrap();
        let existing = ExportArgs { target: Some(tmp.path().to_string_lossy().into()), ..Default::default() };
        assert_eq!(plan_export(&existing, Path::new("/"), NAME, &est(0), None).unwrap().dest, tmp.path().join(NAME));

        // A trailing slash says "directory" before the directory exists.
        let new = ExportArgs { target: Some("sync/".into()), ..Default::default() };
        assert_eq!(
            plan_export(&new, tmp.path(), NAME, &est(0), None).unwrap().dest,
            tmp.path().join("sync").join(NAME)
        );

        let file = ExportArgs { target: Some("lib.zip".into()), ..Default::default() };
        assert_eq!(plan_export(&file, tmp.path(), NAME, &est(0), None).unwrap().dest, tmp.path().join("lib.zip"));
    }

    #[test]
    fn a_terminal_is_asked_about_thumbnails_with_their_real_size() {
        let mut ask = Script::new(vec![Reply::No]);
        let plan = plan_export(&ExportArgs::default(), Path::new("/x"), NAME, &est(3100), Some(&mut ask)).unwrap();
        assert!(!plan.include_thumbs);
        assert_eq!(ask.said, vec!["? Include 3,100 thumbnails (112 MB)? [Y/n]"]);
    }

    #[test]
    fn nothing_is_asked_about_thumbnails_there_are_none_of_or_the_flag_settled() {
        let mut ask = Script::default();
        plan_export(&ExportArgs::default(), Path::new("/x"), NAME, &est(0), Some(&mut ask)).unwrap();
        let flagged = ExportArgs { thumbs: Some(true), ..Default::default() };
        let plan = plan_export(&flagged, Path::new("/x"), NAME, &est(3100), Some(&mut ask)).unwrap();
        assert!(plan.include_thumbs);
        assert_eq!(ask.asked(), 0);
    }

    #[test]
    fn an_existing_archive_is_overwritten_only_on_a_yes_when_someone_can_be_asked() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join(NAME), b"yesterday").unwrap();
        let no_thumbs = ExportArgs { thumbs: Some(false), ..Default::default() };

        let mut ask = Script::new(vec![Reply::No]);
        assert_eq!(plan_export(&no_thumbs, tmp.path(), NAME, &est(0), Some(&mut ask)), Err(Stop::Cancelled));
        assert!(ask.said[0].contains("already exists") && ask.said[0].ends_with("[y/N]"), "{:?}", ask.said);

        let mut ask = Script::new(vec![Reply::Yes]);
        assert!(plan_export(&no_thumbs, tmp.path(), NAME, &est(0), Some(&mut ask)).is_ok());

        // --yes, or no terminal at all: a nightly export overwrites last night's.
        let yes = ExportArgs { yes: true, ..no_thumbs };
        let mut ask = Script::default();
        assert!(plan_export(&yes, tmp.path(), NAME, &est(0), Some(&mut ask)).is_ok());
        assert!(plan_export(&ExportArgs::default(), tmp.path(), NAME, &est(0), None).is_ok());
    }

    #[test]
    fn esc_at_an_export_question_cancels() {
        let mut ask = Script::new(vec![Reply::Esc]);
        assert_eq!(
            plan_export(&ExportArgs::default(), Path::new("/x"), NAME, &est(5), Some(&mut ask)),
            Err(Stop::Cancelled)
        );
    }

    fn chan(id: &str, videos: usize, here: bool) -> ArchiveChannel {
        ArchiveChannel {
            channel_id: id.into(),
            title: format!("Channel {id}"),
            video_count: videos,
            subscribed: true,
            member: false,
            already_here: here,
        }
    }

    fn summary(channels: Vec<ArchiveChannel>) -> ArchiveSummary {
        ArchiveSummary {
            format: 1,
            app_version: "3.0.0".into(),
            exported_at: 1_759_400_000,
            exported_from: "desktop-home".into(),
            includes_thumbs: true,
            thumb_count: 3,
            download_dir: "/home/me/Videos/YouTube".into(),
            download_dir_exists: true,
            video_count: channels.iter().map(|c| c.video_count).sum(),
            channels,
            local_only_channels: 3,
            local_only_videos: 120,
        }
    }

    fn import_args(mode: Option<ImportMode>, yes: bool) -> ImportArgs {
        ImportArgs { file: "a.zip".into(), mode, settings: None, yes }
    }

    fn three() -> ArchiveSummary {
        summary(vec![chan("A", 10, true), chan("B", 20, false), chan("C", 30, true)])
    }

    #[test]
    fn unattended_import_merges_every_channel_and_applies_settings() {
        let plan = plan_import(&import_args(None, false), &three(), None).unwrap();
        assert_eq!(
            plan,
            ImportPlan { picked: vec!["A".into(), "B".into(), "C".into()], mode: ImportMode::Merge, apply_settings: true }
        );
    }

    #[test]
    fn unattended_replace_needs_yes_as_well() {
        let refused = plan_import(&import_args(Some(ImportMode::Replace), false), &three(), None);
        assert!(matches!(&refused, Err(Stop::Usage(why)) if why.contains("--yes")), "{refused:?}");
        let plan = plan_import(&import_args(Some(ImportMode::Replace), true), &three(), None).unwrap();
        assert_eq!(plan.mode, ImportMode::Replace);
    }

    #[test]
    fn a_terminal_is_asked_mode_channels_settings_and_then_to_confirm() {
        let mut ask = Script::new(vec![Reply::Choose(0), Reply::Pick(vec![0, 2]), Reply::No, Reply::Yes]);
        let plan = plan_import(&import_args(None, false), &three(), Some(&mut ask)).unwrap();
        assert_eq!(
            plan,
            ImportPlan { picked: vec!["A".into(), "C".into()], mode: ImportMode::Merge, apply_settings: false }
        );
        assert_eq!(ask.asked(), 4, "{:?}", ask.said);
        let checklist = ask.said.iter().find(|s| s.contains("Channel B")).unwrap();
        assert!(checklist.contains("Channel B · 20 videos\""), "{checklist}");
        assert!(checklist.contains("Channel A · 10 videos · already here"), "{checklist}");
        assert!(
            ask.said.iter().any(|s| s.contains("skipped entirely")),
            "unticking is said not to be a delete: {:?}",
            ask.said
        );
        assert_eq!(ask.said.last().unwrap(), "? Import 2 channels (40 videos) by merging? [Y/n]");
    }

    #[test]
    fn the_replace_option_and_its_confirmation_say_what_it_removes_and_default_to_no() {
        let mut ask = Script::new(vec![Reply::Choose(1), Reply::Pick(vec![0, 1, 2]), Reply::Yes, Reply::No]);
        let outcome = plan_import(&import_args(None, false), &three(), Some(&mut ask));
        assert_eq!(outcome, Err(Stop::Cancelled), "a no to the confirmation cancels");
        assert!(ask.said[0].contains("3 channels and 120 videos"), "{}", ask.said[0]);
        let warning = &ask.said[ask.said.len() - 2];
        assert!(warning.contains("Removes 3 channels and 120 videos"), "{warning}");
        assert!(warning.contains("no downloaded file and no cached thumbnail is deleted"), "{warning}");
        assert!(ask.said.last().unwrap().ends_with("[y/N]"));
    }

    #[test]
    fn a_replace_that_removes_nothing_says_so() {
        let mut s = three();
        s.local_only_channels = 0;
        s.local_only_videos = 0;
        let flagged = ImportArgs { settings: Some(true), ..import_args(Some(ImportMode::Replace), false) };
        let mut ask = Script::new(vec![Reply::Pick(vec![0]), Reply::Yes]);
        plan_import(&flagged, &s, Some(&mut ask)).unwrap();
        assert!(ask.said.iter().any(|l| l.starts_with("Nothing would be removed")), "{:?}", ask.said);
    }

    #[test]
    fn flags_answer_their_questions_and_yes_skips_the_confirmation() {
        let flagged = ImportArgs { settings: Some(false), ..import_args(Some(ImportMode::Merge), true) };
        let mut ask = Script::new(vec![Reply::Pick(vec![1])]);
        let plan = plan_import(&flagged, &three(), Some(&mut ask)).unwrap();
        assert_eq!(plan.picked, vec!["B".to_string()]);
        assert_eq!(ask.asked(), 1, "only the checklist: {:?}", ask.said);
    }

    #[test]
    fn a_single_channel_needs_no_checklist() {
        let flagged = ImportArgs { settings: Some(true), ..import_args(Some(ImportMode::Merge), true) };
        let mut ask = Script::default();
        let plan = plan_import(&flagged, &summary(vec![chan("A", 1, false)]), Some(&mut ask)).unwrap();
        assert_eq!(plan.picked, vec!["A".to_string()]);
        assert_eq!(ask.asked(), 0);
    }

    #[test]
    fn a_missing_download_folder_is_mentioned_before_settings_are_offered() {
        let mut s = summary(vec![chan("A", 1, false)]);
        s.download_dir_exists = false;
        let mut ask = Script::new(vec![Reply::Choose(0), Reply::Yes, Reply::Yes]);
        plan_import(&import_args(None, false), &s, Some(&mut ask)).unwrap();
        let note = ask.said.iter().position(|l| l.contains("/home/me/Videos/YouTube")).expect("mentioned");
        let question = ask.said.iter().position(|l| l.starts_with('?') && l.contains("settings")).unwrap();
        assert!(note < question, "{:?}", ask.said);
    }

    #[test]
    fn an_archive_with_no_channels_is_refused() {
        assert!(matches!(plan_import(&import_args(None, true), &summary(vec![]), None), Err(Stop::Failed(_))));
    }

    #[test]
    fn esc_at_an_import_question_cancels() {
        let mut ask = Script::new(vec![Reply::Esc]);
        assert_eq!(plan_import(&import_args(None, false), &three(), Some(&mut ask)), Err(Stop::Cancelled));
    }

    // -------------------------------------------------------------- reports

    #[test]
    fn an_import_report_leaves_out_what_did_not_happen() {
        let quiet = ImportReport { videos_added: 1, ..Default::default() };
        assert_eq!(import_report(&quiet), vec!["Imported 0 channels and 1 video."]);

        let busy = ImportReport {
            channels_added: 3,
            videos_added: 2100,
            channels_updated: 38,
            videos_updated: 2950,
            downloads_relinked: 12,
            channels_removed: 3,
            videos_removed: 120,
            settings_applied: true,
            download_dir_kept: true,
            ..Default::default()
        };
        assert_eq!(
            import_report(&busy),
            vec![
                "Imported 3 channels and 2,100 videos, and found 12 already downloaded.",
                "Updated 38 channels and 2,950 videos that were already here.",
                "Removed 120 video rows and 3 channels from the library. No files were deleted.",
                "Applied the archive's settings, but kept this machine's download folder: the archive's is not here.",
            ]
        );
    }

    #[test]
    fn an_export_report_names_the_file_and_its_size() {
        let dest = Path::new("/home/me/mytube-export-2026-10-02.zip");
        assert_eq!(
            export_report(&est(3100), true, dest, 118 * 1024 * 1024),
            "Exported 42 channels and 3,412 videos with 3,100 thumbnails to \
             /home/me/mytube-export-2026-10-02.zip (118 MB)."
        );
        assert_eq!(
            export_report(&est(3100), false, dest, 2 * 1024 * 1024),
            "Exported 42 channels and 3,412 videos to /home/me/mytube-export-2026-10-02.zip (2.0 MB)."
        );
    }

    #[test]
    fn counts_are_pluralised_and_grouped() {
        assert_eq!(count(1, "video"), "1 video");
        assert_eq!(count(0, "video"), "0 videos");
        assert_eq!(count(1_234_567, "video"), "1,234,567 videos");
    }

    #[test]
    fn sizes_read_as_the_export_tick_quotes_them() {
        // The same cases as `formatBytes` in src/format.test.ts.
        for (n, want) in [
            (0, "0 B"),
            (999, "999 B"),
            (1023, "1023 B"),
            (1024, "1.0 KB"),
            (812 * 1024, "812 KB"),
            (112 * 1024 * 1024, "112 MB"),
            (1024 * 1024 - 1, "1.0 MB"),
            (1024u64.pow(3) - 1, "1.0 GB"),
            ((1.4 * 1024f64.powi(3)).round() as u64, "1.4 GB"),
            (3 * 1024u64.pow(5), "3072 TB"),
        ] {
            assert_eq!(format_bytes(n), want, "{n}");
        }
    }
}
