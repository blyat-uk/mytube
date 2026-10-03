//! The terminal itself: `cli::library`'s questions, drawn by inquire, and the
//! one-line progress readout. Nothing here decides anything.

use super::library::{Ask, Stop};
use inquire::error::InquireError;
use inquire::list_option::ListOption;
use inquire::validator::Validation;
use inquire::{Confirm, MultiSelect, Select};
use std::cell::Cell;
use std::io::{IsTerminal, Write};
use std::time::{Duration, Instant};

/// Asks on the terminal. There is one only when both ends of a question are:
/// stdin to read the answer from, and stderr, where inquire draws — stdout may
/// well be a log file.
pub(super) struct Terminal;

impl Terminal {
    pub(super) fn if_interactive() -> Option<Self> {
        (std::io::stdin().is_terminal() && std::io::stderr().is_terminal()).then_some(Terminal)
    }
}

/// Channels shown at once in the checklist; the rest scroll.
const PAGE: usize = 12;

fn stop(e: InquireError) -> Stop {
    match e {
        InquireError::OperationCanceled | InquireError::OperationInterrupted => Stop::Cancelled,
        other => Stop::Failed(format!("the terminal: {other}")),
    }
}

impl Ask for Terminal {
    fn note(&mut self, text: &str) {
        super::warn(text);
    }

    fn confirm(&mut self, question: &str, default: bool) -> Result<bool, Stop> {
        Confirm::new(question).with_default(default).prompt().map_err(stop)
    }

    fn choose(&mut self, question: &str, options: &[String]) -> Result<usize, Stop> {
        // Echoed back as its first word: the rest was there to choose by.
        let first_word =
            &|o: ListOption<&String>| o.value.split_whitespace().next().unwrap_or_default().to_string();
        Select::new(question, options.to_vec())
            .with_formatter(first_word)
            .raw_prompt()
            .map(|o| o.index)
            .map_err(stop)
    }

    fn pick(&mut self, question: &str, options: &[String]) -> Result<Vec<usize>, Stop> {
        // Echoed back as a tally: forty channel names would bury the next question.
        let total = options.len();
        let tally = move |picked: &[ListOption<&String>]| format!("{} of {total}", picked.len());
        MultiSelect::new(question, options.to_vec())
            .with_all_selected_by_default()
            .with_page_size(PAGE)
            .with_formatter(&tally)
            .with_validator(|picked: &[ListOption<&String>]| {
                Ok(if picked.is_empty() {
                    Validation::Invalid("Pick at least one channel, or press Esc to cancel.".into())
                } else {
                    Validation::Valid
                })
            })
            .raw_prompt()
            .map(|picked| picked.into_iter().map(|o| o.index).collect())
            .map_err(stop)
    }
}

/// How wide the progress line may get. Fixed rather than measured: it only has
/// to fit the narrowest terminal anyone uses, and a line that wraps cannot be
/// redrawn in place.
const WIDTH: usize = 78;

/// Redraws at most this often. Thousands of thumbnails go by in a second or two,
/// and every redraw is bytes down an SSH link.
const TICK: Duration = Duration::from_millis(100);

/// `Importing 1234/3412  Some video title…` on stderr, redrawn in place. Off
/// when stderr is not a terminal, so a cron log gets the report and no noise.
pub(super) struct ProgressLine {
    verb: &'static str,
    live: bool,
    last: Cell<Option<Instant>>,
    drawn: Cell<bool>,
}

impl ProgressLine {
    pub(super) fn new(verb: &'static str) -> Self {
        Self { verb, live: std::io::stderr().is_terminal(), last: Cell::new(None), drawn: Cell::new(false) }
    }

    pub(super) fn update(&self, done: usize, total: usize, current: &str) {
        if !self.live {
            return;
        }
        let now = Instant::now();
        if done < total && self.last.get().is_some_and(|t| now.duration_since(t) < TICK) {
            return;
        }
        self.last.set(Some(now));
        let mut err = std::io::stderr().lock();
        let _ = write!(err, "\r\x1b[2K{}", progress_text(self.verb, done, total, current));
        let _ = err.flush();
        self.drawn.set(true);
    }

    /// Clears the line, so the report that follows starts on a clean one.
    pub(super) fn finish(&self) {
        if self.drawn.get() {
            let mut err = std::io::stderr().lock();
            let _ = write!(err, "\r\x1b[2K");
            let _ = err.flush();
        }
    }
}

fn progress_text(verb: &str, done: usize, total: usize, current: &str) -> String {
    let head = format!("{verb} {done}/{total}");
    let room = WIDTH.saturating_sub(head.len() + 2);
    format!("{head}  {}", clip(current, room))
}

/// Cut to fit `room` columns, ending in `…` when anything was cut. Anything
/// outside ASCII counts as two columns: exact for CJK, which really is that
/// wide, and over-cautious for the rest — which only costs a few characters of
/// a title already being cut. A control character becomes a space, since a
/// newline inside a title would break the line it is redrawn on.
fn clip(text: &str, room: usize) -> String {
    let width = |c: char| if c.is_ascii() { 1 } else { 2 };
    let clean = text.chars().map(|c| if c.is_control() { ' ' } else { c });
    if clean.clone().map(width).sum::<usize>() <= room {
        return clean.collect();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in clean {
        // One column kept back for the ellipsis.
        if used + width(c) > room.saturating_sub(1) {
            break;
        }
        used += width(c);
        out.push(c);
    }
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Columns as `clip` counts them.
    fn cols(s: &str) -> usize {
        s.chars().map(|c| if c.is_ascii() { 1 } else { 2 }).sum::<usize>() - s.matches('…').count()
    }

    #[test]
    fn a_title_that_fits_is_left_alone() {
        assert_eq!(clip("Short title", 20), "Short title");
    }

    #[test]
    fn a_long_title_is_cut_and_marked() {
        assert_eq!(clip("A rather long title", 8), "A rathe…");
    }

    #[test]
    fn wide_characters_count_double() {
        // Four CJK characters are eight columns; seven leave room for three and the ellipsis.
        assert_eq!(clip("日本語の", 7), "日本語…");
        assert_eq!(clip("日本語の", 8), "日本語の");
    }

    #[test]
    fn control_characters_cannot_break_the_line() {
        assert_eq!(clip("two\nlines\r", 20), "two lines ");
    }

    #[test]
    fn the_progress_line_fits_the_width_it_promises() {
        let long = "x".repeat(200);
        let text = progress_text("Importing", 1234, 3412, &long);
        assert!(text.starts_with("Importing 1234/3412  x"), "{text}");
        assert!(cols(&text) <= WIDTH, "{} columns", cols(&text));
        assert_eq!(progress_text("Exporting", 1, 2, "manifest.json"), "Exporting 1/2  manifest.json");
    }
}
