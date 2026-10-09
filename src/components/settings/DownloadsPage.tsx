import {
  Fragment, useEffect, useId, useRef, useState, type KeyboardEvent, type ReactNode,
} from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api, errText } from "../../api";
import { TEMPLATE_PRESETS, TOKEN_CHIPS, insertToken } from "../../format";
import { overriddenByFormat, sanitizeQuality } from "../../quality";
import type {
  Quality, QualityAudioFormat, QualityCodec, QualityContainer, QualityFps, QualityMode,
  VideoFilter,
} from "../../types";
import {
  IconAudioMode, IconCheck, IconChevron, IconEdit, IconFolder, IconSectionDownloads,
  IconVideoMode,
} from "../Icons";
import { useToast } from "../Toast";
import { useSettingsCtx, type RailLine, type SettingsCtx } from "./context";
import { LIMITS, clamp } from "./doc";
import {
  SAMPLE_VIDEO, folderName, folderSegments, renderTemplate, splitRendered, templateFields,
  type PreviewVideo,
} from "./filenamePreview";
import { Block, Cap, Segmented, SettingsPage, Stepper, type SegOption } from "./parts";

const CODEC_SHORT = { any: "", av1: "AV1", vp9: "VP9", h264: "H.264" } as const;
const AUDIO_SHORT = { original: "Original", m4a: "M4A", mp3: "MP3", opus: "Opus" } as const;

/** "1080p AV1 · MKV · 3 at once", "Audio · MP3 · 3 at once". */
export function summary({ settings: s }: SettingsCtx): RailLine {
  const q = sanitizeQuality(s.quality);
  const n = `${s.max_concurrent_downloads} at once`;
  if (q.format.trim()) return { text: `Custom format · ${n}` };
  if (q.mode === "audio") return { text: `Audio · ${AUDIO_SHORT[q.audio_format]} · ${n}` };
  const height = q.max_height ? `${q.max_height}p` : "Best";
  const codec = CODEC_SHORT[q.vcodec];
  return { text: `${codec ? `${height} ${codec}` : height} · ${q.container.toUpperCase()} · ${n}` };
}

export default function DownloadsPage() {
  return (
    <SettingsPage
      icon={<IconSectionDownloads />}
      title="Downloads"
      intro="Where files land, what they are called, and the quality every download aims for."
    >
      <Block>
        <SavedTo />
        <SavedAs />
      </Block>
      <QualityBlock />
      <AtOnce />
    </SettingsPage>
  );
}

/* ------------------------------------------------------------------ *
 * Saved to
 * ------------------------------------------------------------------ */

/**
 * The download folder, drawn as a path rather than sitting in a text box: the
 * last segment is the folder that matters, so it is the bold one, and a long
 * path is trimmed from the left so that segment is never what gets cut. Edit
 * swaps in a field for the rare hand-typed path (a mount the folder dialog
 * cannot browse to); Choose… is the usual way.
 */
function SavedTo() {
  const { settings, set, save, flush } = useSettingsCtx();
  const toast = useToast();
  const capId = useId();
  const [editing, setEditing] = useState(false);
  /** The folder as it was when Edit was pressed, which Esc puts back. */
  const before = useRef("");
  /** Set by Esc, so a blur the field may still send as it unmounts saves
   *  nothing. Cleared by the next Edit. */
  const cancelled = useRef(false);
  /** Enter and Esc hand focus back to Edit; a click elsewhere keeps it where it landed. */
  const refocus = useRef(false);
  const editRef = useRef<HTMLButtonElement | null>(null);
  /** A second Choose… while the folder dialog is up would open another. */
  const picking = useRef(false);

  useEffect(() => {
    if (!editing && refocus.current) {
      refocus.current = false;
      editRef.current?.focus();
    }
  }, [editing]);

  const startEdit = () => {
    before.current = settings.download_dir;
    cancelled.current = false;
    setEditing(true);
  };

  const finish = () => {
    if (!cancelled.current) flush();
    setEditing(false);
  };

  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Enter") {
      e.preventDefault();
      refocus.current = true;
      // Saved by the blur this causes, so Enter and a click away are one path
      // and one write.
      e.currentTarget.blur();
    } else if (e.key === "Escape") {
      e.preventDefault();
      // The shell's Escape leaves an open series; here it only means "never mind".
      e.stopPropagation();
      cancelled.current = true;
      refocus.current = true;
      set("download_dir", before.current);
      setEditing(false);
    }
  };

  async function choose() {
    if (picking.current) return;
    picking.current = true;
    try {
      const path = await open({ directory: true, multiple: false, title: "Choose a download folder" });
      if (typeof path === "string" && path) save("download_dir", path);
    } catch (err) {
      toast.error(errText(err));
    } finally {
      picking.current = false;
    }
  }

  return (
    <>
      <Cap id={capId}>Saved to</Cap>
      <div className={`st-dl-folder${editing ? " is-editing" : ""}`} role="group" aria-labelledby={capId}>
        <span className="st-dl-folder-tile" aria-hidden="true"><IconFolder /></span>
        {editing ? (
          <input
            className="text-input st-dl-folder-input"
            aria-label="Download folder"
            value={settings.download_dir}
            spellCheck={false}
            autoFocus
            onChange={(e) => set("download_dir", e.currentTarget.value)}
            onKeyDown={onKeyDown}
            onBlur={finish}
          />
        ) : (
          <FolderPath dir={settings.download_dir} />
        )}
        <div className="st-dl-folder-actions">
          {!editing && (
            <button ref={editRef} type="button" className="btn btn-quiet st-dl-btn" onClick={startEdit}>
              <IconEdit />Edit
            </button>
          )}
          <button type="button" className="btn st-dl-btn" onClick={() => void choose()}>
            Choose…
          </button>
        </div>
      </div>
    </>
  );
}

function FolderPath({ dir }: { dir: string }) {
  const { segments, sep } = folderSegments(dir);
  const last = segments.length - 1;
  return (
    <span className="st-dl-path" title={dir}>
      {/* Drawn for the eye; the plain path below is what a screen reader reads. */}
      <span className="st-dl-path-inner" aria-hidden="true">
        {!dir.trim() ? (
          <span className="st-dl-path-dim">No folder set</span>
        ) : segments.length === 0 ? (
          dir
        ) : (
          segments.map((seg, i) => (
            <Fragment key={i}>
              {i === last ? seg : <span className="st-dl-path-dim">{seg}</span>}
              {i < last && <span className="st-dl-path-sep">{sep}</span>}
            </Fragment>
          ))
        )}
      </span>
      <span className="sr-only">{dir || "No folder set"}</span>
    </span>
  );
}

/* ------------------------------------------------------------------ *
 * Each video is saved as
 * ------------------------------------------------------------------ */

/** The one newest video in the library, whatever the feed is filtered to. */
const NEWEST: VideoFilter = {
  channelIds: [], hideWatched: false, downloadedOnly: false, showHidden: false,
  groupsOnly: false, inProgress: false, search: null, siblingOf: null, sort: "newest",
  limit: 1, offset: 0,
};

/** Null until it lands, and for good when the library is empty or unreadable:
 *  the sample stands in, which is all a preview needs. */
function useNewestVideo(): PreviewVideo | null {
  const [video, setVideo] = useState<PreviewVideo | null>(null);
  useEffect(() => {
    let alive = true;
    api.listVideos(NEWEST)
      .then(([v]) => {
        if (!alive || !v) return;
        setVideo({
          id: v.id, title: v.title, uploader: v.channel_title, published_at: v.published_at,
          duration_secs: v.duration_secs, view_count: v.view_count,
        });
      })
      .catch(() => {});
    return () => { alive = false; };
  }, []);
  return video;
}

/** A radio value no template can be: Custom… is a mode, not a template. */
const CUSTOM = "\0custom";

const PRESET_OPTIONS: SegOption<string>[] = [
  ...TEMPLATE_PRESETS.map((p) => ({ value: p.value, label: p.label, title: p.hint })),
  { value: CUSTOM, label: "Custom…" },
];

/**
 * What a file ends up called, shown rather than described: the template
 * rendered against the newest video, starting from the download folder's own
 * name so one line says both where and what. The presets cover what most
 * people want; Custom… opens the template itself, with the fields to build one.
 */
function SavedAs() {
  const { settings, set, save, flush } = useSettingsCtx();
  const newest = useNewestVideo();
  const template = settings.filename_template;
  const isPreset = TEMPLATE_PRESETS.some((p) => p.value === template);
  // Once opened, Custom stays open until a preset is picked: typing that
  // happens to spell a preset must not swap the field away mid-edit.
  const [customOpen, setCustomOpen] = useState(!isPreset);
  const custom = customOpen || !isPreset;

  const inputRef = useRef<HTMLInputElement | null>(null);
  /** Where the caret was when the field last lost focus, for a field chip
   *  pressed from the keyboard -- focus is on the chip by then. */
  const caret = useRef<number | null>(null);

  const pickPreset = (value: string) => {
    if (value === CUSTOM) {
      setCustomOpen(true);
      return;
    }
    setCustomOpen(false);
    save("filename_template", value);
  };

  /** Inserts a field at the caret and saves, keeping the caret after it. */
  const applyToken = (token: string) => {
    const el = inputRef.current;
    const at = el && document.activeElement === el ? el.selectionStart : caret.current;
    const { value, caret: next } = insertToken(template, at, token);
    save("filename_template", value);
    caret.current = next;
    requestAnimationFrame(() => {
      const field = inputRef.current;
      if (!field) return;
      field.focus();
      field.setSelectionRange(next, next);
    });
  };

  const q = sanitizeQuality(settings.quality);
  const rendered = renderTemplate(template, templateFields(newest ?? SAMPLE_VIDEO, q));
  const { absolute, dirs, file } = splitRendered(rendered);
  // An absolute template ignores the download folder, so it is not shown.
  const root = absolute ? "" : folderName(settings.download_dir);
  const whole = absolute ? rendered : [root, rendered].filter(Boolean).join("/");
  const tokensId = useId();

  return (
    <>
      <Cap right={newest ? "shown with your newest video" : undefined}>Each video is saved as</Cap>
      <div className="st-dl-preview" title={whole}>
        {root && (
          <>
            <span className="st-dl-preview-root">{root}</span>
            <span className="st-dl-preview-sep">/</span>
          </>
        )}
        {dirs.map((d, i) => (
          <Fragment key={i}>
            <span className="st-dl-preview-dir">{d}</span>
            <span className="st-dl-preview-sep">/</span>
          </Fragment>
        ))}
        <span className="st-dl-preview-file">{file}</span>
      </div>

      <div className="st-dl-tmpl">
        <Segmented
          label="File name template"
          options={PRESET_OPTIONS}
          value={custom ? CUSTOM : template}
          onChange={pickPreset}
        />
        {!custom && <code className="st-dl-tmpl-raw" title={template}>{template}</code>}
      </div>

      {custom && (
        <div className="st-dl-custom">
          <input
            ref={inputRef}
            className="text-input mono st-dl-custom-input"
            aria-label="Custom template"
            value={template}
            spellCheck={false}
            onChange={(e) => set("filename_template", e.currentTarget.value)}
            onBlur={(e) => {
              caret.current = e.currentTarget.selectionStart;
              flush();
            }}
          />
          <Cap id={tokensId}>Insert a field</Cap>
          <div className="st-dl-tokens" role="group" aria-labelledby={tokensId}>
            {TOKEN_CHIPS.map((token) => (
              <button
                key={token}
                type="button"
                className="st-dl-token"
                title={`Insert ${token} at the cursor`}
                // Keeps focus, and so the caret, in the field for a mouse click.
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => applyToken(token)}
              >
                {token}
              </button>
            ))}
          </div>
        </div>
      )}
    </>
  );
}

/* ------------------------------------------------------------------ *
 * Quality
 * ------------------------------------------------------------------ */

/* Short labels: the chips sit in rows under a caption that already says what
   they are. quality.ts keeps the long ones, which the custom-download dialog's
   selects still use. */
const MODE_OPTIONS: SegOption<QualityMode>[] = [
  { value: "video", label: "Video", icon: <IconVideoMode /> },
  { value: "audio", label: "Audio only", icon: <IconAudioMode /> },
];
const CODEC_OPTIONS: SegOption<QualityCodec>[] = [
  { value: "any", label: "Any" },
  { value: "av1", label: "AV1", note: "smallest" },
  { value: "vp9", label: "VP9" },
  { value: "h264", label: "H.264", note: "plays everywhere" },
];
const FPS_OPTIONS: SegOption<QualityFps>[] = [
  { value: "any", label: "Any" },
  { value: "prefer60", label: "Prefer 60" },
  { value: "max30", label: "At most 30" },
];
const CONTAINER_OPTIONS: SegOption<QualityContainer>[] = [
  { value: "mkv", label: "MKV" },
  { value: "mp4", label: "MP4" },
];
const AUDIO_OPTIONS: SegOption<QualityAudioFormat>[] = [
  { value: "original", label: "Original", note: "no conversion" },
  { value: "m4a", label: "M4A" },
  { value: "mp3", label: "MP3" },
  { value: "opus", label: "Opus" },
];

/** The ceiling, smallest first, so the lit steps read as "everything up to here". */
const LADDER = [
  ...[360, 480, 720, 1080, 1440, 2160].map((h) => ({ value: h, label: `${h}p` })),
  { value: 0, label: "Best" },
];

/**
 * The Settings default every download follows -- the Download button,
 * auto-download and the Add box; Download (custom)… overrides it per video.
 * Every pick saves at once; only the raw `-f` field, which is typed, waits
 * for its blur.
 */
function QualityBlock() {
  const { settings, set, save, flush } = useSettingsCtx();
  // Rust fills the block on every read; sanitising covers a Settings built
  // without one, so no control renders an undefined value.
  const q = sanitizeQuality(settings.quality);
  const raw = overriddenByFormat(q);
  const pick = <K extends keyof Quality>(key: K, value: Quality[K]) => save("quality", { ...q, [key]: value });
  // Open from the start when there is something in it: a raw format hidden
  // behind a closed disclosure would leave the disabled controls unexplained.
  const [rawOpen, setRawOpen] = useState(raw);
  const rawId = useId();

  return (
    <Block
      title="Quality"
      aside={
        <Segmented label="Mode" size="big" options={MODE_OPTIONS} value={q.mode} onChange={(v) => pick("mode", v)} />
      }
    >
      {q.mode === "video" ? (
        <>
          <Ladder value={q.max_height} disabled={raw} onChange={(v) => pick("max_height", v)} />
          <div className="st-dl-qrows">
            <ChipRow label="Codec" options={CODEC_OPTIONS} value={q.vcodec} disabled={raw}
              onChange={(v) => pick("vcodec", v)} />
            <ChipRow label="Frame rate" options={FPS_OPTIONS} value={q.fps} disabled={raw}
              onChange={(v) => pick("fps", v)} />
            {/* Merging still applies to whatever a raw format picks. */}
            <ChipRow label="Container" options={CONTAINER_OPTIONS} value={q.container}
              onChange={(v) => pick("container", v)} />
          </div>
        </>
      ) : (
        <div className="st-dl-qrows">
          <ChipRow label="Audio format" options={AUDIO_OPTIONS} value={q.audio_format}
            onChange={(v) => pick("audio_format", v)} />
        </div>
      )}

      <div className="st-dl-qsum">
        <span className="st-dl-qsum-icon" aria-hidden="true"><IconCheck /></span>
        <span className="st-dl-qsum-text">
          {q.mode === "video"
            ? "Preferences, not requirements: a video without them still downloads."
            : "Downloads the best audio stream; anything but Original is converted by ffmpeg."}
        </span>
        <button
          type="button"
          className="st-dl-adv"
          aria-expanded={rawOpen}
          aria-controls={rawOpen ? rawId : undefined}
          onClick={() => setRawOpen((o) => !o)}
        >
          Raw <code>-f</code>
          <IconChevron />
        </button>
      </div>
      {rawOpen && (
        <RawFormat
          id={rawId}
          value={q.format}
          onChange={(format) => set("quality", { ...q, format })}
          onBlur={flush}
        />
      )}
    </Block>
  );
}

/**
 * The resolution ceiling as steps rather than a select: everything at or under
 * the ceiling is lit, so "up to 1080p" reads as the range it is. Best is the
 * top step and lights them all. Real radios, so arrow keys walk it.
 */
function Ladder({ value, disabled, onChange }: {
  value: number;
  disabled: boolean;
  onChange: (height: number) => void;
}) {
  const name = useId();
  const found = LADDER.findIndex((s) => s.value === value);
  const ceiling = found < 0 ? LADDER.length - 1 : found;
  const note = disabled
    ? "replaced by the raw -f below"
    : value ? "smaller videos download as they are" : "the largest each video has";
  return (
    <div className={`st-dl-ladder${disabled ? " is-off" : ""}`}>
      <div className="st-dl-ladder-cap">
        <span>Resolution · <strong>{value ? `up to ${value}p` : "best available"}</strong></span>
        <span>{note}</span>
      </div>
      <div className="st-dl-ladder-track" role="radiogroup" aria-label="Resolution">
        {LADDER.map((step, i) => (
          <label
            key={step.value}
            className={`st-dl-step${i < ceiling ? " is-in" : i === ceiling ? " is-on" : ""}`}
          >
            <input
              type="radio"
              className="sr-only"
              name={name}
              checked={i === ceiling}
              disabled={disabled}
              onChange={() => onChange(step.value)}
            />
            <span>{step.label}</span>
          </label>
        ))}
      </div>
    </div>
  );
}

/** A caption and its chips, the caption naming the group. */
function ChipRow<T>({ label, options, value, onChange, disabled }: {
  label: string;
  options: SegOption<T>[];
  value: T;
  onChange: (value: T) => void;
  disabled?: boolean;
}) {
  const id = useId();
  return (
    <div className="st-dl-qrow">
      <span className="st-dl-ql" id={id}>{label}</span>
      <Segmented variant="chips" labelledBy={id} options={options} value={value} disabled={disabled}
        onChange={onChange} />
    </div>
  );
}

function RawFormat({ id, value, onChange, onBlur }: {
  id: string;
  value: string;
  onChange: (value: string) => void;
  onBlur: () => void;
}) {
  const inputId = useId();
  const hint: ReactNode = (
    <>
      A yt-dlp <code>-f</code> selector such as <code>bv*[height&lt;=720]+ba/b</code>. When set,
      it replaces the choices above; a bad one fails with yt-dlp's own error.
    </>
  );
  return (
    <div className="st-dl-raw" id={id}>
      <label className="st-dl-raw-label" htmlFor={inputId}>Format (-f)</label>
      <input
        id={inputId}
        className="text-input mono"
        value={value}
        placeholder="bv*+ba/b"
        spellCheck={false}
        aria-describedby={`${inputId}-hint`}
        onChange={(e) => onChange(e.currentTarget.value)}
        onBlur={onBlur}
      />
      <p className="st-dl-raw-hint" id={`${inputId}-hint`}>{hint}</p>
    </div>
  );
}

/* ------------------------------------------------------------------ *
 * At once
 * ------------------------------------------------------------------ */

/** Concurrent downloads, with a lane lit for each one that may run. */
function AtOnce() {
  const { settings, save } = useSettingsCtx();
  const limits = LIMITS.max_concurrent_downloads;
  const n = settings.max_concurrent_downloads;
  return (
    <Block className="st-dl-par">
      <div className="st-dl-par-text">
        <h3 className="st-block-title">At once</h3>
        <p className="st-dl-par-hint">How many downloads run side by side.</p>
      </div>
      <div className="st-dl-lanes" aria-hidden="true">
        {Array.from({ length: limits.max }, (_, i) => (
          <i key={i} className={i < n ? "is-on" : undefined} />
        ))}
      </div>
      <Stepper
        value={n}
        min={limits.min}
        max={limits.max}
        label="concurrent downloads"
        onChange={(v) => save("max_concurrent_downloads", clamp(v, limits))}
      />
    </Block>
  );
}
