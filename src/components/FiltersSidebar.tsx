import { useEffect, useId, useRef, type ReactElement, type ReactNode, type RefObject } from "react";
import {
  IconAutoDownload, IconClose, IconContinue, IconDownload, IconFilters, IconHidden, IconSeries,
  IconStar, IconUnwatched,
} from "./Icons";
import ChannelList from "./ChannelList";
import { LengthRows } from "./LengthPicker";
import SavedViews from "./SavedViews";
import { useDismiss } from "./usePopover";
import { sortsFor, TOGGLES, type ToggleDef, type ToggleId } from "../filters";
import type { ChannelPick } from "../channelPick";
import type { Channel, QuickFilter, SavedView, SortOrder, ViewFilters } from "../types";

/** Each toggle's glyph, worn by its bar chip and its sidebar row alike, so a
 *  filter is recognisable in both. */
export const TOGGLE_ICON: Record<ToggleId, () => ReactElement> = {
  hide_watched: IconUnwatched,
  downloaded_only: IconDownload,
  show_hidden: IconHidden,
  grouped: IconSeries,
  in_progress: IconContinue,
  // The channel switch's glyph, outlined: a component with no props of its own.
  auto_downloading: () => <IconAutoDownload />,
};

/** Matches `--side-out` in App.css: how long the sidebar takes to slide away. */
export const SIDEBAR_OUT_MS = 200;

export type Toggles = Record<ToggleId, { on: boolean; set: (on: boolean) => void }>;

interface Props {
  /** On its way out: still drawn for the slide, but no longer there to use. */
  closing: boolean;
  onClose: (refocus: boolean) => void;
  /** The nav it hangs from. A press anywhere in it -- a starred chip, the
   *  search box, All filters itself -- leaves the sidebar open. */
  nav: RefObject<HTMLElement | null>;
  /** A series is open, so only the sort applies. */
  off: boolean;
  toggles: Toggles;
  grouped: boolean;
  minLength: number;
  onMinLength: (secs: number) => void;
  minParts: number;
  onMinParts: (parts: number) => void;
  sort: SortOrder;
  onSort: (s: SortOrder) => void;
  channels: Channel[];
  pick: ChannelPick;
  onPick: (pick: ChannelPick) => void;
  starred: readonly QuickFilter[];
  onStar: (id: QuickFilter) => void;
  views: SavedView[];
  onViews: (views: SavedView[]) => void;
  onApplyView: (view: SavedView) => void;
  /** The filters on screen and the search box, which a view is held up against. */
  current: ViewFilters;
  search: string;
  channelsLabel: (f: ViewFilters) => string;
  /** Something narrows the list, so Clear has work to do. */
  canClear: boolean;
  onClear: () => void;
}

function Star({ id, label, starred, onStar }: {
  id: QuickFilter; label: string; starred: boolean; onStar: (id: QuickFilter) => void;
}) {
  return (
    <button
      type="button"
      className={`side-star${starred ? " is-on" : ""}`}
      aria-pressed={starred}
      aria-label={`Star ${label}`}
      title={starred ? "In the top bar. Click to take it out" : "Star to keep it in the top bar"}
      onClick={() => onStar(id)}
    >
      <IconStar on={starred} />
    </button>
  );
}

function Section({ title, note, star, dim, children }: {
  title: string; note?: string; star?: ReactNode; dim?: boolean; children: ReactNode;
}) {
  const id = useId();
  return (
    <section className={`side-sec${dim ? " is-dim" : ""}`} aria-labelledby={id}>
      <div className="side-sec-head">
        <h3 className="side-sec-title" id={id}>{title}</h3>
        {note && <span className="side-sec-note">{note}</span>}
        <span className="side-sec-fill" />
        {/* The same column as every row's star, so the stars read down it. */}
        {star ?? <span className="side-star-gap" aria-hidden="true" />}
      </div>
      {children}
    </section>
  );
}

function ToggleRow({ def, on, disabled, title, sub, starred, onStar, onChange }: {
  def: ToggleDef; on: boolean; disabled: boolean; title?: string; sub?: boolean;
  starred: boolean; onStar: (id: QuickFilter) => void; onChange: (on: boolean) => void;
}) {
  const Icon = TOGGLE_ICON[def.id];
  const descId = useId();
  return (
    <div className={`side-row${on ? " is-on" : ""}${sub ? " is-sub" : ""}`}>
      <button
        type="button"
        className="side-toggle"
        aria-pressed={on}
        aria-label={def.label}
        aria-describedby={descId}
        disabled={disabled}
        title={title}
        onClick={() => onChange(!on)}
      >
        <span className="side-glyph" aria-hidden="true"><Icon /></span>
        <span className="side-text">
          <span className="side-label">{def.label}</span>
          <span className="side-desc" id={descId}>{def.desc}</span>
        </span>
        <span className="side-switch" aria-hidden="true"><span /></span>
      </button>
      {/* A star stays live where its filter cannot be used -- ungrouped, or in a
          series: what the bar holds is a choice about the bar, not this view. */}
      <Star id={def.id} label={def.label} starred={starred} onStar={onStar} />
    </div>
  );
}

/**
 * Every filter in one place, floating over the right of the feed: saved views,
 * what is in the list, the series filters, length, the order, then which
 * channels. Changes apply as they are made, like the bar's own chips, so it
 * stays open until Esc, ✕, All filters again, or a press outside the nav. It
 * floats rather than taking room from the grid: a grid that reflowed every
 * time it opened would move the very cards the filters are being judged
 * against.
 *
 * Each filter and each view has a star. A starred one sits in the top bar as
 * well, which is how the bar comes to hold exactly what this person reaches for.
 */
export default function FiltersSidebar(p: Props) {
  const ref = useRef<HTMLElement>(null);
  useDismiss([p.nav], p.onClose, !p.closing);

  // Into the sidebar on opening, so the keyboard can carry on from where the
  // button was; the panel itself rather than a control in it, which would
  // read as having chosen one.
  useEffect(() => {
    ref.current?.focus();
  }, []);

  const hint = p.off ? "Not applied while a series is open" : undefined;
  const starred = new Set(p.starred);
  const row = (def: ToggleDef, sub = false) => {
    const t = p.toggles[def.id];
    return (
      <ToggleRow
        key={def.id}
        def={def}
        on={t.on}
        disabled={p.off || (def.seriesOnly && !p.grouped)}
        title={hint ?? (def.seriesOnly && !p.grouped ? "Turn on Group siblings to use this" : undefined)}
        sub={sub}
        starred={starred.has(def.id)}
        onStar={p.onStar}
        onChange={t.set}
      />
    );
  };
  const show = TOGGLES.filter((t) => !t.seriesOnly && t.id !== "grouped");
  const seriesOnly = TOGGLES.filter((t) => t.seriesOnly);
  const groupedDef = TOGGLES.find((t) => t.id === "grouped")!;

  return (
    <aside
      ref={ref}
      className={`filters-side${p.closing ? " is-closing" : ""}`}
      role="dialog"
      aria-label="All filters"
      aria-hidden={p.closing || undefined}
      inert={p.closing}
      tabIndex={-1}
    >
      <div className="side-head">
        <span className="side-head-glyph" aria-hidden="true"><IconFilters /></span>
        <h2 className="side-title">All filters</h2>
        <button
          type="button"
          className="side-clear"
          disabled={!p.canClear || p.off}
          title={hint ?? "Turn off everything that narrows the list. Grouping and the order stay as they are"}
          onClick={p.onClear}
        >
          Clear
        </button>
        <button
          type="button"
          className="side-x"
          aria-label="Close all filters"
          title="Close (Esc)"
          onClick={() => p.onClose(true)}
        >
          <IconClose />
        </button>
      </div>

      <div className="side-scroll">
        {p.off && (
          <p className="side-note">
            <span className="side-note-glyph" aria-hidden="true"><IconSeries /></span>
            <span>A series shows every part, so only <b>Sort</b> applies while one is open.</span>
          </p>
        )}

        <Section title="Views">
          <SavedViews
            views={p.views}
            current={p.current}
            search={p.search}
            channelsLabel={p.channelsLabel}
            onApply={p.onApplyView}
            onViews={p.onViews}
            off={p.off}
          />
        </Section>

        <Section title="Show" dim={p.off}>
          {show.map((d) => row(d))}
        </Section>

        <Section title="Series" dim={p.off} note={p.grouped ? undefined : "The rest need Group siblings"}>
          {row(groupedDef)}
          {seriesOnly.map((d) => row(d, true))}
        </Section>

        <Section
          title="Length"
          dim={p.off}
          star={<Star id="length" label="Length" starred={starred.has("length")} onStar={p.onStar} />}
        >
          <div className="side-length">
            <LengthRows
              minLength={p.minLength}
              onMinLength={p.onMinLength}
              minParts={p.minParts}
              onMinParts={p.onMinParts}
              grouped={p.grouped}
              disabled={p.off}
              title={hint}
            />
            <p className="side-hint">A series counts all its parts added up; a lone video, its own length.</p>
          </div>
        </Section>

        <Section
          title="Sort"
          star={<Star id="sort" label="Sort" starred={starred.has("sort")} onStar={p.onStar} />}
        >
          <div className="side-radios" role="radiogroup" aria-label="Sort order">
            {sortsFor(p.grouped).map((s) => (
              <button
                key={s.value}
                type="button"
                role="radio"
                aria-checked={p.sort === s.value}
                className={`side-radio${p.sort === s.value ? " is-on" : ""}`}
                onClick={() => p.onSort(s.value)}
              >
                <span className="side-dot" aria-hidden="true" />
                {s.label}
              </button>
            ))}
          </div>
        </Section>

        <Section
          title="Channels"
          dim={p.off}
          star={<Star id="channel_ids" label="Channels" starred={starred.has("channel_ids")} onStar={p.onStar} />}
        >
          <div className="side-channels">
            <ChannelList
              channels={p.channels}
              pick={p.pick}
              onPick={p.onPick}
              disabled={p.off}
              title={hint}
            />
          </div>
        </Section>
      </div>

      <p className="side-foot">
        <span className="side-foot-glyph" aria-hidden="true"><IconStar on /></span>
        Starred views and filters stay in the top bar.
      </p>
    </aside>
  );
}
