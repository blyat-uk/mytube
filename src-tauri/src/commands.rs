use std::sync::Arc;
use futures::stream::StreamExt;
use tauri::{AppHandle, Emitter, State};

use crate::db::Db;
use crate::models::*;
use crate::poll::{self, AppState};
use crate::quality::Quality;
use crate::{app_update, config, player, resolve, transfer, ytdlp};

/// Backfills run a few at a time: enough to hide latency, few enough
/// to stay clear of YouTube rate limiting.
const IMPORT_CONCURRENCY: usize = 3;

type R<T> = Result<T, String>;
fn e<E: std::fmt::Display>(err: E) -> String {
    err.to_string()
}

/// Runs blocking work -- SQL, the filesystem, settings.json, spawning or
/// probing a binary -- on tokio's blocking pool.
///
/// Every command here is `async` for this reason. Tauri 2 runs a plain
/// `#[tauri::command] pub fn` on the main thread, which on Linux is also the
/// thread driving the webview, so any wait inside one froze the whole window:
/// a download disk spinning up, or the `Db` Mutex held by a poll, an import or
/// a grouping walk for as long as that takes. An `async fn` alone is not
/// enough either -- blocking inline there parks a runtime worker, stalling
/// every other future scheduled on it, progress events included.
///
/// The closure is `'static`, so it owns everything it touches: nothing
/// borrowed survives into the await, which is what `generate_handler!`'s
/// higher-ranked bounds insist on (see "Async gotchas" in CLAUDE.md). A panic
/// inside comes back as an `Err`, not a torn-down command.
async fn blocking<T, F>(f: F) -> R<T>
where
    T: Send + 'static,
    F: FnOnce() -> anyhow::Result<T> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(e)?
        .map_err(e)
}

/// [`blocking`] with the database in hand. Deliberately not an `async fn`:
/// the `Arc` is cloned before the future is built, so the command's `State`
/// borrow ends here rather than being captured and held across the await.
fn db_blocking<T, F>(state: &AppState, f: F) -> impl std::future::Future<Output = R<T>> + Send + 'static
where
    T: Send + 'static,
    F: FnOnce(&Db) -> anyhow::Result<T> + Send + 'static,
{
    let db = state.db.clone();
    blocking(move || f(&db))
}

#[tauri::command]
pub async fn get_settings() -> R<config::Settings> {
    blocking(config::load).await
}

#[tauri::command]
pub async fn save_settings(
    settings: config::Settings,
    state: State<'_, Arc<AppState>>,
    app: AppHandle,
) -> R<()> {
    let (settings, disk) = blocking(move || {
        let mut settings = settings;
        // The Settings view sends a snapshot taken at mount, so if a filter
        // changed while it was open, saving it verbatim would revert `view` to
        // that stale copy. A failed re-read (e.g. no file yet) just means there
        // is nothing on disk to defer to, so fall through and write as sent.
        let disk = config::load().ok();
        if let Some(disk) = &disk {
            settings.keep_view_of(disk);
            // The tool overrides have no UI, only hand edits, so the snapshot's
            // copy can only ever be stale; see `keep_machine_overrides_of`.
            settings.keep_machine_overrides_of(disk);
        }
        config::save(&settings)?;
        Ok((settings, disk))
    })
    .await?;
    // A new update channel or auto-update switched on should take effect now
    // rather than at the next hourly tools tick. The override paths cannot
    // differ from the file's any more; a hand edit to one is picked up by the
    // next resolution, which is keyed on them.
    let tools_changed = disk.as_ref().is_none_or(|d| {
        d.ytdlp_channel != settings.ytdlp_channel
            || d.ytdlp_auto_update != settings.ytdlp_auto_update
    });
    if tools_changed {
        let tools = state.tools.clone();
        let s = settings.clone();
        tauri::async_runtime::spawn(async move {
            tools.ensure_all(&s).await;
            tools.maybe_update(&s).await;
        });
    }
    // Likewise the release check: switching it off should take the tray item
    // away now, and switching it on should look now, not up to an hour later.
    // `tick` still gates the request itself on `due`.
    if disk.as_ref().is_none_or(|d| d.check_app_updates != settings.check_app_updates) {
        let http = state.http.clone();
        let s = settings.clone();
        tauri::async_runtime::spawn(async move {
            crate::app_update::tick(&app, &http, &s).await;
        });
    }
    state
        .queue
        .set_concurrency(settings.max_concurrent_downloads)
        .await;
    Ok(())
}

/// Writes only the feed's filters, re-reading the file first so a settings
/// edit or the window geometry is never overwritten by a filter change --
/// the same re-read-then-touch-one-block pattern `window::persist` uses to
/// save geometry without disturbing the rest of the file.
#[tauri::command]
pub async fn save_view_state(mut view: config::ViewState) -> R<()> {
    // `Settings::from_json_str` only sanitises on the way back in, so without
    // this an unbounded search string (no `maxLength` on the search box)
    // would be written to settings.json in full every 400ms of typing.
    // Sanitising here also means the stored block is always byte-for-byte
    // what the next launch's `sanitize` call would produce anyway.
    view.sanitize();
    blocking(move || {
        let mut s = config::load()?;
        s.view = view;
        config::save(&s)
    })
    .await
}

#[tauri::command]
pub async fn list_channels(state: State<'_, Arc<AppState>>) -> R<Vec<Channel>> {
    // Only subscriptions: ad-hoc uploaders are not part of the user's channel list.
    db_blocking(&state, |db| db.list_subscribed_channels()).await
}

/// Tells the UI whether the Add box holds a channel, a video, or a rejected Short.
#[tauri::command]
pub fn classify_add_input(input: String) -> R<AddKind> {
    match resolve::parse_add_input(&input).map_err(e)? {
        resolve::AddTarget::Channel(_) => Ok(AddKind::Channel),
        resolve::AddTarget::Video(_) => Ok(AddKind::Video),
        resolve::AddTarget::Short(_) => Ok(AddKind::Short),
    }
}

/// Adds one video without subscribing, then downloads it immediately.
/// It sorts to the top of the grid by `sort_at`, while its card still shows
/// the real upload date.
#[tauri::command]
pub async fn add_video(input: String, state: State<'_, Arc<AppState>>) -> R<Video> {
    let video_id = match resolve::parse_add_input(&input).map_err(e)? {
        resolve::AddTarget::Video(id) => id,
        resolve::AddTarget::Short(_) => {
            return Err("MyTube does not handle YouTube Shorts.".into())
        }
        resolve::AddTarget::Channel(_) => {
            return Err("That is a channel link. Use Add channel to subscribe.".into())
        }
    };
    let (s, known) = {
        let id = video_id.clone();
        db_blocking(&state, move |db| Ok((config::load()?, db.get_video(&id)?.is_some()))).await?
    };

    if !known {
        // The lease lives exactly as long as the probe; the download that
        // follows takes its own in the queue.
        let inv = state.tools.ytdlp(&s).await.map_err(e)?;
        let probe = ytdlp::probe(
            &inv,
            &s.quality,
            &ytdlp::watch_url(&video_id),
            &s.download_dir,
            &s.filename_template,
        )
        .await
        .map_err(e)?;
        drop(inv);

        let channel_id = if probe.channel_id.is_empty() {
            "UC000000000000000000000".to_string()
        } else {
            probe.channel_id.clone()
        };

        // subscribed = false: keeps the uploader's name on the card without
        // adding them to the subscription list or any poll cycle.
        let channel = Channel {
            id: channel_id.clone(),
            title: if probe.channel_title.is_empty() {
                "Added manually".into()
            } else {
                probe.channel_title.clone()
            },
            handle: None,
            url: format!("https://www.youtube.com/channel/{channel_id}"),
            thumb_path: None,
            subscribed: false,
            member: false,
            added_at: chrono::Utc::now().timestamp(),
            last_polled_at: None,
            terminated: false, auto_download: false,
        };
        let video = NewVideo {
            id: video_id.clone(),
            channel_id,
            title: probe.title.clone(),
            description: None,
            thumb_url: Some(ytdlp::thumb_url_for(&video_id)),
            published_at: probe.published_at, // true upload date, for display
            sort_at: Some(chrono::Utc::now().timestamp()), // top of the grid
            feed_rank: 0,
            added_manually: true,
            duration_secs: probe.duration_secs,
            view_count: None,
            status: VideoStatus::Ready,
        };
        db_blocking(&state, move |db| {
            db.upsert_channel(&channel)?;
            db.insert_video_if_new(&video)
        })
        .await?;

        poll::cache_thumb(
            &state.http,
            &state.db,
            &video_id,
            &ytdlp::thumb_url_for(&video_id),
        )
        .await;
    }

    state
        .queue
        .enqueue(video_id.clone(), s.download_dir, s.filename_template)
        .await
        .map_err(e)?;
    db_blocking(&state, move |db| db.get_video(&video_id))
        .await?
        .ok_or_else(|| "Video disappeared after being added".to_string())
}

#[tauri::command]
pub async fn add_channel(
    input: String,
    state: State<'_, Arc<AppState>>,
    app: AppHandle,
) -> R<Channel> {
    if let resolve::AddTarget::Short(_) = resolve::parse_add_input(&input).map_err(e)? {
        return Err("MyTube does not handle YouTube Shorts.".into());
    }
    let id = resolve::resolve(&state.http, &input).await.map_err(e)?;
    // The settings are read in the same trip but only unwrapped past the early
    // return, so an unreadable settings.json never fails a channel already here.
    let (existing, settings) = {
        let id = id.clone();
        db_blocking(&state, move |db| Ok((db.get_channel(&id)?, config::load()))).await?
    };
    if let Some(existing) = existing {
        if existing.subscribed {
            return Ok(existing);
        }
    }

    let settings = settings.map_err(e)?;
    let channel = Channel {
        id: id.clone(),
        title: id.clone(),
        handle: None,
        url: format!("https://www.youtube.com/channel/{id}"),
        thumb_path: None,
        subscribed: true,
        member: false,
        added_at: chrono::Utc::now().timestamp(),
        last_polled_at: None,
        terminated: false, auto_download: false,
    };
    {
        let channel = channel.clone();
        db_blocking(&state, move |db| db.upsert_channel(&channel)).await?;
    }

    // Backfill history, then poll RSS so the recent window gets real dates.
    if let Err(err) = poll::backfill_channel(&state, &id, settings.backfill_count).await {
        eprintln!("backfill failed for {id}: {err}");
    }
    poll::poll_channels(&state, &app, vec![channel]).await;

    db_blocking(&state, move |db| db.get_channel(&id))
        .await?
        .ok_or_else(|| "Channel disappeared after being added".to_string())
}

#[tauri::command]
pub async fn remove_channel(channel_id: String, state: State<'_, Arc<AppState>>) -> R<()> {
    db_blocking(&state, move |db| db.remove_channel(&channel_id)).await
}

/// Parses the CSV without importing anything, so the UI can offer a checklist.
#[tauri::command]
pub async fn preview_takeout_csv(
    path: String,
    state: State<'_, Arc<AppState>>,
) -> R<Vec<TakeoutRow>> {
    db_blocking(&state, move |db| {
        let data = std::fs::read_to_string(&path)?;
        let rows = resolve::parse_takeout_csv(&data)?;
        let subscribed: std::collections::HashSet<String> = db
            .list_subscribed_channels()?
            .into_iter()
            .map(|c| c.id)
            .collect();
        Ok(rows
            .into_iter()
            .map(|(channel_id, title)| TakeoutRow {
                already_subscribed: subscribed.contains(&channel_id),
                title: if title.is_empty() { channel_id.clone() } else { title },
                channel_id,
            })
            .collect())
    })
    .await
}

/// Imports only the channels the user ticked. Backfills run a few at a time and
/// report progress, rather than blocking on one long opaque spinner.
#[tauri::command]
pub async fn import_takeout_csv(
    path: String,
    channel_ids: Vec<String>,
    state: State<'_, Arc<AppState>>,
    app: AppHandle,
) -> R<ImportResult> {
    // The file read and one upsert per ticked row, off the runtime: a Takeout
    // export can list hundreds of channels, each its own write.
    let (settings, total, mut result, to_backfill) = db_blocking(&state, move |db| {
        let data = std::fs::read_to_string(&path)?;
        let all = resolve::parse_takeout_csv(&data)?;
        let wanted: std::collections::HashSet<String> = channel_ids.into_iter().collect();
        let rows: Vec<(String, String)> =
            all.into_iter().filter(|(id, _)| wanted.contains(id)).collect();

        let settings = config::load()?;
        let total = rows.len();
        let mut result = ImportResult::default();
        let mut to_backfill: Vec<(String, String)> = Vec::new();

        for (id, title) in rows {
            if db.get_channel(&id)?.map(|c| c.subscribed).unwrap_or(false) {
                result.skipped += 1;
                continue;
            }
            let channel = Channel {
                id: id.clone(),
                title: if title.is_empty() { id.clone() } else { title.clone() },
                handle: None,
                url: format!("https://www.youtube.com/channel/{id}"),
                thumb_path: None,
                subscribed: true,
                member: false,
                added_at: chrono::Utc::now().timestamp(),
                last_polled_at: None,
                terminated: false, auto_download: false,
            };
            if let Err(err) = db.upsert_channel(&channel) {
                result.failed.push(format!("{title}: {err}"));
                continue;
            }
            to_backfill.push((id, channel.title));
        }
        Ok((settings, total, result, to_backfill))
    })
    .await?;

    let done = std::sync::atomic::AtomicUsize::new(0);
    let failures: tokio::sync::Mutex<Vec<String>> = tokio::sync::Mutex::new(Vec::new());
    let inner = state.inner().clone();

    futures::stream::iter(to_backfill.iter())
        .for_each_concurrent(IMPORT_CONCURRENCY, |(id, title)| {
            let inner = inner.clone();
            let done = &done;
            let failures = &failures;
            let app = app.clone();
            async move {
                if let Err(err) =
                    poll::backfill_channel(&inner, id, settings.backfill_count).await
                {
                    failures.lock().await.push(format!("{title}: {err}"));
                }
                let n = done.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                let _ = app.emit(
                    "import://progress",
                    ImportProgress { done: n, total, current: title.clone() },
                );
            }
        })
        .await;

    let failed = failures.into_inner();
    result.added = to_backfill.len() - failed.len();
    result.failed.extend(failed);

    let channels = db_blocking(&state, |db| db.list_subscribed_channels()).await?;
    poll::poll_channels(&state, &app, channels).await;
    Ok(result)
}

/// Hides a video so polling never surfaces it again.
#[tauri::command]
pub async fn set_video_hidden(
    video_id: String,
    hidden: bool,
    state: State<'_, Arc<AppState>>,
) -> R<()> {
    db_blocking(&state, move |db| db.set_hidden(&video_id, hidden)).await
}

/// Removes a manually added video outright. Refuses subscription videos, which
/// would simply reappear on the next poll — those must be hidden instead.
#[tauri::command]
pub async fn delete_video(video_id: String, state: State<'_, Arc<AppState>>) -> R<()> {
    db_blocking(&state, move |db| {
        let v = db
            .get_video(&video_id)?
            .ok_or_else(|| anyhow::anyhow!("Unknown video"))?;
        if !v.added_manually {
            anyhow::bail!("Only manually added videos can be deleted. Hide this one instead.");
        }
        db.delete_video(&video_id)?;
        db.prune_orphan_channel(&v.channel_id)
    })
    .await
}

#[tauri::command]
pub async fn poll_all(state: State<'_, Arc<AppState>>, app: AppHandle) -> R<PollSummary> {
    let channels = db_blocking(&state, |db| db.list_subscribed_channels()).await?;
    Ok(poll::poll_channels(&state, &app, channels).await)
}

#[tauri::command]
pub async fn poll_channel(
    channel_id: String,
    state: State<'_, Arc<AppState>>,
    app: AppHandle,
) -> R<PollSummary> {
    let c = {
        let id = channel_id.clone();
        db_blocking(&state, move |db| db.get_channel(&id)).await?
    }
    .ok_or_else(|| format!("No such channel: {channel_id}"))?;
    Ok(poll::poll_channels(&state, &app, vec![c]).await)
}

/// Records whether you have joined this channel's membership, and returns how
/// many members-only uploads that turned up.
///
/// Joining reads one listing at backfill depth there and then. The poll's own
/// window is [`poll::TITLE_REFRESH_LIMIT`] entries and a membership backlog
/// routinely reaches past it, so anything older would otherwise never be seen
/// at all -- it has already slid out of the window by the time you join.
/// Leaving a membership removes nothing: those videos are yours to keep, the
/// same way an unsubscribed channel's are.
#[tauri::command]
pub async fn set_channel_member(
    channel_id: String,
    member: bool,
    state: State<'_, Arc<AppState>>,
) -> R<usize> {
    let depth = {
        let id = channel_id.clone();
        db_blocking(&state, move |db| {
            if db.get_channel(&id)?.is_none() {
                anyhow::bail!("No such channel: {id}");
            }
            db.set_channel_member(&id, member)?;
            Ok(config::load().map(|s| s.backfill_count).unwrap_or(30))
        })
        .await?
    };
    if !member {
        return Ok(0);
    }
    poll::ingest_members_only(&state, &channel_id, depth).await.map_err(e)
}

/// Which of a channel's existing videos "everything so far as well" takes,
/// when auto-download is switched on.
#[derive(Debug, Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Backlog {
    pub include_watched: bool,
    pub include_hidden: bool,
}

/// How many videos a switch-on with this backlog would queue. Read-only: it is
/// the live count the prompt shows while the ticks move.
#[tauri::command]
pub async fn auto_download_backlog_count(
    channel_id: String,
    include_watched: bool,
    include_hidden: bool,
    state: State<'_, Arc<AppState>>,
) -> R<usize> {
    db_blocking(&state, move |db| {
        db.auto_download_backlog(&channel_id, include_watched, include_hidden)
            .map(|ids| ids.len())
    })
    .await
}

/// Switches auto-download on or off for one channel, and returns how many
/// backlog videos were queued.
///
/// Off is immediate and removes nothing: queued and running downloads carry
/// on, and nothing on disk is touched. On with `backlog` also queues the
/// videos already in the library that it selects, flagged `auto_queued` like
/// anything the sweep takes. The library only -- no listing is read here.
#[tauri::command]
pub async fn set_channel_auto_download(
    channel_id: String,
    enabled: bool,
    backlog: Option<Backlog>,
    state: State<'_, Arc<AppState>>,
) -> R<usize> {
    let queue = db_blocking(&state, move |db| {
        if db.get_channel(&channel_id)?.is_none() {
            anyhow::bail!("No such channel: {channel_id}");
        }
        db.set_channel_auto_download(&channel_id, enabled)?;
        let Some(b) = backlog.filter(|_| enabled) else { return Ok(None) };
        let ids = db.auto_download_backlog(&channel_id, b.include_watched, b.include_hidden)?;
        if ids.is_empty() {
            return Ok(None);
        }
        Ok(Some((ids, config::load()?)))
    })
    .await?;
    let Some((ids, s)) = queue else { return Ok(0) };
    Ok(poll::queue_auto(&state, ids, &s.download_dir, &s.filename_template).await)
}

#[tauri::command]
pub async fn list_videos(filter: VideoFilter, state: State<'_, Arc<AppState>>) -> R<Vec<Video>> {
    db_blocking(&state, move |db| db.list_videos(&filter)).await
}

/// The same feed, with each channel's multi-part uploads behind one card.
/// `filter.limit` and `filter.offset` count groups here, not videos.
#[tauri::command]
pub async fn list_video_groups(
    filter: VideoFilter,
    state: State<'_, Arc<AppState>>,
) -> R<Vec<VideoGroup>> {
    db_blocking(&state, move |db| db.list_video_groups(&filter)).await
}

/// Marks videos siblings by hand, for a series whose titles no matcher could
/// ever join -- a renamed follow-up, most often. Returns the finished group's
/// size, which can exceed what was sent: marking across two hand-built groups
/// merges both. Refused across channels.
#[tauri::command]
pub async fn mark_siblings(video_ids: Vec<String>, state: State<'_, Arc<AppState>>) -> R<usize> {
    db_blocking(&state, move |db| db.mark_siblings(&video_ids)).await
}

/// Takes one video back out of its hand-built group, dissolving the group when
/// that leaves it with a single member.
#[tauri::command]
pub async fn unlink_siblings(video_id: String, state: State<'_, Arc<AppState>>) -> R<()> {
    db_blocking(&state, move |db| db.unlink_siblings(&video_id)).await
}

#[tauri::command]
pub async fn set_watched(
    video_id: String,
    watched: bool,
    state: State<'_, Arc<AppState>>,
) -> R<()> {
    db_blocking(&state, move |db| db.set_watched(&video_id, watched)).await
}

/// `quality` is "Download (custom)…": `Some` stores it on the row first, so
/// the job -- which reads it when it starts -- and any Retry after a failure
/// use it. `None` is a plain Download and leaves a stored one alone; the row
/// only carries one while it is not back at `none` (see the v9 migration).
///
/// A video already queued or downloading is left exactly as it is: its job
/// has settled (or will settle) its own quality, and storing a different one
/// under it would make the row claim a quality the file was not fetched at.
#[tauri::command]
pub async fn enqueue_download(
    video_id: String,
    quality: Option<Quality>,
    state: State<'_, Arc<AppState>>,
) -> R<()> {
    let s = blocking(config::load).await?;
    if let Some(q) = quality {
        if state.queue.is_active(&video_id).await {
            return Ok(());
        }
        let id = video_id.clone();
        db_blocking(&state, move |db| db.set_download_quality(&id, Some(&q.sanitized()))).await?;
    }
    state
        .queue
        .enqueue(video_id, s.download_dir, s.filename_template)
        .await
        .map_err(e)
}

/// Every format one video offers, for the "Download (custom)…" dialog, and
/// which of them the Settings default would pick. Runs with the same cookies,
/// deno and ffmpeg a download would, under a tools lease.
#[tauri::command]
pub async fn probe_formats(
    video_id: String,
    state: State<'_, Arc<AppState>>,
) -> R<ytdlp::VideoFormats> {
    let s = blocking(config::load).await?;
    let inv = state.tools.ytdlp(&s).await.map_err(e)?;
    ytdlp::formats(&inv, &video_id, &s.quality).await.map_err(e)
}

#[tauri::command]
pub async fn cancel_download(video_id: String, state: State<'_, Arc<AppState>>) -> R<()> {
    state.queue.cancel(&video_id).await.map_err(e)
}

#[tauri::command]
pub async fn open_in_player(video_id: String, state: State<'_, Arc<AppState>>) -> R<()> {
    // All of it off the main thread: the existence check is the first touch of
    // the download disk, and a sleeping one takes seconds to answer.
    db_blocking(&state, move |db| {
        let v = db
            .get_video(&video_id)?
            .ok_or_else(|| anyhow::anyhow!("Unknown video"))?;
        let path = v
            .file_path
            .ok_or_else(|| anyhow::anyhow!("This video has not been downloaded"))?;
        // The one existence check on this path, here rather than in
        // `player::launch`, because only the caller can mark the row.
        if !std::path::Path::new(&path).exists() {
            db.clear_file_path(&video_id)?;
            anyhow::bail!("File is gone: {path}. Marked as not downloaded.");
        }
        let s = config::load()?;
        player::start(&s.player_command, &path)
    })
    .await
}

/// Forgets a download at once and unlinks the file afterwards.
///
/// The row is cleared and `download://state` sent before the file is touched,
/// so every view -- not only the one that asked -- drops the card's
/// downloaded look immediately. The unlink follows on the blocking pool: on a
/// spun-down disk it can take seconds, and nothing the user sees depends on
/// it. A failure there is only logged; the row is already `none` either way,
/// the same outcome the old unlink-then-clear order reached when the unlink
/// failed.
#[tauri::command]
pub async fn delete_download(
    video_id: String,
    state: State<'_, Arc<AppState>>,
    app: AppHandle,
) -> R<()> {
    let id = video_id.clone();
    let path = db_blocking(&state, move |db| {
        let v = db
            .get_video(&id)?
            .ok_or_else(|| anyhow::anyhow!("Unknown video"))?;
        db.clear_file_path(&id)?;
        Ok(v.file_path)
    })
    .await?;
    crate::queue::emit_state(&app, &video_id, DownloadState::None, None, None);
    if let Some(p) = path {
        tauri::async_runtime::spawn_blocking(move || {
            // Already gone is the outcome wanted, not a failure worth a line.
            match std::fs::remove_file(&p) {
                Err(err) if err.kind() != std::io::ErrorKind::NotFound => {
                    eprintln!("mytube: could not delete {p}: {err}")
                }
                _ => {}
            }
        });
    }
    Ok(())
}

// ---- config transfer ----

/// What an export would weigh, so the "Include thumbnails" tick can offer a
/// real number instead of a guess.
#[tauri::command]
pub async fn transfer_estimate(state: State<'_, Arc<AppState>>) -> R<TransferEstimate> {
    db_blocking(&state, |db| {
        let channels = db.export_channels()?;
        let videos = db.export_videos()?;
        // `transfer` owns the sizing because it owns what actually goes in the
        // zip: a thumbnail is counted only if the file is really there to be
        // copied -- one stat per thumbnail, which is why this is off the runtime.
        Ok(transfer::estimate(&channels, &videos))
    })
    .await
}

#[tauri::command]
pub async fn export_config(
    path: String,
    include_thumbs: bool,
    state: State<'_, Arc<AppState>>,
    app: AppHandle,
) -> R<()> {
    let dest = std::path::PathBuf::from(path);

    // Zipping thousands of thumbnails is blocking work. On the async runtime's
    // own threads it would stall every other command behind it -- including the
    // progress events this very call is emitting. The reads that feed it go
    // with it: every row in the library, under the shared connection lock.
    db_blocking(&state, move |db| {
        let channels = db.export_channels()?;
        let videos = db.export_videos()?;
        let settings = config::load()?;
        transfer::write_archive(
            &dest,
            &channels,
            &videos,
            &settings,
            include_thumbs,
            &|done, total, current| {
                let _ = app.emit(
                    "transfer://progress",
                    TransferProgress {
                        phase: TransferPhase::Export,
                        done,
                        total,
                        current: current.to_string(),
                    },
                );
            },
        )
    })
    .await
}

/// Parses the archive's manifest without writing anything, so the dialog can
/// show what is inside before anything is committed to.
#[tauri::command]
pub async fn read_archive(
    path: String,
    state: State<'_, Arc<AppState>>,
) -> R<ArchiveSummary> {
    let p = std::path::PathBuf::from(path);
    db_blocking(&state, move |db| {
        // Every channel, not just the subscribed ones: an ad-hoc uploader
        // already here is "already here", and saying otherwise would offer to
        // re-add it.
        let known: std::collections::HashSet<String> =
            db.export_channels()?.into_iter().map(|c| c.id).collect();
        let mut summary = transfer::read_summary(&p, &known)?;

        // What a Replace would remove, measured here because only the database
        // can answer it. Against the archive's whole roster, never the ticked
        // subset: unticking a row means "skip it", so the number the dialog
        // shows must not move as the user works down the checklist.
        let roster: Vec<String> =
            summary.channels.iter().map(|c| c.channel_id.clone()).collect();
        let (local_only_channels, local_only_videos) = db.absent_from(&roster)?;
        summary.local_only_channels = local_only_channels;
        summary.local_only_videos = local_only_videos;
        Ok(summary)
    })
    .await
}

#[tauri::command]
pub async fn import_config(
    path: String,
    channel_ids: Vec<String>,
    mode: ImportMode,
    apply_settings: bool,
    state: State<'_, Arc<AppState>>,
    app: AppHandle,
) -> R<ImportReport> {
    let p = std::path::PathBuf::from(path);
    let emitter = app.clone();

    // Everything outside the database: settings, thumbnails, path re-rooting.
    let prepared = blocking(move || {
        transfer::prepare_import(&p, &channel_ids, apply_settings, &|done, total, current| {
            let _ = emitter.emit(
                "transfer://progress",
                TransferProgress {
                    phase: TransferPhase::Import,
                    done,
                    total,
                    current: current.to_string(),
                },
            );
        })
    })
    .await?;

    // And now the part that has to be atomic. One `Db` call, one transaction:
    // the connection Mutex is not reentrant, so a loop over several public
    // methods was never available, and a failure here must leave the library
    // exactly as it was. Off the runtime: thousands of rows in one transaction
    // is the longest the connection lock is ever held.
    let (mut report, prepared) = db_blocking(&state, move |db| {
        let report = db.apply_import(
            &prepared.channels,
            &prepared.videos,
            mode,
            &prepared.archive_channel_ids,
        )?;
        Ok((report, prepared))
    })
    .await?;

    // `db` knows the row counts; `transfer` knows everything that happened
    // outside SQL. Neither can fill the other's half.
    report.downloads_relinked = prepared.report.downloads_relinked;
    report.thumbs_written = prepared.report.thumbs_written;
    report.settings_applied = prepared.report.settings_applied;
    report.download_dir_kept = prepared.report.download_dir_kept;

    // An imported settings block is a settings save, so the live queue has to
    // hear about a changed limit the same way `save_settings` tells it.
    if report.settings_applied {
        if let Ok(s) = blocking(config::load).await {
            state.queue.set_concurrency(s.max_concurrent_downloads).await;
        }
    }

    // The library changed underneath every view. `App.tsx` refetches on this,
    // the same way it does after a poll -- deliberately a separate event from
    // `poll://finished`, which would also fire the "N new videos" toast and the
    // tray badge for videos that are not new to you at all.
    let _ = app.emit("transfer://finished", report.clone());
    Ok(report)
}

/// Players installed on this machine, "System default" first.
#[tauri::command]
pub async fn detect_players() -> Vec<PlayerOption> {
    // Probing means stat-ing and walking PATH. A failed join is an empty list,
    // which the view already shows as "System default" alone.
    tauri::async_runtime::spawn_blocking(crate::detect::detect_players)
        .await
        .unwrap_or_default()
}

/// Browsers yt-dlp could read cookies from, as found on this machine.
#[tauri::command]
pub async fn detect_browsers() -> Vec<BrowserOption> {
    tauri::async_runtime::spawn_blocking(crate::detect::detect_browsers)
        .await
        .unwrap_or_default()
}

#[tauri::command]
pub async fn tools_status(state: State<'_, Arc<AppState>>) -> R<Vec<ToolStatus>> {
    let s = blocking(config::load).await.unwrap_or_default();
    Ok(state.tools.status(&s).await)
}

/// "Check for updates" / "Retry" in the Tools section.
#[tauri::command]
pub async fn tools_update_now(state: State<'_, Arc<AppState>>) -> R<Vec<ToolStatus>> {
    let s = blocking(config::load).await.unwrap_or_default();
    state.tools.update_now(&s).await.map_err(e)
}

/// This build's version and any newer release already known, straight from
/// `update-check.json` -- no network. The nav's version pill and Settings'
/// About block draw from it at mount; `app://version-info` keeps them current.
#[tauri::command]
pub async fn app_version_info() -> R<app_update::VersionInfo> {
    blocking(|| Ok(app_update::current_info(&config::load().unwrap_or_default()))).await
}

/// About's "Check now": asks GitHub whether or not a day has passed.
#[tauri::command]
pub async fn check_app_update_now(
    state: State<'_, Arc<AppState>>,
    app: AppHandle,
) -> R<app_update::VersionInfo> {
    let s = blocking(config::load).await?;
    app_update::check_now(&app, &state.http, &s).await.map_err(e)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocking_runs_the_work_off_the_calling_thread() {
        let caller = std::thread::current().id();
        let ran_on =
            tauri::async_runtime::block_on(blocking(|| Ok(std::thread::current().id()))).unwrap();
        assert_ne!(ran_on, caller);
    }

    #[test]
    fn blocking_hands_back_the_value_and_the_error_as_text() {
        assert_eq!(tauri::async_runtime::block_on(blocking(|| Ok(42))), Ok(42));
        let err = tauri::async_runtime::block_on(blocking(|| -> anyhow::Result<()> {
            anyhow::bail!("Unknown video")
        }));
        assert_eq!(err, Err("Unknown video".to_string()));
    }

    #[test]
    fn a_panic_in_blocking_work_is_an_error_not_a_crash() {
        let r = tauri::async_runtime::block_on(blocking(|| -> anyhow::Result<()> {
            panic!("boom")
        }));
        assert!(r.is_err());
    }
}
