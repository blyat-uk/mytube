import { useEffect, useId, useLayoutEffect, useMemo, useRef, useState } from "react";
import ChannelPanel from "./ChannelPanel";
import ChannelPicture from "./ChannelPicture";
import type { Backlog } from "./AutoDownloadPrompt";
import {
  IconAutoDownload, IconCaret, IconDownload, IconImportCsv, IconMember, IconSearch,
} from "../Icons";
import { useToast } from "../Toast";
import { api } from "../../api";
import { usePending } from "../../pending";
import { formatCompact } from "../../format";
import type { Channel, ChannelStats } from "../../types";

interface Props {
  channels: Channel[];
  /** Null until the first answer: the counts stay blank rather than read 0. */
  stats: ReadonlyMap<string, ChannelStats> | null;
  folded: boolean;
  onFold: (folded: boolean) => void;
  /** Opens one channel and brings it into view; `n` makes a repeat ask anew. */
  reveal: { id: string; n: number } | null;
  importing: boolean;
  onImportCsv: () => void;
  /** Something changed server-side: the shell reloads, the dialog recounts. */
  onChanged: () => void;
}

/** `--fold-time`: how long the list takes to fold. */
const FOLD_TIME = 260;
/** How long a row's panel takes to open or close, plus a frame or two. */
const PANEL_TIME = 300;

export function zeroStats(channelId: string): ChannelStats {
  return {
    channelId, videos: 0, downloaded: 0, watched: 0, hidden: 0, runtimeSecs: 0,
    latestUpload: null, followerCount: null,
  };
}

/** The filter box's test: a substring of the name or the @handle, any case. */
export function matchesChannel(c: Channel, query: string): boolean {
  const q = query.trim().toLocaleLowerCase();
  if (!q) return true;
  return c.title.toLocaleLowerCase().includes(q)
    || (c.handle ?? "").toLocaleLowerCase().includes(q);
}

/**
 * The Add dialog's subscriptions: every channel at once, which is what makes
 * membership and auto-download readable down a column. A row opens into a
 * panel with the channel's counts, facts and actions; one at a time, so the
 * list stays a list. The whole thing folds away -- the dialog is mostly opened
 * to paste a link -- and the shell remembers which way it was left.
 */
export default function SubscriptionList({
  channels, stats, folded, onFold, reveal, importing, onImportCsv, onChanged,
}: Props) {
  const toast = useToast();
  const uid = useId();
  const bodyId = `${uid}-body`;
  const [filter, setFilter] = useState("");
  const [openId, setOpenId] = useState<string | null>(null);
  // The row folding shut while another opens keeps its panel for the length
  // of the animation; a panel emptied at once would snap rather than close.
  const [closingId, setClosingId] = useState<string | null>(null);
  const [confirmRemove, setConfirmRemove] = useState<string | null>(null);
  /** The channel whose "turn auto-download on" prompt is open, if any. */
  const [autoPrompt, setAutoPrompt] = useState<string | null>(null);
  // Keyed by action and channel, not one flag for the list: joining reads a
  // whole listing, turning auto-download on can queue a whole backlog, and a
  // slow one must not lock the rest of the list.
  const { pending, slow, run } = usePending();
  const act = (key: string) => ({ busy: pending.has(key), spinning: slow.has(key) });
  const rows = useRef(new Map<string, HTMLLIElement>());
  const scrollTimer = useRef<number | undefined>(undefined);

  const shown = useMemo(
    () => (filter.trim() ? channels.filter((c) => matchesChannel(c, filter)) : channels),
    [channels, filter],
  );

  useEffect(() => {
    if (!closingId) return;
    const t = window.setTimeout(() => setClosingId(null), PANEL_TIME);
    return () => window.clearTimeout(t);
  }, [closingId]);

  useEffect(() => () => window.clearTimeout(scrollTimer.current), []);

  // The fold slides between two measured heights: what the list stood at when
  // it was folded (read as the button is pressed, before the class changes),
  // and what it settles at once it is out (read after, with the dialog's own
  // limit already applied). Folded, CSS holds it at 0; the animation only
  // carries it between the two, so nothing is left behind on the element.
  const body = useRef<HTMLDivElement | null>(null);
  const foldFrom = useRef<number | null>(null);
  const firstFold = useRef(true);
  useLayoutEffect(() => {
    const el = body.current;
    const measured = foldFrom.current;
    foldFrom.current = null;
    if (firstFold.current) { firstFold.current = false; return; }
    // Unfolded from elsewhere ("Show in list") starts from nothing.
    const from = measured ?? (folded ? null : 0);
    if (!el || from === null || typeof el.animate !== "function") return;
    if (window.matchMedia?.("(prefers-reduced-motion: reduce)").matches) return;
    const to = el.offsetHeight;
    if (from === to) return;
    el.animate([{ height: `${from}px` }, { height: `${to}px` }], {
      duration: FOLD_TIME, easing: "cubic-bezier(0.2, 0.7, 0.2, 1)",
    });
  }, [folded]);

  function toggleFold() {
    foldFrom.current = body.current?.offsetHeight ?? null;
    onFold(!folded);
  }

  /** Once the panel has opened (and the list unfolded), scroll the whole row
   *  into view: a row near the bottom would otherwise open out of sight. */
  function bringIntoView(id: string) {
    window.clearTimeout(scrollTimer.current);
    scrollTimer.current = window.setTimeout(() => {
      rows.current.get(id)?.scrollIntoView?.({ block: "nearest", behavior: "smooth" });
    }, PANEL_TIME);
  }

  function openRow(id: string | null) {
    if (id === openId) return;
    setClosingId(openId);
    setOpenId(id);
    // What was being asked belonged to the row that closed.
    setConfirmRemove(null);
    setAutoPrompt(null);
    if (id) bringIntoView(id);
  }

  // "Show in list", from a pasted channel you already follow.
  useEffect(() => {
    if (!reveal) return;
    const c = channels.find((x) => x.id === reveal.id);
    if (!c) return;
    if (!matchesChannel(c, filter)) setFilter("");
    openRow(reveal.id);
    bringIntoView(reveal.id);
    // Only a new ask runs this; the list changing under an old one must not.
  }, [reveal]);

  /**
   * Joins or leaves a channel's membership.
   *
   * Joining reads one listing at backfill depth there and then, and the count
   * that comes back is how many members-only uploads it found — RSS never
   * carried them, so they have been invisible for as long as the subscription
   * has existed. Leaving collects nothing further and removes nothing already
   * collected.
   */
  function setMember(c: Channel) {
    void run(`member:${c.id}`, async () => {
      const found = await api.setChannelMember(c.id, !c.member);
      onChanged();
      if (c.member) {
        toast.info(`Left ${c.title}. Videos already collected stay.`);
      } else if (found === 0) {
        toast.info(`No members-only videos found on ${c.title}.`);
      } else {
        toast.success(`${found} members-only video${found === 1 ? "" : "s"} added.`);
      }
    });
  }

  /** Off is immediate and takes nothing back; on goes through the prompt. */
  function toggleAutoDownload(c: Channel) {
    if (pending.has(`auto:${c.id}`)) return;
    if (c.auto_download) {
      setAutoDownload(c, false, null);
    } else if (autoPrompt === c.id) {
      setAutoPrompt(null);
    } else {
      setConfirmRemove(null);
      setAutoPrompt(c.id);
    }
  }

  function setAutoDownload(c: Channel, enabled: boolean, backlog: Backlog | null) {
    setAutoPrompt(null);
    void run(`auto:${c.id}`, async () => {
      const queued = await api.setChannelAutoDownload(c.id, enabled, backlog);
      onChanged();
      if (!enabled) {
        toast.info(`Auto-download off for ${c.title}.`);
      } else {
        toast.success(
          `Auto-download on for ${c.title}.` +
          (queued > 0 ? ` ${queued} video${queued === 1 ? "" : "s"} queued.` : ""),
        );
      }
    });
  }

  function openChannel(c: Channel) {
    // Keyed so a double press opens one browser tab, not two.
    void run(`open:${c.id}`, () => api.openExternal(c.url));
  }

  /** Removal deletes every video the channel has, so it must not run twice --
   *  the confirm stays pressed and disabled until the answer is in. */
  function removeChannel(c: Channel) {
    void run(`remove:${c.id}`, async () => {
      await api.removeChannel(c.id);
      toast.success(`Removed ${c.title} and its videos.`);
      setConfirmRemove(null);
      setOpenId((id) => (id === c.id ? null : id));
      onChanged();
    });
  }

  return (
    <section className={`add-subs${folded ? " is-folded" : ""}`} aria-label="Subscriptions">
      <div className="add-subs-head">
        <button
          type="button"
          className="add-fold"
          aria-expanded={!folded}
          aria-controls={bodyId}
          onClick={toggleFold}
        >
          <span className="add-fold-caret" aria-hidden="true"><IconCaret /></span>
          Subscriptions{" "}
          <span className="section-count">{channels.length}</span>
        </button>
        {/* Hidden while folded: a filter over a list nobody can see would be a
            control that does nothing. Faded rather than removed, so the header
            keeps its height and the fold reads as the list going, not the row
            rebuilding. */}
        <div className="add-subs-tools" inert={folded} aria-hidden={folded || undefined}>
          <label className="add-filter">
            <span className="add-filter-glyph" aria-hidden="true"><IconSearch /></span>
            <input
              type="text"
              className="text-input"
              value={filter}
              placeholder="Filter channels"
              aria-label="Filter channels"
              spellCheck={false}
              onChange={(e) => setFilter(e.currentTarget.value)}
              onKeyDown={(e) => {
                // A filter is cleared before the dialog is closed: Escape in a
                // box with something in it means "never mind the filter".
                if (e.key === "Escape" && filter) {
                  e.stopPropagation();
                  setFilter("");
                }
              }}
            />
          </label>
          <button
            type="button"
            className="btn btn-sm add-import"
            title="Import the channels in a Google Takeout subscriptions CSV"
            onClick={onImportCsv}
            disabled={importing}
          >
            <span className="btn-glyph" aria-hidden="true"><IconImportCsv /></span>
            <span className="add-import-label">{importing ? "Importing…" : "Import CSV"}</span>
          </button>
        </div>
      </div>

      <div className="add-subs-body" id={bodyId} ref={body} inert={folded}>
        {channels.length === 0 ? (
          <p className="add-subs-empty">No channels yet. Paste one above to subscribe.</p>
        ) : shown.length === 0 ? (
          <p className="add-subs-empty">No channel matches “{filter.trim()}”.</p>
        ) : (
          <ul className="add-subs-list">
            {shown.map((c) => {
              const s = stats ? (stats.get(c.id) ?? zeroStats(c.id)) : null;
              const open = openId === c.id;
              return (
                <li
                  key={c.id}
                  ref={(el) => {
                    if (el) rows.current.set(c.id, el);
                    else rows.current.delete(c.id);
                  }}
                  className={`add-ch${open ? " is-open" : ""}${c.terminated ? " is-terminated" : ""}`}
                >
                  <RowHead
                    channel={c}
                    stats={s}
                    open={open}
                    panelId={`${uid}-p-${c.id}`}
                    nameId={`${uid}-n-${c.id}`}
                    onToggle={() => openRow(open ? null : c.id)}
                  />
                  <div
                    className="add-ch-panel"
                    id={`${uid}-p-${c.id}`}
                    role="region"
                    aria-labelledby={`${uid}-n-${c.id}`}
                    inert={!open}
                  >
                    <div className="add-ch-clip">
                      {(open || closingId === c.id) && (
                        <ChannelPanel
                          channel={c}
                          stats={s ?? zeroStats(c.id)}
                          confirming={confirmRemove === c.id}
                          prompting={autoPrompt === c.id}
                          busy={{
                            open: act(`open:${c.id}`),
                            member: act(`member:${c.id}`),
                            auto: act(`auto:${c.id}`),
                            remove: act(`remove:${c.id}`),
                          }}
                          onOpen={() => openChannel(c)}
                          onMember={() => setMember(c)}
                          onAuto={() => toggleAutoDownload(c)}
                          onAutoConfirm={(backlog) => setAutoDownload(c, true, backlog)}
                          onAutoCancel={() => setAutoPrompt(null)}
                          onAskRemove={() => { setAutoPrompt(null); setConfirmRemove(c.id); }}
                          onRemove={() => removeChannel(c)}
                          onKeep={() => setConfirmRemove(null)}
                        />
                      )}
                    </div>
                  </div>
                </li>
              );
            })}
          </ul>
        )}
      </div>
    </section>
  );
}

/**
 * A row's header, which is the button that opens it: the picture, the name
 * (and once open, its @handle and subscriber count), the member and
 * auto-download marks when on, and how many videos it has and has on disk.
 */
function RowHead({ channel: c, stats, open, panelId, nameId, onToggle }: {
  channel: Channel;
  stats: ChannelStats | null;
  open: boolean;
  panelId: string;
  nameId: string;
  onToggle: () => void;
}) {
  const sub = [
    c.handle,
    stats?.followerCount != null
      ? `${formatCompact(stats.followerCount)} subscriber${stats.followerCount === 1 ? "" : "s"}`
      : null,
  ].filter(Boolean).join(" · ");
  return (
    <button
      type="button"
      className="add-ch-head"
      aria-expanded={open}
      aria-controls={panelId}
      title={c.terminated
        ? `YouTube has terminated ${c.title}'s account. It is no longer polled; the videos already collected stay.`
        : undefined}
      onClick={onToggle}
    >
      <ChannelPicture channel={c} />
      <span className="add-ch-who">
        <span className="add-ch-name" id={nameId}>{c.title}</span>
        {c.terminated && <span className="sr-only"> (terminated by YouTube)</span>}
        {sub && <span className="add-ch-sub" aria-hidden={!open || undefined}>{sub}</span>}
      </span>
      {c.member && (
        <span className="add-ch-flag is-member" title="You are a member">
          <IconMember joined />
          <span className="sr-only">, member</span>
        </span>
      )}
      {c.auto_download && (
        <span className="add-ch-flag is-auto" title="New uploads download on their own">
          <IconAutoDownload on />
          <span className="sr-only">, auto-download on</span>
        </span>
      )}
      <span className="add-ch-count">
        {stats && (
          <>
            {stats.videos.toLocaleString("en")} video{stats.videos === 1 ? "" : "s"}
            {" · "}
            {/* Green only when there is something on disk: a column of
                bright zeros would draw the eye to the channels with none. */}
            <b className={stats.downloaded ? "is-some" : undefined}>
              {stats.downloaded.toLocaleString("en")}
            </b>
            <span className="add-ch-count-glyph" aria-hidden="true"><IconDownload /></span>
            <span className="sr-only"> downloaded</span>
          </>
        )}
      </span>
      <span className="add-ch-caret" aria-hidden="true"><IconCaret /></span>
    </button>
  );
}
