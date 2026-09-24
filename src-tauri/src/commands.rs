use std::sync::Arc;
use futures::stream::StreamExt;
use tauri::{AppHandle, Emitter, State};

use crate::models::*;
use crate::poll::{self, AppState};
use crate::{config, player, resolve, transfer, ytdlp};

/// Backfills run a few at a time: enough to hide latency, few enough
/// to stay clear of YouTube rate limiting.
const IMPORT_CONCURRENCY: usize = 3;

type R<T> = Result<T, String>;
fn e<E: std::fmt::Display>(err: E) -> String {
    err.to_string()
}

#[tauri::command]
pub fn get_settings() -> R<config::Settings> {
    config::load().map_err(e)
}

#[tauri::command]
pub async fn save_settings(
    mut settings: config::Settings,
    state: State<'_, Arc<AppState>>,
) -> R<()> {
    // The Settings view sends a snapshot taken at mount, so if a filter
    // changed while it was open, saving it verbatim would revert `view` to
    // that stale copy. A failed re-read (e.g. no file yet) just means there
    // is nothing on disk to defer to, so fall through and write as sent.
    if let Ok(disk) = config::load() {
        settings.keep_view_of(&disk);
    }
    config::save(&settings).map_err(e)?;
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
pub fn save_view_state(mut view: config::ViewState) -> R<()> {
    // `Settings::from_json_str` only sanitises on the way back in, so without
    // this an unbounded search string (no `maxLength` on the search box)
    // would be written to settings.json in full every 400ms of typing.
    // Sanitising here also means the stored block is always byte-for-byte
    // what the next launch's `sanitize` call would produce anyway.
    view.sanitize();
    let mut s = config::load().map_err(e)?;
    s.view = view;
    config::save(&s).map_err(e)
}

#[tauri::command]
pub fn list_channels(state: State<'_, Arc<AppState>>) -> R<Vec<Channel>> {
    // Only subscriptions: ad-hoc uploaders are not part of the user's channel list.
    state.db.list_subscribed_channels().map_err(e)
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
    let s = config::load().map_err(e)?;

    if state.db.get_video(&video_id).map_err(e)?.is_none() {
        let probe = ytdlp::probe(
            &ytdlp::watch_url(&video_id),
            &s.download_dir,
            &s.filename_template,
        )
        .await
        .map_err(e)?;

        let channel_id = if probe.channel_id.is_empty() {
            "UC000000000000000000000".to_string()
        } else {
            probe.channel_id.clone()
        };

        // subscribed = false: keeps the uploader's name on the card without
        // adding them to the subscription list or any poll cycle.
        state
            .db
            .upsert_channel(&Channel {
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
            })
            .map_err(e)?;

        state
            .db
            .insert_video_if_new(&NewVideo {
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
            })
            .map_err(e)?;

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
    state
        .db
        .get_video(&video_id)
        .map_err(e)?
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
    if let Some(existing) = state.db.get_channel(&id).map_err(e)? {
        if existing.subscribed {
            return Ok(existing);
        }
    }

    let settings = config::load().map_err(e)?;
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
    };
    state.db.upsert_channel(&channel).map_err(e)?;

    // Backfill history, then poll RSS so the recent window gets real dates.
    if let Err(err) = poll::backfill_channel(&state, &id, settings.backfill_count).await {
        eprintln!("backfill failed for {id}: {err}");
    }
    poll::poll_channels(&state, &app, vec![channel]).await;

    state
        .db
        .get_channel(&id)
        .map_err(e)?
        .ok_or_else(|| "Channel disappeared after being added".to_string())
}

#[tauri::command]
pub fn remove_channel(channel_id: String, state: State<'_, Arc<AppState>>) -> R<()> {
    state.db.remove_channel(&channel_id).map_err(e)
}

/// Parses the CSV without importing anything, so the UI can offer a checklist.
#[tauri::command]
pub fn preview_takeout_csv(path: String, state: State<'_, Arc<AppState>>) -> R<Vec<TakeoutRow>> {
    let data = std::fs::read_to_string(&path).map_err(e)?;
    let rows = resolve::parse_takeout_csv(&data).map_err(e)?;
    let subscribed: std::collections::HashSet<String> = state
        .db
        .list_subscribed_channels()
        .map_err(e)?
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
    let data = std::fs::read_to_string(&path).map_err(e)?;
    let all = resolve::parse_takeout_csv(&data).map_err(e)?;
    let wanted: std::collections::HashSet<String> = channel_ids.into_iter().collect();
    let rows: Vec<(String, String)> =
        all.into_iter().filter(|(id, _)| wanted.contains(id)).collect();

    let settings = config::load().map_err(e)?;
    let total = rows.len();
    let mut result = ImportResult::default();
    let mut to_backfill: Vec<(String, String)> = Vec::new();

    for (id, title) in rows {
        if state.db.get_channel(&id).map_err(e)?.map(|c| c.subscribed).unwrap_or(false) {
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
        };
        if let Err(err) = state.db.upsert_channel(&channel) {
            result.failed.push(format!("{title}: {err}"));
            continue;
        }
        to_backfill.push((id, channel.title));
    }

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

    let channels = state.db.list_subscribed_channels().map_err(e)?;
    poll::poll_channels(&state, &app, channels).await;
    Ok(result)
}

/// Hides a video so polling never surfaces it again.
#[tauri::command]
pub fn set_video_hidden(
    video_id: String,
    hidden: bool,
    state: State<'_, Arc<AppState>>,
) -> R<()> {
    state.db.set_hidden(&video_id, hidden).map_err(e)
}

/// Removes a manually added video outright. Refuses subscription videos, which
/// would simply reappear on the next poll — those must be hidden instead.
#[tauri::command]
pub fn delete_video(video_id: String, state: State<'_, Arc<AppState>>) -> R<()> {
    let v = state
        .db
        .get_video(&video_id)
        .map_err(e)?
        .ok_or_else(|| "Unknown video".to_string())?;
    if !v.added_manually {
        return Err("Only manually added videos can be deleted. Hide this one instead.".into());
    }
    state.db.delete_video(&video_id).map_err(e)?;
    state.db.prune_orphan_channel(&v.channel_id).map_err(e)
}

#[tauri::command]
pub async fn poll_all(state: State<'_, Arc<AppState>>, app: AppHandle) -> R<PollSummary> {
    let channels = state.db.list_subscribed_channels().map_err(e)?;
    Ok(poll::poll_channels(&state, &app, channels).await)
}

#[tauri::command]
pub async fn poll_channel(
    channel_id: String,
    state: State<'_, Arc<AppState>>,
    app: AppHandle,
) -> R<PollSummary> {
    let c = state
        .db
        .get_channel(&channel_id)
        .map_err(e)?
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
    if state.db.get_channel(&channel_id).map_err(e)?.is_none() {
        return Err(format!("No such channel: {channel_id}"));
    }
    state.db.set_channel_member(&channel_id, member).map_err(e)?;
    if !member {
        return Ok(0);
    }
    let depth = config::load().map(|s| s.backfill_count).unwrap_or(30);
    poll::ingest_members_only(&state, &channel_id, depth).await.map_err(e)
}

#[tauri::command]
pub fn list_videos(filter: VideoFilter, state: State<'_, Arc<AppState>>) -> R<Vec<Video>> {
    state.db.list_videos(&filter).map_err(e)
}

/// The same feed, with each channel's multi-part uploads behind one card.
/// `filter.limit` and `filter.offset` count groups here, not videos.
#[tauri::command]
pub fn list_video_groups(
    filter: VideoFilter,
    state: State<'_, Arc<AppState>>,
) -> R<Vec<VideoGroup>> {
    state.db.list_video_groups(&filter).map_err(e)
}

/// Marks videos siblings by hand, for a series whose titles no matcher could
/// ever join -- a renamed follow-up, most often. Returns the finished group's
/// size, which can exceed what was sent: marking across two hand-built groups
/// merges both. Refused across channels.
#[tauri::command]
pub fn mark_siblings(video_ids: Vec<String>, state: State<'_, Arc<AppState>>) -> R<usize> {
    state.db.mark_siblings(&video_ids).map_err(e)
}

/// Takes one video back out of its hand-built group, dissolving the group when
/// that leaves it with a single member.
#[tauri::command]
pub fn unlink_siblings(video_id: String, state: State<'_, Arc<AppState>>) -> R<()> {
    state.db.unlink_siblings(&video_id).map_err(e)
}

#[tauri::command]
pub fn set_watched(
    video_id: String,
    watched: bool,
    state: State<'_, Arc<AppState>>,
) -> R<()> {
    state.db.set_watched(&video_id, watched).map_err(e)
}

#[tauri::command]
pub async fn enqueue_download(video_id: String, state: State<'_, Arc<AppState>>) -> R<()> {
    let s = config::load().map_err(e)?;
    state
        .queue
        .enqueue(video_id, s.download_dir, s.filename_template)
        .await
        .map_err(e)
}

#[tauri::command]
pub async fn cancel_download(video_id: String, state: State<'_, Arc<AppState>>) -> R<()> {
    state.queue.cancel(&video_id).await.map_err(e)
}

#[tauri::command]
pub fn open_in_player(video_id: String, state: State<'_, Arc<AppState>>) -> R<()> {
    let v = state
        .db
        .get_video(&video_id)
        .map_err(e)?
        .ok_or_else(|| "Unknown video".to_string())?;
    let path = v
        .file_path
        .ok_or_else(|| "This video has not been downloaded".to_string())?;
    if !std::path::Path::new(&path).exists() {
        state.db.clear_file_path(&video_id).map_err(e)?;
        return Err(format!("File is gone: {path}. Marked as not downloaded."));
    }
    let s = config::load().map_err(e)?;
    player::launch(&s.player_command, &path).map_err(e)
}

#[tauri::command]
pub fn delete_download(video_id: String, state: State<'_, Arc<AppState>>) -> R<()> {
    let v = state
        .db
        .get_video(&video_id)
        .map_err(e)?
        .ok_or_else(|| "Unknown video".to_string())?;
    if let Some(p) = v.file_path {
        let _ = std::fs::remove_file(p);
    }
    state.db.clear_file_path(&video_id).map_err(e)
}

// ---- config transfer ----

/// What an export would weigh, so the "Include thumbnails" tick can offer a
/// real number instead of a guess.
#[tauri::command]
pub fn transfer_estimate(state: State<'_, Arc<AppState>>) -> R<TransferEstimate> {
    let channels = state.db.export_channels().map_err(e)?;
    let videos = state.db.export_videos().map_err(e)?;
    // `transfer` owns the sizing because it owns what actually goes in the zip:
    // a thumbnail is counted only if the file is really there to be copied.
    Ok(transfer::estimate(&channels, &videos))
}

#[tauri::command]
pub async fn export_config(
    path: String,
    include_thumbs: bool,
    state: State<'_, Arc<AppState>>,
    app: AppHandle,
) -> R<()> {
    let channels = state.db.export_channels().map_err(e)?;
    let videos = state.db.export_videos().map_err(e)?;
    let settings = config::load().map_err(e)?;
    let dest = std::path::PathBuf::from(path);

    // Zipping thousands of thumbnails is blocking work. On the async runtime's
    // own threads it would stall every other command behind it -- including the
    // progress events this very call is emitting.
    tauri::async_runtime::spawn_blocking(move || {
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
    .map_err(e)?
    .map_err(e)
}

/// Parses the archive's manifest without writing anything, so the dialog can
/// show what is inside before anything is committed to.
#[tauri::command]
pub async fn read_archive(
    path: String,
    state: State<'_, Arc<AppState>>,
) -> R<ArchiveSummary> {
    // Every channel, not just the subscribed ones: an ad-hoc uploader already
    // here is "already here", and saying otherwise would offer to re-add it.
    let known: std::collections::HashSet<String> = state
        .db
        .export_channels()
        .map_err(e)?
        .into_iter()
        .map(|c| c.id)
        .collect();
    let p = std::path::PathBuf::from(path);
    let mut summary = tauri::async_runtime::spawn_blocking(move || transfer::read_summary(&p, &known))
        .await
        .map_err(e)?
        .map_err(e)?;

    // What a Replace would remove, measured here because only the database can
    // answer it. Against the archive's whole roster, never the ticked subset:
    // unticking a row means "skip it", so the number the dialog shows must not
    // move as the user works down the checklist.
    let roster: Vec<String> = summary.channels.iter().map(|c| c.channel_id.clone()).collect();
    let (local_only_channels, local_only_videos) = state.db.absent_from(&roster).map_err(e)?;
    summary.local_only_channels = local_only_channels;
    summary.local_only_videos = local_only_videos;
    Ok(summary)
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
    let prepared = tauri::async_runtime::spawn_blocking(move || {
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
    .await
    .map_err(e)?
    .map_err(e)?;

    // And now the part that has to be atomic. One `Db` call, one transaction:
    // the connection Mutex is not reentrant, so a loop over several public
    // methods was never available, and a failure here must leave the library
    // exactly as it was.
    let mut report = state
        .db
        .apply_import(
            &prepared.channels,
            &prepared.videos,
            mode,
            &prepared.archive_channel_ids,
        )
        .map_err(e)?;

    // `db` knows the row counts; `transfer` knows everything that happened
    // outside SQL. Neither can fill the other's half.
    report.downloads_relinked = prepared.report.downloads_relinked;
    report.thumbs_written = prepared.report.thumbs_written;
    report.settings_applied = prepared.report.settings_applied;
    report.download_dir_kept = prepared.report.download_dir_kept;

    // An imported settings block is a settings save, so the live queue has to
    // hear about a changed limit the same way `save_settings` tells it.
    if report.settings_applied {
        if let Ok(s) = config::load() {
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
