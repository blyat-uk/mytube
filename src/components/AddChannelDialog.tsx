import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import TakeoutDialog from "./TakeoutDialog";
import CustomDownloadDialog, { type Batch } from "./CustomDownloadDialog";
import SubscriptionList from "./add/SubscriptionList";
import {
  ChannelAnswer, PlaylistCard, PreviewSkeleton, VideoCard, type Acting,
} from "./add/LinkPreview";
import { tickedEntries, wantsDownload } from "./add/playlist";
import { useAutoHeight } from "./add/useAutoHeight";
import { IconClose, IconLink } from "./Icons";
import { useToast } from "./Toast";
import { api, errText } from "../api";
import { usePending } from "../pending";
import type {
  AddKind, AddPreview, Channel, ChannelStats, PlaylistPreview, TakeoutRow, Video,
} from "../types";

interface Props {
  open: boolean;
  onClose: () => void;
  channels: Channel[];
  /** Something changed server-side; the shell should reload channels and videos. */
  onChanged: () => void;
  /** Whether the subscriptions list is folded away. The shell owns it, and
   *  remembers it with the rest of the view. */
  subsFolded: boolean;
  onSubsFolded: (folded: boolean) => void;
}

/**
 * What the input holds, as far as it has been read.
 *
 * `kind` survives into `reading` so the chip and the card stay put while a
 * link that was already understood is edited; it is only replaced once the
 * edit has been read in turn.
 */
type Look =
  | { phase: "empty" }
  | { phase: "reading"; kind: AddKind | null }
  | { phase: "loading"; kind: AddKind }
  /** `preview` is null for a Short, which is refused before it is fetched. */
  | { phase: "ready"; kind: AddKind; preview: AddPreview | null }
  | { phase: "error"; kind: AddKind | null; message: string };

type Custom =
  | { kind: "video"; video: Video }
  | { kind: "batch"; batch: Batch };

const KIND_LABEL: Record<AddKind, string> = {
  channel: "Channel",
  video: "Video",
  playlist: "Playlist",
  short: "Short",
};

/** Typing waits this long before a link is read; a paste is read at once. */
const READ_DELAY = 300;

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

export default function AddChannelDialog({
  open: isOpen, onClose, channels, onChanged, subsFolded, onSubsFolded,
}: Props) {
  const toast = useToast();
  const [input, setInput] = useState("");
  const [look, setLook] = useState<Look>({ phase: "empty" });
  // Bumped to read the same input again: after a custom download was backed
  // out of, the video is in the library now and the card should say so.
  const [readAgain, setReadAgain] = useState(0);
  const [ticked, setTicked] = useState<ReadonlySet<string>>(new Set());
  const [importing, setImporting] = useState(false);
  const [takeout, setTakeout] = useState<{ path: string; rows: TakeoutRow[] } | null>(null);
  const [custom, setCustom] = useState<Custom | null>(null);
  const [stats, setStats] = useState<ReadonlyMap<string, ChannelStats> | null>(null);
  const [reveal, setReveal] = useState<{ id: string; n: number } | null>(null);
  // One key for the preview's three actions: an Add and a Download of the
  // same paste must never both be out. `acting` says which one to spin.
  const { pending, slow, run } = usePending();
  const [acting, setActing] = useState<Acting>(null);
  const inputRef = useRef<HTMLInputElement | null>(null);
  const readId = useRef(0);
  const readNow = useRef(false);
  /** The text `look` describes, once it has been read. */
  const shownText = useRef("");
  const statsId = useRef(0);
  // Read by the window's Escape handler: one Escape closes only the dialog on
  // top. The custom download listens for its own; the Takeout checklist has
  // none, so it is closed from here -- unless its import is running.
  const stacked = useRef<"custom" | "takeout" | null>(null);
  stacked.current = custom ? "custom" : takeout ? "takeout" : null;
  const importingRef = useRef(false);
  importingRef.current = importing;

  useEffect(() => {
    if (isOpen) inputRef.current?.focus();
  }, [isOpen]);

  useEffect(() => {
    if (!isOpen) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      if (stacked.current === "takeout") {
        if (!importingRef.current) setTakeout(null);
      } else if (stacked.current === null) {
        onClose();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [isOpen, onClose]);

  /** The counts behind the list and its panels. A failure is not worth a
   *  toast -- they are a summary of what is already on screen elsewhere -- so
   *  the rows simply read as empty. */
  const refreshStats = useCallback(() => {
    const id = ++statsId.current;
    api.subscriptionStats().then(
      (rows) => {
        if (id === statsId.current) setStats(new Map(rows.map((r) => [r.channelId, r])));
      },
      () => {
        if (id === statsId.current) setStats((s) => s ?? new Map());
      },
    );
  }, []);

  // On opening, and whenever the shell's channel list moves under an open
  // dialog (a poll finishing, a change made here).
  useEffect(() => {
    if (isOpen) refreshStats();
  }, [isOpen, channels, refreshStats]);

  const changed = useCallback(() => {
    onChanged();
    refreshStats();
  }, [onChanged, refreshStats]);

  // Reading a link is two steps: what it is (pure parsing, instant), then
  // what it holds (`preview_add`, which for a video or a playlist goes to
  // YouTube). Both answer to one counter, so an edit made while either is out
  // drops whatever the older text comes back with.
  useEffect(() => {
    const text = input.trim();
    const id = ++readId.current;
    if (!text) {
      readNow.current = false;
      shownText.current = "";
      setLook({ phase: "empty" });
      return;
    }
    // The same text read again (`readAgain`) keeps its card up until the new
    // answer replaces it, rather than blinking back to a skeleton.
    const again = text === shownText.current;
    if (!again) setLook((l) => ({ phase: "reading", kind: "kind" in l ? l.kind : null }));
    const delay = readNow.current || again ? 0 : READ_DELAY;
    readNow.current = false;
    const settle = (next: Look) => {
      if (id !== readId.current) return;
      shownText.current = text;
      setLook(next);
    };
    const t = window.setTimeout(async () => {
      let kind: AddKind;
      try {
        kind = await api.classifyAddInput(text);
      } catch (err) {
        settle({ phase: "error", kind: null, message: errText(err) });
        return;
      }
      if (id !== readId.current) return;
      if (kind === "short") {
        settle({ phase: "ready", kind, preview: null });
        return;
      }
      if (!again) setLook({ phase: "loading", kind });
      try {
        const preview = await api.previewAdd(text);
        if (id !== readId.current) return;
        if (preview.kind === "playlist" && !again) {
          setTicked(new Set(preview.playlist.entries.map((e) => e.id)));
        }
        settle({ phase: "ready", kind: preview.kind, preview });
      } catch (err) {
        // Whether a channel is one you already follow is a nicety; failing to
        // find out must not stand between you and subscribing to it.
        settle(kind === "channel"
          ? { phase: "ready", kind, preview: { kind: "channel", subscribedId: null } }
          : { phase: "error", kind, message: errText(err) });
      }
    }, delay);
    return () => window.clearTimeout(t);
  }, [input, readAgain]);

  if (!isOpen) return null;

  const preview = look.phase === "ready" ? look.preview : null;
  const out = pending.has("add") || pending.has("subscribe");
  const busy = { acting: pending.has("add") ? acting : null, slow: slow.has("add") };

  function clearInput() {
    setInput("");
    inputRef.current?.focus();
  }

  function act(which: Exclude<Acting, null>, fn: () => Promise<void>) {
    // Refused below anyway; returning first keeps the spinner on the button
    // whose call is actually out.
    if (pending.has("add")) return;
    setActing(which);
    void run("add", fn);
  }

  function subscribe() {
    const text = input.trim();
    void run("subscribe", async () => {
      const c = await api.addChannel(text);
      toast.success(`Subscribed to ${c.title}.`);
      clearInput();
      changed();
    });
  }

  /** Add puts the video in the library and stops there; Download goes on to
   *  queue it with the Settings quality; custom opens the quality dialog over
   *  this one. Each adds first, so the feed has it whatever happens next. */
  function videoAction(which: Exclude<Acting, null>) {
    const text = input.trim();
    act(which, async () => {
      const v = await api.addVideo(text);
      changed();
      if (which === "add") {
        toast.success(`Added “${v.title}”.`);
        clearInput();
      } else if (which === "download") {
        try {
          await api.enqueueDownload(v.id);
        } catch (err) {
          // Added, though: the card should stop offering Add.
          setReadAgain((n) => n + 1);
          throw err;
        }
        toast.success(`Downloading “${v.title}”.`);
        clearInput();
      } else {
        setCustom({ kind: "video", video: v });
      }
    });
  }

  /** The same three, for a playlist's ticked parts. Add takes the new ones;
   *  Download every one not on disk or on its way, oldest first (the queue
   *  orders them); custom one quality for all of those. */
  function playlistAction(which: Exclude<Acting, null>, pl: PlaylistPreview) {
    const picked = tickedEntries(pl.entries, ticked);
    act(which, async () => {
      const r = await api.addPlaylist(picked);
      changed();
      const skipped = new Set(r.skippedShorts);
      const shorts = r.skippedShorts.length
        ? `${plural(r.skippedShorts.length, "Short")} skipped`
        : "";
      if (which === "add") {
        if (r.added.length > 0) {
          toast.success(`Added ${plural(r.added.length, "video")}${shorts ? `, ${shorts}` : ""}.`);
        } else {
          toast.info(`Nothing new to add${shorts ? ` — ${shorts}` : ""}.`);
        }
        clearInput();
        return;
      }
      const ids = picked
        .filter((e) => wantsDownload(e.downloadState) && !skipped.has(e.id))
        .map((e) => e.id);
      if (which === "custom") {
        if (ids.length === 0) {
          toast.info(`Nothing to download${shorts ? ` — ${shorts}` : ""}.`);
          setReadAgain((n) => n + 1);
          return;
        }
        setCustom({ kind: "batch", batch: { ids, title: pl.title } });
        return;
      }
      const queued = ids.length ? await api.enqueueDownloads(ids) : 0;
      if (queued > 0) {
        toast.success(`Queued ${plural(queued, "video")}${shorts ? `, ${shorts}` : ""}.`);
      } else {
        toast.info("Nothing to queue — every part is downloaded or on its way.");
      }
      clearInput();
    });
  }

  /** Enter does what the primary button says, and nothing that downloads. */
  function submit() {
    if (out || look.phase !== "ready" || !preview) return;
    if (preview.kind === "channel" && !preview.subscribedId) subscribe();
    else if (preview.kind === "video" && !preview.video.inLibrary) videoAction("add");
    else if (preview.kind === "playlist"
      && tickedEntries(preview.playlist.entries, ticked).some((e) => !e.inLibrary)) {
      playlistAction("add", preview.playlist);
    }
  }

  function showInList(id: string) {
    onSubsFolded(false);
    setReveal((r) => ({ id, n: (r?.n ?? 0) + 1 }));
  }

  /** Step one: parse the CSV and show the checklist. Nothing is imported yet. */
  async function pickCsv() {
    setImporting(true);
    try {
      const path = await open({
        multiple: false,
        directory: false,
        title: "Choose a Google Takeout subscriptions CSV",
        filters: [{ name: "CSV", extensions: ["csv"] }],
      });
      if (!path) return;
      const rows = await api.previewTakeoutCsv(path as string);
      if (rows.length === 0) {
        toast.error("That CSV has no channels in it.");
        return;
      }
      setTakeout({ path: path as string, rows });
    } catch (err) {
      toast.error(errText(err));
    } finally {
      setImporting(false);
    }
  }

  /** Step two: import exactly what was ticked. */
  async function runImport(channelIds: string[]) {
    if (!takeout) return;
    setImporting(true);
    try {
      const result = await api.importTakeoutCsv(takeout.path, channelIds);
      const failed = result.failed.length;
      toast.success(
        `Imported ${result.added} channel${result.added === 1 ? "" : "s"}` +
        `, skipped ${result.skipped}` +
        (failed ? `, ${failed} failed` : "") + ".",
      );
      if (failed) toast.error(`Could not import: ${result.failed.slice(0, 5).join(", ")}`);
      setTakeout(null);
      changed();
    } catch (err) {
      toast.error(errText(err));
    } finally {
      setImporting(false);
    }
  }

  // The zone under the input. Its key changes when what it shows is swapped,
  // which is what lets its height slide rather than jump (`useAutoHeight`).
  let zoneKey: string;
  let zone: ReactNode;
  const kind = look.phase === "empty" ? null : look.kind;
  if (look.phase === "error") {
    zoneKey = "warn";
    zone = <p className="add-hint is-warn" role="alert">{look.message}</p>;
  } else if (look.phase === "ready" && look.kind === "short") {
    zoneKey = "warn";
    zone = <p className="add-hint is-warn">Shorts are excluded from MyTube by design.</p>;
  } else if (preview?.kind === "video") {
    zoneKey = "video";
    zone = (
      <VideoCard
        video={preview.video}
        playlistId={preview.playlistId}
        busy={busy}
        onAdd={() => videoAction("add")}
        onDownload={() => videoAction("download")}
        onCustom={() => videoAction("custom")}
        onWholePlaylist={() => {
          readNow.current = true;
          setInput(`https://www.youtube.com/playlist?list=${encodeURIComponent(preview.playlistId!)}`);
          inputRef.current?.focus();
        }}
      />
    );
  } else if (preview?.kind === "playlist") {
    zoneKey = "playlist";
    zone = (
      <PlaylistCard
        key={preview.playlist.id}
        playlist={preview.playlist}
        ticked={ticked}
        onTicked={setTicked}
        busy={busy}
        onAdd={() => playlistAction("add", preview.playlist)}
        onDownload={() => playlistAction("download", preview.playlist)}
        onCustom={() => playlistAction("custom", preview.playlist)}
      />
    );
  } else if (kind === "video" || kind === "playlist") {
    zoneKey = `${kind}:skeleton`;
    zone = <PreviewSkeleton kind={kind} />;
  } else if (kind === "channel") {
    const id = preview?.kind === "channel" ? preview.subscribedId : null;
    zoneKey = id ? "channel:followed" : "channel";
    zone = (
      <ChannelAnswer
        alreadySubscribed={id ? (channels.find((c) => c.id === id) ?? true) : null}
        checking={look.phase !== "ready"}
        busy={out}
        spinning={slow.has("subscribe")}
        onSubscribe={subscribe}
        onShow={() => id && showInList(id)}
      />
    );
  } else {
    zoneKey = "hint";
    zone = (
      <p className="add-hint">
        {look.phase === "empty"
          ? "Paste a channel to subscribe to, or a video or playlist to add or download."
          : "Reading the link…"}
      </p>
    );
  }

  const chip = kind && look.phase !== "empty" ? kind : null;

  return (
    <>
    <div
      className="modal-backdrop"
      // Pressed on the backdrop itself, not released there: a text selection
      // dragged out of the input must not close the dialog, nor a press that
      // began on a dialog stacked over this one.
      onMouseDown={(e) => { if (e.target === e.currentTarget) onClose(); }}
    >
      <div
        className="modal add-dialog"
        role="dialog"
        aria-modal="true"
        aria-label="Add a channel, video or playlist"
      >
        <div className="modal-head">
          <h2>Add</h2>
          <button type="button" className="icon-btn add-close" onClick={onClose} title="Close">
            <span aria-hidden="true"><IconClose /></span>
            <span className="sr-only">Close</span>
          </button>
        </div>

        <form
          className="add-form"
          onSubmit={(e) => { e.preventDefault(); submit(); }}
        >
          <span className="add-form-glyph" aria-hidden="true"><IconLink /></span>
          <input
            ref={inputRef}
            className="text-input"
            value={input}
            placeholder="Channel, video or playlist link, or @handle"
            aria-label="Channel, video or playlist to add"
            spellCheck={false}
            autoComplete="off"
            onPaste={() => { readNow.current = true; }}
            onChange={(e) => setInput(e.currentTarget.value)}
          />
          {chip && (
            <span
              className={`add-kind${chip === "short" ? " is-warn" : ""}${look.phase === "reading" ? " is-stale" : ""}`}
            >
              {KIND_LABEL[chip]}
            </span>
          )}
        </form>

        <PreviewZone zoneKey={zoneKey}>{zone}</PreviewZone>
        <p className="sr-only" role="status">
          {look.phase === "reading" || look.phase === "loading" ? "Reading the link…" : ""}
        </p>

        <div className="add-divider" />

        <SubscriptionList
          channels={channels}
          stats={stats}
          folded={subsFolded}
          onFold={onSubsFolded}
          reveal={reveal}
          importing={importing}
          onImportCsv={() => void pickCsv()}
          onChanged={changed}
        />
      </div>
    </div>

    {/* Siblings, not children: nested inside the backdrop above, every click
        inside them bubbled to it. Later in the document, so they sit on top. */}
    {takeout && (
      <TakeoutDialog
        rows={takeout.rows}
        importing={importing}
        onImport={(ids) => void runImport(ids)}
        onCancel={() => setTakeout(null)}
      />
    )}
    {custom?.kind === "video" && (
      <CustomDownloadDialog
        video={custom.video}
        onQueued={(v) => {
          toast.success(`Downloading “${v.title}”.`);
          clearInput();
        }}
        // Backed out of, the video is still added: read the link again so
        // the card says it is in the library now.
        onClose={() => { setCustom(null); setReadAgain((n) => n + 1); }}
      />
    )}
    {custom?.kind === "batch" && (
      <CustomDownloadDialog
        batch={custom.batch}
        onBatchQueued={(queued) => {
          if (queued > 0) toast.success(`Queued ${plural(queued, "video")}.`);
          else toast.info("Nothing to queue — every part is downloaded or on its way.");
          clearInput();
        }}
        onClose={() => { setCustom(null); setReadAgain((n) => n + 1); }}
      />
    )}
    </>
  );
}

/** The space under the input, which slides to the height of whatever it
 *  holds. Mounted with the dialog, so its measuring starts when it is seen. */
function PreviewZone({ zoneKey, children }: { zoneKey: string; children: ReactNode }) {
  const { outer, inner } = useAutoHeight(zoneKey);
  return (
    <div ref={outer} className="add-zone">
      <div ref={inner} className="add-zone-in">
        <div key={zoneKey} className="add-zone-swap">{children}</div>
      </div>
    </div>
  );
}
