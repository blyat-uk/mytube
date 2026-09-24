//! Config export / import: the archive format, and every byte of filesystem
//! work an export or import does.
//!
//! **Nothing in here touches the database.** An export is handed rows and turns
//! them into a file; an import turns a file back into rows and hands them to a
//! caller, which is the only thing that opens a transaction. That line is what
//! makes the whole module testable with nothing but a temp directory — no
//! `Db`, no Tauri runtime, no `AppHandle`, which is also why the progress
//! callbacks are plain `&dyn Fn` rather than an app handle to emit events on.
//!
//! The archive is an ordinary zip:
//!
//! ```text
//! manifest.json          everything but pixels
//! thumbs/<video_id>.jpg  only when thumbnails were included
//! ```
//!
//! `manifest.json` is snake_case throughout, unlike half the IPC payloads in
//! `models.rs`. It is a *file format*: it mirrors database column names, it is
//! read by a future version of this app rather than by TypeScript, and it never
//! crosses the IPC boundary — only the summary structs do, and those carry
//! their own `camelCase` rename.

use crate::config::{self, Settings};
use crate::models::{
    ArchiveChannel, ArchiveSummary, Channel, DownloadState, ImportReport, TransferEstimate, Video,
    VideoStatus,
};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use zip::write::SimpleFileOptions;
use zip::CompressionMethod;

/// The archive format this build writes.
///
/// One rule, and only one: an archive claiming a *higher* number is refused
/// outright with a message naming the problem. A lower or equal one is read.
/// That is the whole compatibility contract — a future version may add fields
/// (serde ignores what it does not know) but the moment it changes a meaning it
/// bumps this, and older builds stop guessing instead of half-applying.
pub const FORMAT: u32 = 1;

const MANIFEST: &str = "manifest.json";
const THUMB_PREFIX: &str = "thumbs/";
const THUMB_SUFFIX: &str = ".jpg";

// ---------------------------------------------------------------- the format

/// The portable half of `settings.json`: what describes the library rather than
/// this screen. Window geometry and the `view` block are deliberately absent —
/// see `Settings::adopt_portable`, which is the other end of this.
///
/// `extra` is flattened for the same reason `Settings` flattens it: a key this
/// build cannot name still has to survive the trip.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PortableSettings {
    download_dir: String,
    filename_template: String,
    player_command: String,
    max_concurrent_downloads: usize,
    poll_interval_minutes: u64,
    poll_on_startup: bool,
    backfill_count: u32,
    card_size: u32,
    #[serde(flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
}

impl PortableSettings {
    fn of(s: &Settings) -> Self {
        Self {
            download_dir: s.download_dir.clone(),
            filename_template: s.filename_template.clone(),
            player_command: s.player_command.clone(),
            max_concurrent_downloads: s.max_concurrent_downloads,
            poll_interval_minutes: s.poll_interval_minutes,
            poll_on_startup: s.poll_on_startup,
            backfill_count: s.backfill_count,
            card_size: s.card_size,
            extra: s.extra.clone(),
        }
    }

    /// Back to a `Settings`, via `from_json_str` rather than field assignment so
    /// an archive's numbers are clamped exactly as a hand-edited settings.json
    /// would be. The file came off another machine and is no more trustworthy
    /// than one somebody typed.
    fn to_settings(&self) -> Result<Settings> {
        Settings::from_json_str(&serde_json::to_string(self)?)
            .context("the archive's settings block is not readable")
    }
}

/// One video as the manifest stores it: every column of [`Video`] except the
/// three that mean nothing on another machine.
///
/// - `channel_title` is denormalised from a join, so it is rebuilt on import
///   from the channels list rather than carried.
/// - `thumb_path` is an absolute path into the *exporting* machine's config
///   directory. The pixels travel under `thumbs/<id>.jpg`; the path is rebuilt.
/// - `file_path` becomes `rel_path` / `abs_path` below.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct ArchiveVideo {
    id: String,
    channel_id: String,
    title: String,
    description: Option<String>,
    thumb_url: Option<String>,
    published_at: Option<i64>,
    sort_at: Option<i64>,
    feed_rank: i64,
    added_manually: bool,
    duration_secs: Option<i64>,
    view_count: Option<i64>,
    status: VideoStatus,
    hidden: bool,
    watched: bool,
    watched_at: Option<i64>,
    download_state: DownloadState,
    download_error: Option<String>,
    /// The downloaded file's path *relative to the exporting machine's
    /// `download_dir`*, which the importer re-roots against its own. The normal
    /// case, and the only one that survives a move to another machine.
    rel_path: Option<String>,
    /// Set instead of `rel_path` when the file lay outside `download_dir` and
    /// so cannot be re-rooted at all — a download the folder has since moved
    /// out from under, or one placed by hand. Stored absolute and honestly:
    /// pretending it was relative to anything would invent a path.
    ///
    /// Two nullable fields rather than a tagged object because this manifest
    /// mirrors database columns, and a person reading it should be able to see
    /// which kind of path a row carries without decoding a wrapper.
    abs_path: Option<String>,
    downloaded_at: Option<i64>,
    first_seen_at: i64,
    sibling_group: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Manifest {
    format: u32,
    app_version: String,
    exported_at: i64,
    exported_from: String,
    /// The exporting machine's download folder: the root every `rel_path` was
    /// measured from, recorded even when the *setting* is not applied on import
    /// so the dialog can say where these files used to live.
    download_dir: String,
    includes_thumbs: bool,
    settings: PortableSettings,
    channels: Vec<Channel>,
    videos: Vec<ArchiveVideo>,
}

/// Just enough of the manifest to version-check it.
///
/// The check has to happen *before* the full parse, not after: a format that
/// reshaped a field would fail `from_str` with a serde error about a missing
/// key, and "invalid type: string, expected i64 at line 1 column 8213" is not
/// the message this failure deserves.
#[derive(Deserialize)]
struct FormatProbe {
    format: u32,
}

// ------------------------------------------------------------ path re-rooting

/// Splits an absolute `file_path` into the `(rel_path, abs_path)` pair the
/// manifest stores. Exactly one of the two is `Some`.
///
/// `Path::strip_prefix` compares component by component, so a `download_dir`
/// written with a trailing slash and one written without produce the same
/// answer — which matters, because that string is hand-editable in
/// settings.json and both forms occur in the wild.
fn portable_path(file_path: &str, download_dir: &str) -> (Option<String>, Option<String>) {
    let rel = Path::new(file_path)
        .strip_prefix(Path::new(download_dir))
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty());
    match rel {
        Some(r) => (Some(r), None),
        None => (None, Some(file_path.to_string())),
    }
}

/// The inverse: where this machine would keep the file the manifest describes.
/// `None` when the row carried no path at all.
fn rejoin_path(v: &ArchiveVideo, download_dir: &str) -> Option<PathBuf> {
    match (&v.rel_path, &v.abs_path) {
        (Some(rel), _) => Some(Path::new(download_dir).join(rel)),
        (None, Some(abs)) => Some(PathBuf::from(abs)),
        (None, None) => None,
    }
}

// ------------------------------------------------------------------- export

impl ArchiveVideo {
    /// Normalises the download state on the way out, because only `Done` means
    /// anything on another machine.
    ///
    /// `Queued` and `Downloading` are transient — `Db::reset_stale_downloads`
    /// wipes them at every startup anyway, since no yt-dlp child survives a
    /// restart — so they travel as `None` rather than as a queue position
    /// another machine would have to honour. `Failed` travels as `None` *and*
    /// drops its error: that string is a message from another machine's yt-dlp
    /// about another machine's network, and repeating it here would be a lie.
    fn of(v: &Video, download_dir: &str) -> Self {
        let done = v.download_state == DownloadState::Done;
        let (rel_path, abs_path) = match (done, v.file_path.as_deref()) {
            (true, Some(p)) => portable_path(p, download_dir),
            _ => (None, None),
        };
        Self {
            id: v.id.clone(),
            channel_id: v.channel_id.clone(),
            title: v.title.clone(),
            description: v.description.clone(),
            thumb_url: v.thumb_url.clone(),
            published_at: v.published_at,
            sort_at: v.sort_at,
            feed_rank: v.feed_rank,
            added_manually: v.added_manually,
            duration_secs: v.duration_secs,
            view_count: v.view_count,
            status: v.status,
            hidden: v.hidden,
            watched: v.watched,
            watched_at: v.watched_at,
            download_state: if done { DownloadState::Done } else { DownloadState::None },
            download_error: None,
            rel_path,
            abs_path,
            downloaded_at: if done { v.downloaded_at } else { None },
            first_seen_at: v.first_seen_at,
            sibling_group: v.sibling_group.clone(),
        }
    }

    /// The row to hand the database, before step 4 and step 5 of
    /// `prepare_import` fill in `thumb_path` and the download.
    fn into_video(self, channel_title: String) -> Video {
        Video {
            id: self.id,
            channel_id: self.channel_id,
            channel_title,
            title: self.title,
            description: self.description,
            thumb_url: self.thumb_url,
            thumb_path: None,
            published_at: self.published_at,
            sort_at: self.sort_at,
            feed_rank: self.feed_rank,
            added_manually: self.added_manually,
            duration_secs: self.duration_secs,
            view_count: self.view_count,
            status: self.status,
            hidden: self.hidden,
            watched: self.watched,
            watched_at: self.watched_at,
            download_state: DownloadState::None,
            download_error: None,
            file_path: None,
            downloaded_at: None,
            first_seen_at: self.first_seen_at,
            sibling_group: self.sibling_group,
        }
    }
}

/// `mytube-export-YYYY-MM-DD.zip`. The date is the archive's only human handle
/// — two exports on one day overwrite each other, which is the right default
/// for a backup you are about to carry somewhere.
pub fn default_archive_name() -> String {
    format!("mytube-export-{}.zip", chrono::Local::now().format("%Y-%m-%d"))
}

/// What an export would weigh, so the "Include thumbnails" tick can show a real
/// number rather than a guess. Stats the cached thumbnails; reads none of them.
pub fn estimate(channels: &[Channel], videos: &[Video]) -> TransferEstimate {
    let mut est = TransferEstimate {
        channel_count: channels.len(),
        video_count: videos.len(),
        ..Default::default()
    };
    for v in videos {
        if let Some(p) = v.thumb_path.as_deref() {
            if let Ok(meta) = std::fs::metadata(p) {
                est.thumb_count += 1;
                est.thumb_bytes += meta.len();
            }
        }
    }
    est
}

/// Writes the archive. `progress` is called as work proceeds so the caller can
/// emit Tauri events without this module depending on Tauri.
pub fn write_archive(
    dest: &Path,
    channels: &[Channel],
    videos: &[Video],
    settings: &Settings,
    include_thumbs: bool,
    progress: &dyn Fn(usize, usize, &str),
) -> Result<()> {
    let manifest = Manifest {
        format: FORMAT,
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        exported_at: chrono::Utc::now().timestamp(),
        exported_from: hostname(),
        download_dir: settings.download_dir.clone(),
        // The flag records what was *asked for*. Whether any thumbnail actually
        // existed to copy is a fact of the file listing, which `read_summary`
        // counts rather than trusts.
        includes_thumbs: include_thumbs,
        settings: PortableSettings::of(settings),
        channels: channels.to_vec(),
        videos: videos.iter().map(|v| ArchiveVideo::of(v, &settings.download_dir)).collect(),
    };
    let json = serde_json::to_vec_pretty(&manifest)?;

    let thumbs: Vec<&Video> = if include_thumbs {
        videos
            .iter()
            .filter(|v| v.thumb_path.as_deref().is_some_and(|p| Path::new(p).is_file()))
            .collect()
    } else {
        Vec::new()
    };
    let total = 1 + thumbs.len();
    progress(0, total, MANIFEST);

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let file = std::fs::File::create(dest)
        .with_context(|| format!("cannot write {}", dest.display()))?;
    let mut zip = zip::ZipWriter::new(std::io::BufWriter::new(file));

    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    // JPEG is already compressed. Deflating a hundred megabytes of it costs
    // real CPU to save a percent or two, so thumbnails go in verbatim.
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);

    zip.start_file(MANIFEST, deflated)?;
    zip.write_all(&json)?;
    progress(1, total, MANIFEST);

    let mut seen: HashSet<&str> = HashSet::new();
    for (i, v) in thumbs.iter().enumerate() {
        if !seen.insert(v.id.as_str()) {
            continue; // ids are a primary key; belt and braces against a duplicate name.
        }
        let src = v.thumb_path.as_deref().unwrap_or_default();
        // A thumbnail that vanished between the listing and now is not worth
        // failing an export over — the importer simply re-fetches it.
        if let Ok(bytes) = std::fs::read(src) {
            zip.start_file(format!("{THUMB_PREFIX}{}{THUMB_SUFFIX}", v.id), stored)?;
            zip.write_all(&bytes)?;
        }
        progress(2 + i, total, &v.title);
    }

    // `finish` hands the BufWriter back rather than dropping it, because a
    // BufWriter dropped on its own swallows the error from its last flush --
    // and that flush is the tail of the archive.
    let mut out = zip.finish()?;
    out.flush().context("flushing the archive")?;
    Ok(())
}

fn hostname() -> String {
    // Linux-only app, so the kernel's own copy is the live answer and always
    // present; /etc/hostname can be stale or missing on a systemd-networkd box.
    for p in ["/proc/sys/kernel/hostname", "/etc/hostname"] {
        if let Ok(s) = std::fs::read_to_string(p) {
            let s = s.trim();
            if !s.is_empty() {
                return s.to_string();
            }
        }
    }
    std::env::var("HOSTNAME").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "unknown".into())
}

// ------------------------------------------------------------------- reading

type Archive = zip::ZipArchive<std::io::BufReader<std::fs::File>>;

fn open(path: &Path) -> Result<Archive> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("cannot open {}", path.display()))?;
    zip::ZipArchive::new(std::io::BufReader::new(file))
        .with_context(|| format!("{} is not a MyTube archive", path.display()))
}

fn read_manifest(zip: &mut Archive) -> Result<Manifest> {
    let mut raw = String::new();
    zip.by_name(MANIFEST)
        .context("this zip has no manifest.json, so it is not a MyTube archive")?
        .read_to_string(&mut raw)?;

    let probe: FormatProbe =
        serde_json::from_str(&raw).context("the archive's manifest.json is not readable")?;
    if probe.format > FORMAT {
        bail!(
            "This archive was written by a newer version of MyTube. \
             It is in archive format {}, and this build reads up to format {FORMAT}.",
            probe.format
        );
    }
    serde_json::from_str(&raw).context("the archive's manifest.json is not readable")
}

/// Reads ONLY the manifest. Writes nothing, anywhere. `known_channel_ids` is
/// what this machine already has, so the summary can flag each row.
pub fn read_summary(path: &Path, known_channel_ids: &HashSet<String>) -> Result<ArchiveSummary> {
    let mut zip = open(path)?;
    let m = read_manifest(&mut zip)?;

    let ids: HashSet<&str> = m.videos.iter().map(|v| v.id.as_str()).collect();
    let thumb_count = zip.file_names().filter(|n| thumb_entry_id(n, &ids).is_some()).count();

    let mut per_channel: HashMap<&str, usize> = HashMap::new();
    for v in &m.videos {
        *per_channel.entry(v.channel_id.as_str()).or_default() += 1;
    }

    Ok(ArchiveSummary {
        format: m.format,
        app_version: m.app_version,
        exported_at: m.exported_at,
        exported_from: m.exported_from,
        includes_thumbs: m.includes_thumbs,
        thumb_count,
        download_dir_exists: Path::new(&m.download_dir).is_dir(),
        download_dir: m.download_dir,
        video_count: m.videos.len(),
        // Not knowable here: this module never opens a database.
        // `commands::read_archive` fills both in.
        local_only_channels: 0,
        local_only_videos: 0,
        channels: m
            .channels
            .iter()
            .map(|c| ArchiveChannel {
                video_count: per_channel.get(c.id.as_str()).copied().unwrap_or(0),
                already_here: known_channel_ids.contains(&c.id),
                channel_id: c.id.clone(),
                title: c.title.clone(),
                subscribed: c.subscribed,
                member: c.member,
            })
            .collect(),
    })
}

/// The zip-slip guard, and the only place an entry name from an archive is ever
/// believed. Returns the video id when `name` is *exactly*
/// `thumbs/<id>.jpg` for an id the manifest carries, and `None` for everything
/// else — `thumbs/../../evil.jpg`, `thumbs/sub/dir.jpg`, an absolute name, a
/// symlink entry, an id nobody asked about.
///
/// The extraction path is then built from the returned id, never from the name
/// the archive supplied, so even a bug here cannot place a file outside the
/// thumbnail cache. `ZipArchive::extract` is deliberately never called.
fn thumb_entry_id<'a>(name: &'a str, wanted: &HashSet<&str>) -> Option<&'a str> {
    let id = name.strip_prefix(THUMB_PREFIX)?.strip_suffix(THUMB_SUFFIX)?;
    let clean = !id.is_empty()
        && id != "."
        && id != ".."
        && !id.contains('/')
        && !id.contains('\\')
        && !id.contains('\0');
    (clean && wanted.contains(id)).then_some(id)
}

// ------------------------------------------------------------------- import

/// What an import has prepared but not yet committed: rows ready for the
/// database, and the half of the report only this module can know.
pub struct Prepared {
    pub channels: Vec<Channel>,
    pub videos: Vec<Video>,
    pub report: ImportReport,
    /// Every channel id the archive carries, ticked or not -- a *wider* set
    /// than `channels`. A Replace measures what to delete against this, never
    /// against the picked subset: leaving a row unticked means "skip it", and
    /// a checklist that quietly doubled as a deletion control would be a trap.
    pub archive_channel_ids: Vec<String>,
}

/// The files an import touches outside the database: this machine's thumbnail
/// cache, and its settings file.
///
/// It exists as a parameter rather than a pair of `config::` calls so the tests
/// can point an import at a temp directory. `config_dir()` is built from
/// `$XDG_CONFIG_HOME`, which is process-wide state; a struct is not, so these
/// tests run in parallel with each other and with everything else in the crate.
struct Dirs {
    thumbs: PathBuf,
    settings: PathBuf,
}

impl Dirs {
    fn live() -> Self {
        Self { thumbs: config::thumbs_dir(), settings: config::settings_path() }
    }
}

/// Everything an import does outside the database: applies settings, extracts
/// thumbnails, re-roots download paths, and builds the rows to commit.
pub fn prepare_import(
    path: &Path,
    picked: &[String],
    apply_settings: bool,
    progress: &dyn Fn(usize, usize, &str),
) -> Result<Prepared> {
    prepare_import_in(path, picked, apply_settings, &Dirs::live(), progress)
}

/// The ordering below is load-bearing, and steps 3 and 4 touch disk *before*
/// the caller's database transaction runs. A failure after this function
/// returns therefore leaves the settings applied and the thumbnails written —
/// both non-destructive: a settings file can be edited back, and a cached
/// thumbnail is the same image the poll would have fetched anyway. The row
/// changes are the part that has to be all-or-nothing, and those are still
/// entirely in the caller's transaction, which is exactly why this function
/// hands back `Vec`s instead of writing them itself.
fn prepare_import_in(
    path: &Path,
    picked: &[String],
    apply_settings: bool,
    dirs: &Dirs,
    progress: &dyn Fn(usize, usize, &str),
) -> Result<Prepared> {
    // 1. Parse and version-check.
    let mut zip = open(path)?;
    let m = read_manifest(&mut zip)?;
    let mut report = ImportReport::default();

    // Taken before the filter below consumes `m.channels`: a Replace needs the
    // archive's *whole* roster to know what it does not contain, which is a
    // different question from what the user ticked.
    let archive_channel_ids: Vec<String> = m.channels.iter().map(|c| c.id.clone()).collect();

    // 2. Filter channels to `picked`, and videos to those channels.
    let wanted: HashSet<&str> = picked.iter().map(String::as_str).collect();
    let channels: Vec<Channel> =
        m.channels.into_iter().filter(|c| wanted.contains(c.id.as_str())).collect();
    let kept: HashSet<&str> = channels.iter().map(|c| c.id.as_str()).collect();
    let archive_videos: Vec<ArchiveVideo> =
        m.videos.into_iter().filter(|v| kept.contains(v.channel_id.as_str())).collect();

    // 3. Settings first — the effective `download_dir` is the root step 5
    //    re-roots every `rel_path` against, so it has to be settled before any
    //    path is resolved.
    let mut settings = config::load_from(&dirs.settings)?;
    if apply_settings {
        let mine = settings.download_dir.clone();
        settings.adopt_portable(&m.settings.to_settings()?);
        if !Path::new(&settings.download_dir).is_dir() {
            // The other machine's folder does not exist here. Taking it anyway
            // would point every future download at a path that cannot be
            // written, so this machine keeps its own and the report says so.
            settings.download_dir = mine;
            report.download_dir_kept = true;
        }
        config::save_to(&dirs.settings, &settings)?;
        report.settings_applied = true;
    }
    let download_dir = settings.download_dir.clone();

    // 4. Thumbnails. Every entry is validated by `thumb_entry_id` and written
    //    to a path built from the id it returned, never from the archive's own
    //    name; `extract()` is never called on anything.
    let ids: HashSet<&str> = archive_videos.iter().map(|v| v.id.as_str()).collect();
    let entries: Vec<(String, String)> = zip
        .file_names()
        .filter_map(|n| thumb_entry_id(n, &ids).map(|id| (n.to_string(), id.to_string())))
        .collect();

    let total = entries.len() + archive_videos.len();
    let mut done = 0usize;
    if !entries.is_empty() {
        std::fs::create_dir_all(&dirs.thumbs)?;
    }
    for (name, id) in &entries {
        let dest = dirs.thumbs.join(format!("{id}{THUMB_SUFFIX}"));
        done += 1;
        // Never overwrite: a cached thumbnail here is byte-for-byte the image
        // the archive carries, both having come from the same YouTube URL.
        if dest.exists() {
            progress(done, total, id);
            continue;
        }
        let mut bytes = Vec::new();
        zip.by_name(name)?.read_to_end(&mut bytes)?;
        std::fs::write(&dest, &bytes)?;
        report.thumbs_written += 1;
        progress(done, total, id);
    }

    let titles: HashMap<&str, &str> =
        channels.iter().map(|c| (c.id.as_str(), c.title.as_str())).collect();

    let mut videos = Vec::with_capacity(archive_videos.len());
    for av in archive_videos {
        // 5. Re-root the download against the effective folder.
        let landed = (av.download_state == DownloadState::Done)
            .then(|| rejoin_path(&av, &download_dir))
            .flatten()
            .filter(|p| p.is_file());

        // 6. Rebuild `channel_title` from the channels list rather than from
        //    the manifest, which never carried it.
        let channel_title = titles.get(av.channel_id.as_str()).copied().unwrap_or("").to_string();
        let cached = dirs.thumbs.join(format!("{}{THUMB_SUFFIX}", av.id));
        let archived_at = av.downloaded_at;
        let mut v = av.into_video(channel_title);

        v.thumb_path = cached.is_file().then(|| cached.to_string_lossy().into_owned());
        if let Some(p) = landed {
            // Only a file that is actually here is claimed as downloaded. A row
            // pointing at a path with nothing behind it would offer a Play
            // button that does nothing and hide the Download button that works.
            v.download_state = DownloadState::Done;
            v.downloaded_at = downloaded_at_or_mtime(archived_at, &p);
            v.file_path = Some(p.to_string_lossy().into_owned());
            report.downloads_relinked += 1;
        }
        done += 1;
        progress(done, total, &v.title);
        videos.push(v);
    }

    // 7. Hand the rows back. Nothing here has opened a database.
    Ok(Prepared { channels, videos, report, archive_channel_ids })
}

/// `downloaded_at` survives the relink, since it records when *you* fetched the
/// video and that is still true on the machine the file moved to. Falls back to
/// the file's own mtime for an archive whose row never had one — the same
/// seeding the v3 migration does, and for the same reason: a NULL here sorts
/// the row to the bottom of the Downloads tab forever.
fn downloaded_at_or_mtime(archived: Option<i64>, file: &Path) -> Option<i64> {
    archived.or_else(|| {
        let meta = std::fs::metadata(file).ok()?;
        let secs = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
        i64::try_from(secs).ok()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noop(_: usize, _: usize, _: &str) {}

    fn channel(id: &str, title: &str) -> Channel {
        Channel {
            id: id.into(),
            title: title.into(),
            handle: Some(format!("@{title}")),
            url: format!("https://www.youtube.com/channel/{id}"),
            thumb_path: None,
            subscribed: true,
            member: false,
            added_at: 1_700_000_000,
            last_polled_at: Some(1_758_000_000),
        }
    }

    /// A video with every field moved off its default, so a round-trip test can
    /// tell "carried" from "happened to match a default".
    fn video(id: &str, channel_id: &str) -> Video {
        Video {
            id: id.into(),
            channel_id: channel_id.into(),
            channel_title: "Exporting Machine's Join".into(),
            title: format!("Title of {id}"),
            description: Some("a description".into()),
            thumb_url: Some(format!("https://i.ytimg.com/vi/{id}/hq.jpg")),
            thumb_path: None,
            published_at: Some(1_757_000_000),
            sort_at: Some(1_757_000_001),
            feed_rank: 7,
            added_manually: true,
            duration_secs: Some(1234),
            view_count: Some(99_000),
            status: VideoStatus::Ready,
            hidden: true,
            watched: true,
            watched_at: Some(1_757_500_000),
            download_state: DownloadState::None,
            download_error: None,
            file_path: None,
            downloaded_at: None,
            first_seen_at: 1_756_000_000,
            sibling_group: Some("grp-1".into()),
        }
    }

    fn settings_with_download_dir(dir: &Path) -> Settings {
        let mut s = Settings::default();
        s.download_dir = dir.to_string_lossy().into_owned();
        s
    }

    /// A config directory for an import to land in, kept out of `~/.config`.
    fn dirs_in(root: &Path) -> Dirs {
        Dirs { thumbs: root.join("thumbs"), settings: root.join("settings.json") }
    }

    fn touch(path: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    // -------------------------------------------------------------- rel_path

    #[test]
    fn rel_path_is_measured_the_same_with_and_without_a_trailing_slash() {
        let file = "/mnt/STORAGE/Videos/Some Channel/A Title [abc].mkv";
        let bare = portable_path(file, "/mnt/STORAGE/Videos");
        let slashed = portable_path(file, "/mnt/STORAGE/Videos/");
        assert_eq!(bare, slashed);
        assert_eq!(bare, (Some("Some Channel/A Title [abc].mkv".into()), None));
    }

    #[test]
    fn rel_path_round_trips_through_a_whole_archive() {
        for download_dir in ["videos", "videos/"] {
            let tmp = tempfile::tempdir().unwrap();
            let dl = tmp.path().join("videos");
            let file = dl.join("Some Channel").join("A Title [v1].mkv");
            touch(&file, b"video bytes");

            let mut v = video("v1", "UC1");
            v.download_state = DownloadState::Done;
            v.file_path = Some(file.to_string_lossy().into_owned());
            v.downloaded_at = Some(1_757_900_000);

            // The trailing-slash variant is spelled onto the same real folder.
            let mut s = settings_with_download_dir(&dl);
            if download_dir.ends_with('/') {
                s.download_dir.push('/');
            }

            let zipped = tmp.path().join("a.zip");
            write_archive(&zipped, &[channel("UC1", "Chan")], &[v], &s, false, &noop).unwrap();

            let m = read_manifest(&mut open(&zipped).unwrap()).unwrap();
            assert_eq!(
                m.videos[0].rel_path.as_deref(),
                Some("Some Channel/A Title [v1].mkv"),
                "download_dir spelled {download_dir:?}"
            );
            assert_eq!(m.videos[0].abs_path, None);

            // Import without applying settings, so re-rooting runs against
            // *this* machine's download_dir rather than the archive's.
            let cfg = tmp.path().join("cfg");
            config::save_to(&cfg.join("settings.json"), &settings_with_download_dir(&dl)).unwrap();
            let p =
                prepare_import_in(&zipped, &["UC1".into()], false, &dirs_in(&cfg), &noop).unwrap();
            assert_eq!(p.videos[0].file_path.as_deref(), Some(&*file.to_string_lossy()));
            assert_eq!(p.videos[0].download_state, DownloadState::Done);
            assert_eq!(p.videos[0].downloaded_at, Some(1_757_900_000));
            assert_eq!(p.report.downloads_relinked, 1);
        }
    }

    #[test]
    fn a_file_outside_the_download_dir_stays_absolute_through_a_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let dl = tmp.path().join("videos");
        std::fs::create_dir_all(&dl).unwrap();
        let stray = tmp.path().join("elsewhere").join("Stray [v1].mkv");
        touch(&stray, b"video bytes");

        let mut v = video("v1", "UC1");
        v.download_state = DownloadState::Done;
        v.file_path = Some(stray.to_string_lossy().into_owned());

        let zipped = tmp.path().join("a.zip");
        let s = settings_with_download_dir(&dl);
        write_archive(&zipped, &[channel("UC1", "Chan")], &[v], &s, false, &noop).unwrap();

        let m = read_manifest(&mut open(&zipped).unwrap()).unwrap();
        assert_eq!(m.videos[0].rel_path, None);
        assert_eq!(m.videos[0].abs_path.as_deref(), Some(&*stray.to_string_lossy()));

        let cfg = tmp.path().join("cfg");
        config::save_to(&cfg.join("settings.json"), &s).unwrap();
        let p = prepare_import_in(&zipped, &["UC1".into()], false, &dirs_in(&cfg), &noop).unwrap();
        assert_eq!(p.videos[0].file_path.as_deref(), Some(&*stray.to_string_lossy()));
        assert_eq!(p.videos[0].download_state, DownloadState::Done);
    }

    // ---------------------------------------------------------- full manifest

    #[test]
    fn every_carried_field_survives_a_manifest_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let dl = tmp.path().join("videos");
        let file = dl.join("A Title [v1].mkv");
        touch(&file, b"video bytes");

        let thumb = tmp.path().join("their-thumbs").join("v1.jpg");
        touch(&thumb, b"jpeg bytes");

        let mut v = video("v1", "UC1");
        v.thumb_path = Some(thumb.to_string_lossy().into_owned());
        v.download_state = DownloadState::Done;
        v.file_path = Some(file.to_string_lossy().into_owned());
        v.downloaded_at = Some(1_757_900_000);

        let ch = channel("UC1", "Chan");
        let mut s = settings_with_download_dir(&dl);
        s.player_command = "mpv --fs".into();
        s.backfill_count = 77;
        s.window_width = 3840;
        s.view.search = "should not travel".into();

        let zipped = tmp.path().join("a.zip");
        write_archive(&zipped, &[ch.clone()], &[v.clone()], &s, true, &noop).unwrap();

        let m = read_manifest(&mut open(&zipped).unwrap()).unwrap();
        assert_eq!(m.format, FORMAT);
        assert_eq!(m.app_version, env!("CARGO_PKG_VERSION"));
        assert!(m.exported_at > 1_700_000_000);
        assert!(!m.exported_from.is_empty());
        assert!(m.includes_thumbs);
        assert_eq!(m.download_dir, s.download_dir);
        assert_eq!(m.channels, vec![ch.clone()], "a channel column was lost");

        // The portable half travels; the this-machine half is simply not there
        // to travel — `PortableSettings` has no field for it.
        assert_eq!(m.settings.player_command, "mpv --fs");
        assert_eq!(m.settings.backfill_count, 77);
        let raw = serde_json::to_string(&m.settings).unwrap();
        for absent in ["window_width", "window_height", "window_x", "window_y", "view"] {
            assert!(!raw.contains(absent), "{absent} travelled in the settings block");
        }

        let a = &m.videos[0];
        assert_eq!(a.id, v.id);
        assert_eq!(a.channel_id, v.channel_id);
        assert_eq!(a.title, v.title);
        assert_eq!(a.description, v.description);
        assert_eq!(a.thumb_url, v.thumb_url);
        assert_eq!(a.published_at, v.published_at);
        assert_eq!(a.sort_at, v.sort_at);
        assert_eq!(a.feed_rank, v.feed_rank);
        assert_eq!(a.added_manually, v.added_manually);
        assert_eq!(a.duration_secs, v.duration_secs);
        assert_eq!(a.view_count, v.view_count);
        assert_eq!(a.status, v.status);
        assert_eq!(a.hidden, v.hidden);
        assert_eq!(a.watched, v.watched);
        assert_eq!(a.watched_at, v.watched_at);
        assert_eq!(a.download_state, DownloadState::Done);
        assert_eq!(a.rel_path.as_deref(), Some("A Title [v1].mkv"));
        assert_eq!(a.downloaded_at, v.downloaded_at);
        assert_eq!(a.first_seen_at, v.first_seen_at);
        assert_eq!(a.sibling_group, v.sibling_group);

        // The three machine-specific columns are absent from the video rows
        // entirely, not merely null. (`channels` does carry its own
        // `thumb_path` column, which is why this looks at the videos alone.)
        let vraw = serde_json::to_string(&m.videos).unwrap();
        for absent in ["channel_title", "thumb_path", "file_path"] {
            assert!(!vraw.contains(absent), "{absent} was carried into the manifest");
        }
        assert!(vraw.contains("rel_path") && vraw.contains("abs_path"));

        // And back out again, with the two rebuilt columns rebuilt.
        let cfg = tmp.path().join("cfg");
        let p = prepare_import_in(&zipped, &["UC1".into()], true, &dirs_in(&cfg), &noop).unwrap();
        assert_eq!(p.channels, vec![ch]);
        let got = &p.videos[0];
        assert_eq!(got.channel_title, "Chan", "channel_title was not rebuilt from the join");
        assert_eq!(
            got.thumb_path.as_deref(),
            Some(&*cfg.join("thumbs").join("v1.jpg").to_string_lossy())
        );
        assert_eq!(got.file_path.as_deref(), Some(&*file.to_string_lossy()));
        assert_eq!(got.title, v.title);
        assert_eq!(got.sibling_group, v.sibling_group);
        assert_eq!(got.hidden, v.hidden);
        assert_eq!(got.watched_at, v.watched_at);
        assert_eq!(got.first_seen_at, v.first_seen_at);
        assert!(p.report.settings_applied);
        assert!(!p.report.download_dir_kept, "the archive's download_dir exists here");
        assert_eq!(p.report.thumbs_written, 1);

        // Settings landed, and the local window geometry survived them.
        let after = config::load_from(&cfg.join("settings.json")).unwrap();
        assert_eq!(after.player_command, "mpv --fs");
        assert_eq!(after.backfill_count, 77);
        assert_eq!(after.window_width, 1280, "the archive's window size was adopted");
        assert_eq!(after.view.search, "", "the archive's feed filters were adopted");
    }

    // ------------------------------------------------------------- versioning

    #[test]
    fn an_archive_from_a_newer_format_is_refused_by_name() {
        let tmp = tempfile::tempdir().unwrap();
        let zipped = tmp.path().join("a.zip");
        write_archive(
            &zipped,
            &[channel("UC1", "Chan")],
            &[],
            &settings_with_download_dir(tmp.path()),
            false,
            &noop,
        )
        .unwrap();

        // Rewrite the manifest with a bumped format and a field this build
        // could not parse, so a version check that ran *after* the full parse
        // would report the wrong problem.
        let future = tmp.path().join("future.zip");
        {
            let f = std::fs::File::create(&future).unwrap();
            let mut z = zip::ZipWriter::new(f);
            z.start_file(MANIFEST, SimpleFileOptions::default()).unwrap();
            z.write_all(
                format!(r#"{{"format":{},"videos":"reshaped in a later format"}}"#, FORMAT + 1)
                    .as_bytes(),
            )
            .unwrap();
            z.finish().unwrap();
        }

        let err = read_summary(&future, &HashSet::new()).unwrap_err().to_string();
        assert!(err.contains("newer version of MyTube"), "unhelpful message: {err}");
        assert!(err.contains(&(FORMAT + 1).to_string()), "the version is not named: {err}");
        assert!(prepare_import_in(
            &future,
            &["UC1".into()],
            true,
            &dirs_in(&tmp.path().join("cfg")),
            &noop
        )
        .is_err());

        // The current format, and anything below it, is fine.
        assert!(read_summary(&zipped, &HashSet::new()).is_ok());
    }

    // -------------------------------------------------------------- zip slip

    /// Builds an archive by hand so entry names this app would never write can
    /// be tested: the manifest knows one video, the archive carries three
    /// thumbnail-shaped entries and only one of them is legitimate.
    fn archive_with_hostile_entries(dest: &Path, id: &str, download_dir: &Path) {
        let m = Manifest {
            format: FORMAT,
            app_version: "0.1.0".into(),
            exported_at: 1_758_499_200,
            exported_from: "elsewhere".into(),
            download_dir: download_dir.to_string_lossy().into_owned(),
            includes_thumbs: true,
            settings: PortableSettings::of(&settings_with_download_dir(download_dir)),
            channels: vec![channel("UC1", "Chan")],
            videos: vec![ArchiveVideo::of(
                &video(id, "UC1"),
                &download_dir.to_string_lossy(),
            )],
        };
        let f = std::fs::File::create(dest).unwrap();
        let mut z = zip::ZipWriter::new(f);
        let opt = SimpleFileOptions::default();
        z.start_file(MANIFEST, opt).unwrap();
        z.write_all(&serde_json::to_vec(&m).unwrap()).unwrap();
        for name in [
            format!("{THUMB_PREFIX}{id}{THUMB_SUFFIX}"),
            "thumbs/../../evil.jpg".into(),
            "thumbs/unknown_id.jpg".into(),
            "thumbs/nested/deep.jpg".into(),
            "/etc/passwd.jpg".into(),
        ] {
            z.start_file(name, opt).unwrap();
            z.write_all(b"payload").unwrap();
        }
        z.finish().unwrap();
    }

    #[test]
    fn a_thumb_entry_escaping_the_thumbs_dir_is_skipped_and_writes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        let cfg = root.join("cfg");
        std::fs::create_dir_all(&cfg).unwrap();
        let dl = tmp.path().join("videos");
        std::fs::create_dir_all(&dl).unwrap();

        let zipped = tmp.path().join("a.zip");
        archive_with_hostile_entries(&zipped, "v1", &dl);

        let p = prepare_import_in(&zipped, &["UC1".into()], false, &dirs_in(&cfg), &noop).unwrap();

        // Exactly one entry was accepted: the one named for a manifest id.
        assert_eq!(p.report.thumbs_written, 1);
        let thumbs = cfg.join("thumbs");
        let written: Vec<String> = std::fs::read_dir(&thumbs)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(written, vec!["v1.jpg".to_string()]);

        // And nothing landed anywhere above it.
        assert!(!root.join("evil.jpg").exists());
        assert!(!tmp.path().join("evil.jpg").exists());
        assert!(!cfg.join("evil.jpg").exists());
        assert!(!thumbs.join("nested").exists());
    }

    #[test]
    fn a_thumb_entry_for_an_id_the_manifest_does_not_carry_is_skipped() {
        let wanted: HashSet<&str> = ["v1"].into_iter().collect();
        assert_eq!(thumb_entry_id("thumbs/v1.jpg", &wanted), Some("v1"));
        for hostile in [
            "thumbs/unknown_id.jpg",
            "thumbs/../../evil.jpg",
            "thumbs/../v1.jpg",
            "thumbs/nested/v1.jpg",
            "thumbs/.jpg",
            "thumbs/v1.jpeg",
            "manifest.json",
            "/thumbs/v1.jpg",
            "v1.jpg",
        ] {
            assert_eq!(thumb_entry_id(hostile, &wanted), None, "accepted {hostile}");
        }
    }

    #[test]
    fn a_thumbnail_already_cached_locally_is_not_overwritten() {
        let tmp = tempfile::tempdir().unwrap();
        let dl = tmp.path().join("videos");
        std::fs::create_dir_all(&dl).unwrap();
        let cfg = tmp.path().join("cfg");
        let mine = cfg.join("thumbs").join("v1.jpg");
        touch(&mine, b"the copy already here");

        let zipped = tmp.path().join("a.zip");
        archive_with_hostile_entries(&zipped, "v1", &dl);

        let p = prepare_import_in(&zipped, &["UC1".into()], false, &dirs_in(&cfg), &noop).unwrap();
        assert_eq!(std::fs::read(&mine).unwrap(), b"the copy already here");
        assert_eq!(p.report.thumbs_written, 0, "an existing thumbnail was rewritten");
        // It is still the row's thumbnail — the file is here either way.
        assert_eq!(p.videos[0].thumb_path.as_deref(), Some(&*mine.to_string_lossy()));
    }

    // ------------------------------------------------------- download states

    #[test]
    fn transient_and_failed_downloads_export_as_none_and_the_error_is_dropped() {
        let tmp = tempfile::tempdir().unwrap();
        let dl = tmp.path().join("videos");
        let file = dl.join("here.mkv");
        touch(&file, b"video bytes");

        let mut videos = Vec::new();
        for (i, state) in [
            DownloadState::Queued,
            DownloadState::Downloading,
            DownloadState::Failed,
            DownloadState::Done,
        ]
        .into_iter()
        .enumerate()
        {
            let mut v = video(&format!("v{i}"), "UC1");
            v.download_state = state;
            v.download_error = Some("ERROR: unable to download video data".into());
            v.file_path = Some(file.to_string_lossy().into_owned());
            v.downloaded_at = Some(1_757_900_000);
            videos.push(v);
        }

        let zipped = tmp.path().join("a.zip");
        let s = settings_with_download_dir(&dl);
        write_archive(&zipped, &[channel("UC1", "Chan")], &videos, &s, false, &noop).unwrap();

        let m = read_manifest(&mut open(&zipped).unwrap()).unwrap();
        for (i, a) in m.videos.iter().enumerate().take(3) {
            assert_eq!(a.download_state, DownloadState::None, "v{i} kept its state");
            assert_eq!(a.download_error, None, "v{i} carried another machine's error");
            assert_eq!(a.rel_path, None, "v{i} carried a path it never finished writing");
            assert_eq!(a.abs_path, None);
            assert_eq!(a.downloaded_at, None, "v{i} kept a queue position");
        }
        assert_eq!(m.videos[3].download_state, DownloadState::Done);
        assert_eq!(m.videos[3].rel_path.as_deref(), Some("here.mkv"));
        assert_eq!(m.videos[3].download_error, None);
    }

    #[test]
    fn re_rooting_relinks_a_file_that_is_here_and_clears_one_that_is_not() {
        let tmp = tempfile::tempdir().unwrap();
        let dl = tmp.path().join("videos");
        let here = dl.join("here.mkv");
        touch(&here, b"video bytes");

        let mut a = video("v1", "UC1");
        a.download_state = DownloadState::Done;
        a.file_path = Some(here.to_string_lossy().into_owned());
        a.downloaded_at = Some(1_757_900_000);
        let mut b = video("v2", "UC1");
        b.download_state = DownloadState::Done;
        b.file_path = Some(dl.join("gone.mkv").to_string_lossy().into_owned());
        b.downloaded_at = Some(1_757_900_001);

        let zipped = tmp.path().join("a.zip");
        let s = settings_with_download_dir(&dl);
        write_archive(&zipped, &[channel("UC1", "Chan")], &[a, b], &s, false, &noop).unwrap();

        // apply_settings, so the archive's download_dir — which exists here —
        // becomes the root the two rel_paths are rejoined to.
        let cfg = tmp.path().join("cfg");
        let p = prepare_import_in(&zipped, &["UC1".into()], true, &dirs_in(&cfg), &noop).unwrap();

        assert_eq!(p.videos[0].download_state, DownloadState::Done);
        assert_eq!(p.videos[0].file_path.as_deref(), Some(&*here.to_string_lossy()));
        assert!(Path::new(p.videos[0].file_path.as_deref().unwrap()).is_absolute());
        assert_eq!(p.videos[0].downloaded_at, Some(1_757_900_000));

        assert_eq!(p.videos[1].download_state, DownloadState::None);
        assert_eq!(p.videos[1].file_path, None);
        assert_eq!(p.videos[1].downloaded_at, None);
        assert_eq!(p.report.downloads_relinked, 1);
    }

    #[test]
    fn a_download_dir_that_is_not_here_leaves_this_machines_own_in_place() {
        let tmp = tempfile::tempdir().unwrap();
        let theirs = tmp.path().join("not-mounted-here");
        let mine = tmp.path().join("mine");
        std::fs::create_dir_all(&mine).unwrap();

        let zipped = tmp.path().join("a.zip");
        write_archive(
            &zipped,
            &[channel("UC1", "Chan")],
            &[],
            &settings_with_download_dir(&theirs),
            false,
            &noop,
        )
        .unwrap();

        let cfg = tmp.path().join("cfg");
        config::save_to(&cfg.join("settings.json"), &settings_with_download_dir(&mine)).unwrap();
        let p = prepare_import_in(&zipped, &["UC1".into()], true, &dirs_in(&cfg), &noop).unwrap();

        assert!(p.report.download_dir_kept);
        let after = config::load_from(&cfg.join("settings.json")).unwrap();
        assert_eq!(after.download_dir, mine.to_string_lossy());
    }

    // ------------------------------------------------------------- summaries

    #[test]
    fn read_summary_describes_the_archive_and_flags_what_is_already_here() {
        let tmp = tempfile::tempdir().unwrap();
        let dl = tmp.path().join("videos");
        std::fs::create_dir_all(&dl).unwrap();
        let thumb = tmp.path().join("their-thumbs").join("v1.jpg");
        touch(&thumb, b"jpeg bytes");

        let mut v1 = video("v1", "UC1");
        v1.thumb_path = Some(thumb.to_string_lossy().into_owned());
        let videos = vec![v1, video("v2", "UC1"), video("v3", "UC2")];
        let channels = vec![channel("UC1", "One"), channel("UC2", "Two")];

        let zipped = tmp.path().join("a.zip");
        write_archive(
            &zipped,
            &channels,
            &videos,
            &settings_with_download_dir(&dl),
            true,
            &noop,
        )
        .unwrap();

        let known: HashSet<String> = ["UC2".to_string()].into_iter().collect();
        let s = read_summary(&zipped, &known).unwrap();
        assert_eq!(s.format, FORMAT);
        assert_eq!(s.video_count, 3);
        assert_eq!(s.thumb_count, 1, "only one video had a thumbnail on disk");
        assert!(s.includes_thumbs);
        assert!(s.download_dir_exists);
        assert_eq!(s.download_dir, dl.to_string_lossy());
        assert_eq!(s.channels.len(), 2);
        assert_eq!(s.channels[0].channel_id, "UC1");
        assert_eq!(s.channels[0].video_count, 2);
        assert!(!s.channels[0].already_here);
        assert_eq!(s.channels[1].video_count, 1);
        assert!(s.channels[1].already_here);
    }

    #[test]
    fn read_summary_writes_nothing_anywhere() {
        let tmp = tempfile::tempdir().unwrap();
        let dl = tmp.path().join("videos");
        std::fs::create_dir_all(&dl).unwrap();
        let thumb = tmp.path().join("their-thumbs").join("v1.jpg");
        touch(&thumb, b"jpeg bytes");

        let mut v = video("v1", "UC1");
        v.thumb_path = Some(thumb.to_string_lossy().into_owned());

        let zipped = tmp.path().join("a.zip");
        write_archive(
            &zipped,
            &[channel("UC1", "Chan")],
            &[v],
            &settings_with_download_dir(&dl),
            true,
            &noop,
        )
        .unwrap();

        // A config directory with a settings file and an empty thumbnail cache,
        // exactly as an import would find it.
        let cfg = tmp.path().join("cfg");
        let dirs = dirs_in(&cfg);
        std::fs::create_dir_all(&dirs.thumbs).unwrap();
        config::save_to(&dirs.settings, &settings_with_download_dir(&dl)).unwrap();
        let before = std::fs::read(&dirs.settings).unwrap();

        read_summary(&zipped, &HashSet::new()).unwrap();

        assert_eq!(std::fs::read(&dirs.settings).unwrap(), before, "settings.json was touched");
        assert_eq!(std::fs::read_dir(&dirs.thumbs).unwrap().count(), 0, "a thumbnail was written");
        // And the real one, which `read_summary` has no parameter for and so
        // must never have reached either.
        assert!(!config::thumbs_dir().join("v1.jpg").exists());
    }

    #[test]
    fn a_channel_that_was_not_picked_takes_its_videos_with_it() {
        let tmp = tempfile::tempdir().unwrap();
        let dl = tmp.path().join("videos");
        std::fs::create_dir_all(&dl).unwrap();
        let zipped = tmp.path().join("a.zip");
        write_archive(
            &zipped,
            &[channel("UC1", "One"), channel("UC2", "Two")],
            &[video("v1", "UC1"), video("v2", "UC2"), video("v3", "UC2")],
            &settings_with_download_dir(&dl),
            false,
            &noop,
        )
        .unwrap();

        let cfg = tmp.path().join("cfg");
        let p = prepare_import_in(&zipped, &["UC2".into()], false, &dirs_in(&cfg), &noop).unwrap();
        assert_eq!(p.channels.len(), 1);
        assert_eq!(p.channels[0].id, "UC2");
        assert_eq!(p.videos.len(), 2);
        assert!(p.videos.iter().all(|v| v.channel_title == "Two"));
        assert!(!p.report.settings_applied);
    }

    #[test]
    fn progress_counts_up_to_the_total_it_reported() {
        use std::sync::Mutex;
        let tmp = tempfile::tempdir().unwrap();
        let dl = tmp.path().join("videos");
        std::fs::create_dir_all(&dl).unwrap();
        let thumb = tmp.path().join("their-thumbs").join("v1.jpg");
        touch(&thumb, b"jpeg bytes");
        let mut v = video("v1", "UC1");
        v.thumb_path = Some(thumb.to_string_lossy().into_owned());

        let seen: Mutex<Vec<(usize, usize)>> = Mutex::new(Vec::new());
        let tick = |done: usize, total: usize, _: &str| {
            seen.lock().unwrap().push((done, total));
        };

        let zipped = tmp.path().join("a.zip");
        write_archive(
            &zipped,
            &[channel("UC1", "Chan")],
            &[v, video("v2", "UC1")],
            &settings_with_download_dir(&dl),
            true,
            &tick,
        )
        .unwrap();
        let last = *seen.lock().unwrap().last().unwrap();
        assert_eq!(last, (2, 2), "export progress did not finish: {:?}", seen.lock().unwrap());

        seen.lock().unwrap().clear();
        let cfg = tmp.path().join("cfg");
        prepare_import_in(&zipped, &["UC1".into()], false, &dirs_in(&cfg), &tick).unwrap();
        let log = seen.lock().unwrap().clone();
        assert_eq!(log.last().copied(), Some((3, 3)), "import progress did not finish: {log:?}");
        assert!(log.windows(2).all(|w| w[0].0 <= w[1].0), "progress went backwards: {log:?}");
    }

    #[test]
    fn estimate_counts_only_thumbnails_that_are_actually_on_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let there = tmp.path().join("thumbs").join("v1.jpg");
        touch(&there, b"0123456789");

        let mut a = video("v1", "UC1");
        a.thumb_path = Some(there.to_string_lossy().into_owned());
        let mut b = video("v2", "UC1");
        b.thumb_path = Some(tmp.path().join("thumbs").join("gone.jpg").to_string_lossy().into());
        let c = video("v3", "UC1");

        let est = estimate(&[channel("UC1", "Chan")], &[a, b, c]);
        assert_eq!(est.channel_count, 1);
        assert_eq!(est.video_count, 3);
        assert_eq!(est.thumb_count, 1);
        assert_eq!(est.thumb_bytes, 10);
    }

    #[test]
    fn the_default_archive_name_carries_the_date() {
        let n = default_archive_name();
        assert!(n.starts_with("mytube-export-"), "{n}");
        assert!(n.ends_with(".zip"), "{n}");
        assert_eq!(n.len(), "mytube-export-2026-09-22.zip".len(), "{n}");
    }

    #[test]
    fn a_zip_without_a_manifest_is_refused_as_not_a_mytube_archive() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("holiday-photos.zip");
        {
            let f = std::fs::File::create(&path).unwrap();
            let mut z = zip::ZipWriter::new(f);
            z.start_file("IMG_0001.jpg", SimpleFileOptions::default()).unwrap();
            z.write_all(b"jpeg").unwrap();
            z.finish().unwrap();
        }
        let err = read_summary(&path, &HashSet::new()).unwrap_err().to_string();
        assert!(err.contains("not a MyTube archive"), "{err}");
    }
}
