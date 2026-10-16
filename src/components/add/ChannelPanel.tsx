import type { ReactNode } from "react";
import AutoDownloadPrompt, { type Backlog } from "./AutoDownloadPrompt";
import {
  IconAutoDownload, IconBusy, IconDelete, IconExternal, IconLastChecked, IconLatestUpload,
  IconMember, IconRuntime, IconSubscribedSince,
} from "../Icons";
import { singleClick } from "../../pending";
import { formatDay, formatLibraryRuntime, formatRelative, percentOf } from "../../format";
import type { Channel, ChannelStats } from "../../types";

/** Whether one of the panel's calls is out, and whether it has been out long
 *  enough to spin (`usePending`). */
export interface Busy { busy: boolean; spinning: boolean }

interface Props {
  channel: Channel;
  stats: ChannelStats;
  /** The removal confirmation stands in for the actions row. */
  confirming: boolean;
  /** "Turn auto-download on" is asking how far back to reach. */
  prompting: boolean;
  busy: { open: Busy; member: Busy; auto: Busy; remove: Busy };
  onOpen: () => void;
  onMember: () => void;
  onAuto: () => void;
  onAutoConfirm: (backlog: Backlog | null) => void;
  onAutoCancel: () => void;
  onAskRemove: () => void;
  onRemove: () => void;
  onKeep: () => void;
}

/**
 * An opened channel, "two tiers": its four counts as tiles, then -- under a
 * dashed rule, in the same four columns -- four facts, each a glyph, a small
 * label and a value, so none of them has to be read out of a sentence. Then
 * what can be done with it, in words: the panel has the room the old row of
 * bare glyphs did not.
 */
export default function ChannelPanel(p: Props) {
  const { channel: c, stats: s } = p;
  return (
    <div className="add-ch-panel-in">
      <div className="add-stats">
        <Tile value={s.videos} label="videos" />
        <Tile value={s.downloaded} label="downloaded" tone="is-ok" />
        <Tile value={s.watched} label={`watched · ${percentOf(s.watched, s.videos)}%`} tone="is-accent" />
        <Tile value={s.hidden} label="hidden" />
        <div className="add-facts">
          <Fact glyph={<IconRuntime />} label="Runtime" value={formatLibraryRuntime(s.runtimeSecs)} />
          <Fact
            glyph={<IconLatestUpload />}
            label="Latest upload"
            value={s.latestUpload ? formatRelative(s.latestUpload) : "—"}
          />
          <Fact glyph={<IconSubscribedSince />} label="Subscribed" value={formatDay(c.added_at)} />
          <Fact
            glyph={<IconLastChecked />}
            label="Last checked"
            value={c.last_polled_at ? formatRelative(c.last_polled_at) : "Never"}
          />
        </div>
      </div>

      {c.terminated && (
        <p className="add-ch-note">
          YouTube has terminated this account, so it is no longer checked. The videos already
          collected stay in your library.
        </p>
      )}

      {p.confirming ? (
        <div className="add-ch-confirm" role="group" aria-label={`Remove ${c.title}?`}>
          <p className="add-ch-confirm-text">
            {s.videos > 0
              ? <>Remove <b>{c.title}</b> and delete its {s.videos} video{s.videos === 1 ? "" : "s"}?</>
              : <>Remove <b>{c.title}</b>?</>}
          </p>
          <div className="add-ch-confirm-buttons">
            <PanelButton
              glyph={<IconDelete />}
              label="Remove"
              tone="btn-danger"
              {...p.busy.remove}
              onClick={p.onRemove}
            />
            <PanelButton
              label="Keep"
              tone="btn-quiet"
              // Too late to keep it once the removal is out.
              busy={p.busy.remove.busy}
              onClick={p.onKeep}
            />
          </div>
        </div>
      ) : (
        <div className="add-ch-actions">
          <PanelButton glyph={<IconExternal />} label="Open on YouTube" {...p.busy.open} onClick={p.onOpen} />
          {/* A terminated account has no uploads left to collect, members-only
              or otherwise, and nothing new will ever arrive to auto-download. */}
          {!c.terminated && (
            <PanelButton
              glyph={<IconMember joined={c.member} />}
              label={c.member ? "Member" : "Join members"}
              tone={c.member ? "is-on" : undefined}
              pressed={c.member}
              title={
                c.member
                  ? `You are a member of ${c.title}. Its members-only uploads are collected with the rest.`
                  : `Joined ${c.title}? Collect its members-only uploads too — the RSS feed never carries them.`
              }
              {...p.busy.member}
              onClick={p.onMember}
            />
          )}
          {!c.terminated && (
            <PanelButton
              glyph={<IconAutoDownload on={c.auto_download} />}
              label="Auto-download"
              tone={c.auto_download || p.prompting ? "is-on" : undefined}
              pressed={c.auto_download}
              title={
                c.auto_download
                  ? `New uploads from ${c.title} download on their own. Click to stop; nothing already queued or downloaded is touched.`
                  : `Download new uploads from ${c.title} automatically.`
              }
              {...p.busy.auto}
              onClick={p.onAuto}
            />
          )}
          <PanelButton
            glyph={<IconDelete />}
            label="Remove"
            tone="btn-quiet is-danger"
            className="add-ch-remove"
            onClick={p.onAskRemove}
          />
        </div>
      )}

      {p.prompting && (
        <AutoDownloadPrompt channel={c} onConfirm={p.onAutoConfirm} onCancel={p.onAutoCancel} />
      )}
    </div>
  );
}

function Tile({ value, label, tone }: { value: number; label: string; tone?: string }) {
  return (
    <div className={`add-tile${tone ? ` ${tone}` : ""}`}>
      <b>{value.toLocaleString("en")}</b>
      <span>{label}</span>
    </div>
  );
}

function Fact({ glyph, label, value }: { glyph: ReactNode; label: string; value: string }) {
  return (
    <div className="add-fact">
      <span className="add-fact-glyph" aria-hidden="true">{glyph}</span>
      <span className="add-fact-label">{label}</span>
      <span className="add-fact-value">{value}</span>
    </div>
  );
}

/**
 * One of the panel's buttons: a glyph and a word. `busy` disables it the
 * moment its call goes out; `spinning` swaps the glyph for IconBusy once the
 * call is slow enough to notice. The second click of a double-click is
 * dropped, since on a toggle it would join and then leave.
 */
function PanelButton({ glyph, label, tone, className, pressed, title, busy, spinning, onClick }: {
  glyph?: ReactNode;
  label: string;
  tone?: string;
  className?: string;
  pressed?: boolean;
  title?: string;
  busy?: boolean;
  spinning?: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      className={["btn btn-sm", tone, className, spinning && "is-spinning"].filter(Boolean).join(" ")}
      title={title}
      aria-pressed={pressed}
      aria-busy={busy || undefined}
      disabled={busy}
      onClick={singleClick(onClick)}
    >
      {glyph && <span className="btn-glyph" aria-hidden="true">{spinning ? <IconBusy /> : glyph}</span>}
      {label}
    </button>
  );
}
