import { useEffect, useRef, useState, type ReactElement } from "react";
import {
  IconClose, IconDownload, IconHidden, IconSeries, IconSettings,
  IconSubscriptions, IconUnwatched,
} from "./Icons";
import type { Channel, SortOrder } from "../types";

export type Tab = "subscriptions" | "downloads" | "settings";

const TABS: { id: Tab; label: string; icon: () => ReactElement }[] = [
  { id: "subscriptions", label: "Subscriptions", icon: IconSubscriptions },
  { id: "downloads", label: "Downloads", icon: IconDownload },
  { id: "settings", label: "Settings", icon: IconSettings },
];

/** What the nav says about an open series, once the view has counted it. */
export interface SeriesCrumb {
  /** The series' shared name: a group's stem, or the anchor video's title. */
  name: string;
  /** Parts found so far. 0 while the first page is still in flight. */
  parts: number;
  /** Their runtime added up, already worded; "" when none is known yet. */
  runtime: string;
  /** Parts with no duration yet, which mark the total with a "+". */
  unknown: number;
}

/**
 * The tail of the breadcrumb, or "" while there is nothing to say.
 *
 * A series of one is the answer "there are no other parts", which is worth
 * saying out loud — silence there reads as still loading.
 */
function tallyText(s: SeriesCrumb): string {
  if (s.parts === 0) return "";
  if (s.parts === 1) return "no other parts found";
  const runtime = s.runtime ? ` · ${s.runtime}${s.unknown > 0 ? "+" : ""}` : "";
  return `${s.parts} parts${runtime}`;
}

interface Props {
  tab: Tab;
  onTab: (t: Tab) => void;
  channels: Channel[];
  channelId: string | null;
  onChannelId: (id: string | null) => void;
  search: string;
  onSearch: (s: string) => void;
  hideWatched: boolean;
  onHideWatched: (b: boolean) => void;
  downloadedOnly: boolean;
  onDownloadedOnly: (b: boolean) => void;
  sort: SortOrder;
  onSort: (s: SortOrder) => void;
  showHidden: boolean;
  onShowHidden: (v: boolean) => void;
  grouped: boolean;
  onGrouped: (v: boolean) => void;
  /** The open series, or null for the ordinary feed. Replaces the tab strip. */
  series: SeriesCrumb | null;
  onExitSeries: () => void;
  polling: boolean;
  onRefresh: () => void;
  onAdd: () => void;
}

export default function TopNav(p: Props) {
  const [text, setText] = useState(p.search);
  const emitted = useRef(p.search);

  // Debounce the search box so a long query is one query, not twelve.
  useEffect(() => {
    if (text === emitted.current) return;
    const t = window.setTimeout(() => {
      emitted.current = text;
      p.onSearch(text);
    }, 250);
    return () => window.clearTimeout(t);
  }, [text, p.onSearch]);

  const searchRef = useRef<HTMLInputElement | null>(null);
  // A shortcut can arrive on a tab that has neither the search box nor the
  // filter row, so the handler — which is bound once — reads what it needs
  // through a ref rather than capturing a stale set of props.
  const live = useRef(p);
  live.current = p;
  /** Set when Ctrl+F had to switch tabs first; the box is focused on arrival. */
  const [focusWanted, setFocusWanted] = useState(false);

  // Three chords, one listener. Each is claimed with preventDefault before it
  // is acted on, because the webview has its own idea about all three: Ctrl+F
  // opens a find bar that only ever searches the rendered page (the grid loads
  // 100 videos at a time, so a native find silently misses the rest), and
  // Ctrl+R reloads — which would throw away the grid's scroll position and
  // every page it has fetched to do a worse version of what Ctrl+R now means.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      // Alt and Shift are rejected rather than ignored: Ctrl+Shift+N is the
      // browser's private window and Ctrl+Alt+<letter> is a desktop-level
      // binding on most Linux setups, so answering to either shadows it.
      if (!(e.ctrlKey || e.metaKey) || e.altKey || e.shiftKey) return;
      // Lowercased so Caps Lock is not a way to lose the shortcut.
      const key = e.key.toLowerCase();
      if (key !== "f" && key !== "r" && key !== "n") return;
      e.preventDefault();
      // A modal owns the keyboard while it is open: pulling focus to a box
      // behind the scrim would strand whoever is typing in it, and the other
      // two would act behind a dialog nobody has finished with.
      if (document.querySelector(".modal-backdrop")) return;

      const { tab, onTab, polling, onRefresh, onAdd } = live.current;

      // These two stand in for the buttons at the end of the nav, so they do
      // what those buttons do — including nothing at all while the refresh is
      // disabled mid-poll. Both are reachable from every tab, as the buttons
      // are: neither lives behind the Subscriptions-only filter row.
      if (key === "r") {
        if (!polling) onRefresh();
        return;
      }
      if (key === "n") {
        onAdd();
        return;
      }

      const el = searchRef.current;
      if (el) {
        if (el.disabled) return;
        el.focus();
        el.select();
        return;
      }
      // No box on this tab: the shortcut means "find a video" everywhere, so it
      // goes to the tab that has one rather than doing nothing.
      if (tab !== "subscriptions") {
        onTab("subscriptions");
        setFocusWanted(true);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  useEffect(() => {
    if (!focusWanted) return;
    setFocusWanted(false);
    const el = searchRef.current;
    if (!el || el.disabled) return;
    el.focus();
    el.select();
  }, [focusWanted, p.tab]);

  const showFilters = p.tab === "subscriptions";
  // A series shows every part regardless of these, so rather than let them look
  // live and do nothing, they are switched off and say why.
  const off = p.series !== null;
  const offHint = off ? "Not applied while a series is open" : undefined;

  const channelTitle = p.channelId
    ? p.channels.find((c) => c.id === p.channelId)?.title ?? "All channels"
    : "All channels";

  return (
    <header className="topnav">
      <div className="topnav-row">
        <span className="brand-mark" aria-hidden="true" />

        {p.series ? (
          /* The series replaces the tab strip rather than adding a bar of its
             own: it is where you are, not a notice about where you are. */
          <nav className="crumb" aria-label="Breadcrumb">
            <button type="button" className="crumb-parent" onClick={p.onExitSeries}>
              Subscriptions
            </button>
            <span className="crumb-sep" aria-hidden="true">›</span>
            <span className="crumb-here" aria-current="page">
              <span className="crumb-name" title={p.series.name}>{p.series.name}</span>
              <button
                type="button"
                className="crumb-x"
                onClick={p.onExitSeries}
                aria-label="Leave this series"
                title="Leave this series (Esc)"
              >
                <IconClose />
              </button>
            </span>
            {/* The count lands a moment after the crumb does, so it is spoken
                when it arrives rather than sitting there unannounced. */}
            {tallyText(p.series) && (
              <span className="crumb-meta" role="status">{tallyText(p.series)}</span>
            )}
          </nav>
        ) : (
          <nav className="tabs" aria-label="Sections">
            {TABS.map((t) => (
              <button
                key={t.id}
                type="button"
                className={`tab${p.tab === t.id ? " is-active" : ""}`}
                aria-current={p.tab === t.id ? "page" : undefined}
                aria-label={t.label}
                title={t.label}
                onClick={() => p.onTab(t.id)}
              >
                <span className="tab-glyph" aria-hidden="true"><t.icon /></span>
                <span className="tab-label">{t.label}</span>
              </button>
            ))}
          </nav>
        )}

        <div className="topnav-spacer" />

        {showFilters && (
          <>
            {/* What is in the list ------------------------------------------ */}
            <div className="seg" role="group" aria-label="Filters">
              <button
                type="button"
                className={`seg-btn${p.hideWatched ? " is-on" : ""}`}
                aria-pressed={p.hideWatched}
                aria-label="Unwatched"
                disabled={off}
                title={offHint ?? "Show only videos you have not watched"}
                onClick={() => p.onHideWatched(!p.hideWatched)}
              >
                <span className="seg-glyph" aria-hidden="true"><IconUnwatched /></span>
                <span className="seg-label">Unwatched</span>
              </button>
              <button
                type="button"
                className={`seg-btn${p.downloadedOnly ? " is-on" : ""}`}
                aria-pressed={p.downloadedOnly}
                aria-label="Downloaded"
                disabled={off}
                title={offHint ?? "Show only videos you have downloaded"}
                onClick={() => p.onDownloadedOnly(!p.downloadedOnly)}
              >
                <span className="seg-glyph" aria-hidden="true"><IconDownload /></span>
                <span className="seg-label">Downloaded</span>
              </button>
              <button
                type="button"
                className={`seg-btn${p.showHidden ? " is-on" : ""}`}
                aria-pressed={p.showHidden}
                aria-label="Include hidden"
                disabled={off}
                title={offHint ?? "Also show videos you have hidden, so you can un-hide them"}
                onClick={() => p.onShowHidden(!p.showHidden)}
              >
                <span className="seg-glyph" aria-hidden="true"><IconHidden /></span>
                <span className="seg-label">Include hidden</span>
              </button>
            </div>

            <div className="search">
              <span className="search-glyph" aria-hidden="true">⌕</span>
              <input
                ref={searchRef}
                className="search-input"
                type="search"
                value={text}
                placeholder="Search titles and channels"
                aria-label="Search videos"
                disabled={off}
                title={offHint ?? "Ctrl+F to focus"}
                onChange={(e) => setText(e.currentTarget.value)}
              />
            </div>

            <select
              className="select channel-select"
              value={p.channelId ?? ""}
              aria-label="Filter by channel"
              disabled={off}
              // The box narrows with the window, so the full name lives here.
              title={offHint ?? channelTitle}
              onChange={(e) => p.onChannelId(e.currentTarget.value || null)}
            >
              <option value="">All channels</option>
              {p.channels.map((c) => (
                <option key={c.id} value={c.id}>{c.title}</option>
              ))}
            </select>

            {/* ...and how it is shown -------------------------------------- */}
            <div className="topnav-rule" aria-hidden="true" />

            <button
              type="button"
              className={`chip${p.grouped ? " is-on" : ""}`}
              aria-pressed={p.grouped}
              aria-label="Grouped"
              disabled={off}
              title={offHint ?? "Collapse each channel's multi-part uploads into one card"}
              onClick={() => p.onGrouped(!p.grouped)}
            >
              <span className="chip-glyph" aria-hidden="true"><IconSeries /></span>
              <span className="chip-label">Grouped</span>
            </button>

            <select
              className="select sort-select"
              value={p.sort}
              aria-label="Sort order"
              onChange={(e) => p.onSort(e.currentTarget.value as SortOrder)}
            >
              <option value="newest">Newest first</option>
              <option value="oldest">Oldest first</option>
              <option value="channel">By channel</option>
              <option value="length">Longest first</option>
              {/* Only groups have a part count to rank by; ungrouped this
                  option would silently be "newest". */}
              {p.grouped && <option value="parts">Most parts first</option>}
            </select>
          </>
        )}

        <button
          type="button"
          className={`icon-btn${p.polling ? " is-spinning" : ""}`}
          onClick={p.onRefresh}
          disabled={p.polling}
          title={p.polling ? "Checking for new videos…" : "Check subscriptions for new videos (Ctrl+R)"}
        >
          <span aria-hidden="true">↻</span>
          <span className="sr-only">Refresh subscriptions</span>
        </button>

        <button
          type="button"
          className="btn btn-primary btn-add"
          title="Add a channel or video (Ctrl+N)"
          onClick={p.onAdd}
        >
          <span aria-hidden="true">+</span>
          <span className="btn-add-label">Add</span>
        </button>
      </div>
    </header>
  );
}
