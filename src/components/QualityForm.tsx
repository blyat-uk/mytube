import { useId, useState, type ReactNode } from "react";
import Field, { hintId } from "./Field";
import {
  AUDIO_FORMAT_OPTIONS, CODEC_OPTIONS, CONTAINER_OPTIONS, FPS_OPTIONS, HEIGHT_OPTIONS,
  MODE_OPTIONS, audioChoices, audioLanguage, codecChoices, formatSize, fpsChoices, hdrChoices,
  heightChoices, languageChoices, overriddenByFormat, pickSize, selectedAudio, selectedVideo,
  withAudio, withAudioLanguage, withVideo, type Choice, type ExactPick, type Option,
} from "../quality";
import type { Quality, VideoFormats } from "../types";

interface GenericProps {
  /** Edits a `Quality` with preference selects: what Settings holds. */
  source: "generic";
  value: Quality;
  /** Typing in the raw field: held, not saved, until blur. */
  onEdit: (q: Quality) => void;
  /** A select changed, which has no "done editing" moment: saved now. */
  onPick: (q: Quality) => void;
  onBlur?: () => void;
}

interface ExactProps {
  /** Offers only what one video has, every option with its size. */
  source: "exact";
  formats: VideoFormats;
  value: ExactPick;
  onChange: (pick: ExactPick) => void;
}

type Props = GenericProps | ExactProps;

/**
 * One form, two sources. Settings edits preferences that yt-dlp sorts by and
 * never fails over; the custom-download dialog picks exact streams out of what
 * one video was found to have. Both keep the Advanced raw `-f` field, and a
 * non-empty one disables the selects it replaces rather than leaving them
 * looking as if they still did something.
 */
export default function QualityForm(p: Props) {
  const id = useId();
  return p.source === "generic" ? <Generic {...p} id={id} /> : <Exact {...p} id={id} />;
}

function Generic({ value: q, onEdit, onPick, onBlur, id }: GenericProps & { id: string }) {
  const raw = overriddenByFormat(q);
  const set = <K extends keyof Quality>(key: K, v: Quality[K]) => onPick({ ...q, [key]: v });
  return (
    <div className="quality-form">
      <div className="settings-grid">
        <OptionSelect
          id={`${id}-mode`} label="Mode" options={MODE_OPTIONS} value={q.mode}
          onChange={(v) => set("mode", v)}
        />
        {q.mode === "video" ? (
          <>
            <OptionSelect
              id={`${id}-height`} label="Resolution" options={HEIGHT_OPTIONS} value={q.max_height}
              disabled={raw} onChange={(v) => set("max_height", v)}
            />
            <OptionSelect
              id={`${id}-codec`} label="Codec" options={CODEC_OPTIONS} value={q.vcodec}
              disabled={raw} onChange={(v) => set("vcodec", v)}
            />
            <OptionSelect
              id={`${id}-fps`} label="Frame rate" options={FPS_OPTIONS} value={q.fps}
              disabled={raw} onChange={(v) => set("fps", v)}
            />
            <OptionSelect
              id={`${id}-container`} label="Container" options={CONTAINER_OPTIONS}
              value={q.container} onChange={(v) => set("container", v)}
            />
          </>
        ) : (
          <OptionSelect
            id={`${id}-audio-format`} label="Audio format" options={AUDIO_FORMAT_OPTIONS}
            value={q.audio_format} onChange={(v) => set("audio_format", v)}
          />
        )}
      </div>
      <p className="field-hint quality-note">
        {q.mode === "video"
          ? "Preferences, not requirements: a video without them downloads in the closest thing it has."
          : "Downloads the best audio stream; anything but Original is converted by ffmpeg."}
      </p>
      <Advanced
        id={`${id}-format`}
        value={q.format}
        onChange={(format) => onEdit({ ...q, format })}
        onBlur={onBlur}
      />
    </div>
  );
}

function Exact({ formats: f, value: pick, onChange, id }: ExactProps & { id: string }) {
  const raw = overriddenByFormat(pick);
  const video = selectedVideo(f, pick);
  const audio = selectedAudio(f, pick);
  const modes = MODE_OPTIONS.filter((m) => (m.value === "video" ? f.video.length : f.audio.length));
  const languages = languageChoices(f);
  const size = raw ? null : pickSize(f, pick);

  return (
    <div className="quality-form">
      <div className="settings-grid">
        {modes.length > 1 && (
          <OptionSelect
            id={`${id}-mode`} label="Mode" options={MODE_OPTIONS} value={pick.mode}
            onChange={(mode) => onChange({ ...pick, mode })}
          />
        )}
        {pick.mode === "video" && video && (
          <>
            <TrackSelect
              id={`${id}-height`} label="Resolution" choices={heightChoices(f, video)}
              current={video.height} disabled={raw}
              onChange={(t) => onChange(withVideo(pick, t))}
            />
            <TrackSelect
              id={`${id}-codec`} label="Codec" choices={codecChoices(f, video)}
              current={video.vcodec} disabled={raw}
              onChange={(t) => onChange(withVideo(pick, t))}
            />
            <TrackSelect
              id={`${id}-fps`} label="Frame rate" choices={fpsChoices(f, video)} onlyIfChoice
              current={video.fps} disabled={raw}
              onChange={(t) => onChange(withVideo(pick, t))}
            />
            <TrackSelect
              id={`${id}-hdr`} label="Dynamic range" choices={hdrChoices(f, video)} onlyIfChoice
              current={video.hdr} disabled={raw}
              onChange={(t) => onChange(withVideo(pick, t))}
            />
            <OptionSelect
              id={`${id}-container`} label="Container" options={CONTAINER_OPTIONS}
              value={pick.container} onChange={(container) => onChange({ ...pick, container })}
            />
          </>
        )}
        {audio && languages.length > 1 && (
          <OptionSelect
            id={`${id}-language`} label="Language" options={languages}
            value={audioLanguage(audio)} disabled={raw}
            onChange={(lang) => onChange(withAudioLanguage(f, pick, lang))}
          />
        )}
        {audio && (
          <TrackSelect
            id={`${id}-audio`} label="Audio track" choices={audioChoices(f, audioLanguage(audio))}
            current={audio.id} disabled={raw}
            onChange={(t) => onChange(withAudio(pick, t))}
          />
        )}
        {pick.mode === "audio" && (
          <OptionSelect
            id={`${id}-audio-format`} label="Audio format" options={AUDIO_FORMAT_OPTIONS}
            value={pick.audio_format} onChange={(audio_format) => onChange({ ...pick, audio_format })}
          />
        )}
      </div>
      {size !== null && (
        <p className="field-hint quality-note">Estimated size {formatSize(size)}</p>
      )}
      <Advanced
        id={`${id}-format`}
        value={pick.format}
        onChange={(format) => onChange({ ...pick, format })}
      />
    </div>
  );
}

/** A select over fixed options. Values are indexes, so a number or a boolean
 *  survives the trip through the DOM's strings unchanged. */
function OptionSelect<T>({ id, label, options, value, disabled, onChange }: {
  id: string; label: string; options: Option<T>[]; value: T; disabled?: boolean;
  onChange: (v: T) => void;
}) {
  const at = options.findIndex((o) => o.value === value);
  return (
    <Field label={label} htmlFor={id}>
      <select
        id={id}
        className="select settings-select quality-select"
        value={String(Math.max(0, at))}
        disabled={disabled}
        onChange={(e) => onChange(options[Number(e.currentTarget.value)].value)}
      >
        {options.map((o, i) => <option key={i} value={i}>{o.label}</option>)}
      </select>
    </Field>
  );
}

/**
 * A select whose every option is a real track. The row shows the current
 * track's attribute; choosing another hands back the track that choice lands
 * on. `onlyIfChoice` hides a row with a single answer — every frame rate on a
 * video being 25 is not a decision to put in front of anyone.
 */
function TrackSelect<T>({ id, label, choices, current, disabled, onlyIfChoice, onChange }: {
  id: string; label: string; choices: Choice<T>[]; current: T; disabled?: boolean;
  onlyIfChoice?: boolean; onChange: (trackId: string) => void;
}) {
  if (onlyIfChoice && choices.length < 2) return null;
  const at = choices.findIndex((c) => c.value === current);
  return (
    <Field label={label} htmlFor={id}>
      <select
        id={id}
        className="select settings-select quality-select"
        value={String(Math.max(0, at))}
        disabled={disabled}
        onChange={(e) => onChange(choices[Number(e.currentTarget.value)].trackId)}
      >
        {choices.map((c, i) => <option key={i} value={i}>{c.label}</option>)}
      </select>
    </Field>
  );
}

function Advanced({ id, value, onChange, onBlur }: {
  id: string; value: string; onChange: (v: string) => void; onBlur?: () => void;
}) {
  // Open from the start when there is something in it: a raw format hidden
  // behind a closed disclosure would leave the disabled selects unexplained.
  const [open, setOpen] = useState(() => value.trim() !== "");
  const hint: ReactNode = (
    <>
      A yt-dlp <code>-f</code> selector such as <code>bv*[height&lt;=720]+ba/b</code>. When set,
      it replaces the choices above; a bad one fails with yt-dlp's own error.
    </>
  );
  return (
    <details
      className="quality-advanced"
      open={open}
      onToggle={(e) => setOpen(e.currentTarget.open)}
    >
      <summary>Advanced</summary>
      <Field label="Format (-f)" htmlFor={id} hint={hint}>
        <input
          id={id}
          className="text-input mono"
          value={value}
          placeholder="bv*+ba/b"
          spellCheck={false}
          aria-describedby={hintId(id)}
          onChange={(e) => onChange(e.currentTarget.value)}
          onBlur={onBlur}
        />
      </Field>
    </details>
  );
}
