import { useCallback, useEffect, useRef, useState, type ReactElement } from "react";
import {
  IconAutoDownload, IconBusy, IconCaret, IconClose, IconDownload, IconFilters, IconHidden,
  IconLength, IconNewRelease, IconSearch, IconSeries, IconSettings, IconShow, IconSubscriptions,
  IconView,
} from "./Icons";
import FiltersSidebar, { SIDEBAR_OUT_MS, TOGGLE_ICON, type Toggles } from "./FiltersSidebar";
import { ChannelButton, ChannelPopover } from "./ChannelPicker";
import { LengthPopover } from "./LengthPicker";
import type { CallState, SeriesActions } from "./SubscriptionsView";
import { useToolbarFit } from "../toolbarFit";
import { singleClick } from "../pending";
import {
  activeFilters, describeFilters, lengthLabel, sortsFor, toggleDef, toggleStar, viewIsShowing,
  type ToggleId,
} from "../filters";
import { EVERY_CHANNEL, pickLabel, type ChannelPick } from "../channelPick";
import type { AppUpdate, Channel, QuickFilter, SavedView, SortOrder, ViewFilters } from "../types";

export type Tab = "subscriptions" | "downloads" | "settings";

const TABS: { id: Tab; label: string; icon: () => ReactElement }[] = [
  { id: "subscriptions", label: "Subscriptions", icon: IconSubscriptions },
  { id: "downloads", label: "Downloads", icon: IconDownload },
  { id: "settings", label: "Settings", icon: IconSettings },
];

/** What the series bar says about an open series, once the view has counted it. */
export interface SeriesInfo {
  /** The series' shared name: a group's stem, or the anchor video's title. */
  name: string;
  /** Parts found so far. 0 while the first page is still in flight. */
  parts: number;
  /** Their runtime added up, already worded; "" when none is known yet. */
  runtime: string;
  /** Parts with no duration yet, which mark the total with a "+". */
  unknown: number;
  /** The channel it is from, when known. */
  channel?: string;
}

/**
 * The series bar's second half, or "" while there is nothing to say.
 *
 * A series of one is the answer "there are no other parts", which is worth
 * saying out loud — silence there reads as still loading.
 */
function metaText(s: SeriesInfo): string {
  if (s.parts === 0) return "";
  const tally = s.parts === 1
    ? "no other parts found"
    : `${s.parts} parts${s.runtime ? ` · ${s.runtime}${s.unknown > 0 ? "+" : ""}` : ""}`;
  return s.channel ? `${tally} · ${s.channel}` : tally;
}

/**
 * One of the series bar's actions: a glyph and its words, the words folding
 * away on a narrow window (App.css) while the glyph, the name and the tooltip
 * stay. Disabled the moment its call goes out; the glyph turns only once the
 * call is slow enough to notice.
 */
function SeriesAction({ glyph, label, title, state, danger, onClick }: {
  glyph: ReactElement;
  label: string;
  title: string;
  state: CallState;
  danger?: boolean;
  onClick: () => void;
}) {
  const spinning = state === "slow";
  return (
    <button
      type="button"
      className={["series-act", danger && "is-danger", spinning && "is-spinning"].filter(Boolean).join(" ")}
      aria-label={label}
      title={title}
      aria-busy={state !== "idle" || undefined}
      disabled={state !== "idle"}
      // Each press changes what is under the pointer -- Hide turns into
      // Un-hide as soon as the call is back -- so a double-click's second half
      // would undo the first.
      onClick={singleClick(onClick)}
    >
      <span className="series-act-glyph" aria-hidden="true">{spinning ? <IconBusy /> : glyph}</span>
      <span className="series-act-label">{label}</span>
    </button>
  );
}

/**
 * What the series card's menu offers, on the bar of the series it stands for.
 * Hide (or Un-hide) leads because it is always there, and the auto-download
 * switch, always there too, follows it; Download comes and goes with what is
 * left to fetch, and last in the row its going moves nothing. Ahead of the
 * other two it would slide them under the pointer that had just pressed it.
 */
function SeriesActionButtons({ a }: { a: SeriesActions }) {
  const parts = `${a.due} ${a.due === 1 ? "part" : "parts"}`;
  return (
    <div className="series-acts" role="group" aria-label="Series actions">
      {a.marked ? (
        <SeriesAction
          glyph={<IconShow />}
          label="Un-hide series"
          title="Bring every part back, and let parts that join the series later into the feed"
          state={a.hiding}
          onClick={a.onUnhide}
        />
      ) : (
        <SeriesAction
          glyph={<IconHidden />}
          label="Hide series"
          title="Hide every part, and any part that joins the series later"
          state={a.hiding}
          danger
          onClick={a.onHide}
        />
      )}
      {a.autoDownload ? (
        <SeriesAction
          glyph={<IconAutoDownload on />}
          label="Stop auto-download"
          title="New parts stop downloading on their own. Nothing already downloaded or queued is touched"
          state={a.autoing}
          onClick={a.onStopAutoDownload}
        />
      ) : a.autoAvailable && (
        <SeriesAction
          glyph={<IconAutoDownload />}
          label="Auto-download"
          title="Download every part that joins this series from now on, as soon as it arrives"
          state={a.autoing}
          onClick={a.onAutoDownload}
        />
      )}
      {a.due > 0 && (
        <SeriesAction
          glyph={<IconDownload />}
          label={`Download ${parts}`}
          title={`Download the ${parts} not downloaded yet, oldest first`}
          state={a.downloading}
          onClick={a.onDownload}
        />
      )}
    </div>
  );
}

/**
 * True while `open`, and for `ms` after it goes false: long enough for a panel
 * to play its way out before it is unmounted.
 */
function usePresence(open: boolean, ms: number): boolean {
  const [lingering, setLingering] = useState(open);
  useEffect(() => {
    if (open) {
      setLingering(true);
      return;
    }
    const t = window.setTimeout(() => setLingering(false), ms);
    return () => window.clearTimeout(t);
  }, [open, ms]);
  return open || lingering;
}

const isToggle = (id: QuickFilter): id is ToggleId =>
  id !== "length" && id !== "sort" && id !== "channel_ids";

interface Props {
  tab: Tab;
  onTab: (t: Tab) => void;
  channels: Channel[];
  /** The channels the feed shows -- or, `except`, leaves out; sorted, empty
   *  for every channel. */
  channelPick: ChannelPick;
  onChannelPick: (pick: ChannelPick) => void;
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
  /** The Length filter: at least this long (seconds), and while grouped at
   *  least this many parts. 0 is any. */
  minLength: number;
  onMinLength: (secs: number) => void;
  minParts: number;
  onMinParts: (parts: number) => void;
  inProgress: boolean;
  onInProgress: (v: boolean) => void;
  autoDownloading: boolean;
  onAutoDownloading: (v: boolean) => void;
  /** The filters starred into the bar, in `QUICK_ORDER`. */
  starred: QuickFilter[];
  onStarred: (ids: QuickFilter[]) => void;
  /** Named filter sets; a starred one has a chip in the bar, ahead of the
   *  starred filters. */
  savedViews: SavedView[];
  onSavedViews: (views: SavedView[]) => void;
  onApplyView: (view: SavedView) => void;
  /** The Downloads tab's own search, which its box in this row holds. */
  downloadSearch: string;
  onDownloadSearch: (s: string) => void;
  /** The open series, or null for the ordinary feed. Opens the series bar. */
  series: SeriesInfo | null;
  /** What the series bar can do with the open series, once its parts are in.
   *  Absent or null, it offers nothing -- as for a series of one. */
  seriesActions?: SeriesActions | null;
  onExitSeries: () => void;
  polling: boolean;
  onRefresh: () => void;
  onAdd: () => void;
  /** A release newer than this build, or null. The backend already holds it
   *  back while update checks are off, so non-null means "say so". */
  update: AppUpdate | null;
  /** Opens Settings at About, where the update is spelled out. */
  onOpenAbout: () => void;
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

  // The box keeps its own copy for the debounce, but the shell can change the
  // search too -- a card's "See all videos from …" clears it -- so a value
  // from above that this box did not send replaces what it holds, a query
  // still being typed included.
  useEffect(() => {
    if (p.search === emitted.current) return;
    emitted.current = p.search;
    setText(p.search);
  }, [p.search]);

  const navRef = useRef<HTMLElement | null>(null);
  const searchRef = useRef<HTMLInputElement | null>(null);
  const toolsRef = useRef<HTMLDivElement | null>(null);
  const addRef = useRef<HTMLButtonElement | null>(null);
  const allBtnRef = useRef<HTMLButtonElement | null>(null);
  const channelBtnRef = useRef<HTMLButtonElement | null>(null);

  const onFeed = p.tab === "subscriptions";
  const actions = p.series ? p.seriesActions ?? null : null;

  // The starred filters that apply right now: the series-only three need
  // grouping, and ungrouped they would sit in the bar doing nothing.
  const quickIds = p.starred.filter((id) => !(isToggle(id) && toggleDef(id).seriesOnly && !p.grouped));
  // The bar's slots: starred views first, each by its own key, then the
  // starred filters. A narrow row folds them from the end, so the filters go
  // before any view does.
  const starredViews = p.savedViews.filter((v) => v.starred);
  const slots: string[] = [...starredViews.map((v) => `view:${v.id}`), ...quickIds];
  const fit = useToolbarFit(
    toolsRef, addRef, slots.length,
    `${p.tab}|${slots.join(",")}|${starredViews.map((v) => v.name).join("\u0000")}`,
  );
  const folded = onFeed && fit.folded;
  const foldedRef = useRef(folded);
  foldedRef.current = folded;
  const inRow = new Set(slots.slice(0, fit.shown));
  const channelsInRow = inRow.has("channel_ids");
  const lengthInRow = inRow.has("length");

  const toggles: Toggles = {
    hide_watched: { on: p.hideWatched, set: p.onHideWatched },
    downloaded_only: { on: p.downloadedOnly, set: p.onDownloadedOnly },
    show_hidden: { on: p.showHidden, set: p.onShowHidden },
    grouped: { on: p.grouped, set: p.onGrouped },
    in_progress: { on: p.inProgress, set: p.onInProgress },
    auto_downloading: { on: p.autoDownloading, set: p.onAutoDownloading },
  };
  const active = activeFilters({ ...p, channelIds: p.channelPick.ids });
  // The filters on screen as a saved view holds them, which a view is held
  // up against to say whether it is showing.
  const current: ViewFilters = {
    hide_watched: p.hideWatched, downloaded_only: p.downloadedOnly, show_hidden: p.showHidden,
    grouped: p.grouped, in_progress: p.inProgress, auto_downloading: p.autoDownloading,
    min_length: p.minLength, min_parts: p.minParts, sort: p.sort,
    channel_ids: p.channelPick.ids, channels_except: p.channelPick.except,
  };
  const channelsLabel = (f: ViewFilters) => {
    const { name, more } = pickLabel(p.channels, { ids: f.channel_ids, except: f.channels_except });
    return more > 0 ? `${name} +${more}` : name;
  };
  // What is on but out of sight: never starred, or folded out of a narrow row.
  // The badge exists so that a filter can never narrow the list unseen.
  const unseen = active.filter((id) => !inRow.has(id)).length;

  // The sidebar: open, and drawn a moment longer while it slides away. It
  // belongs to the feed, so leaving the tab closes it.
  const [sideOpen, setSideOpen] = useState(false);
  const sideDrawn = usePresence(sideOpen, SIDEBAR_OUT_MS);
  useEffect(() => {
    if (!onFeed) setSideOpen(false);
  }, [onFeed]);
  const closeSide = useCallback((refocus: boolean) => {
    setSideOpen(false);
    if (refocus) allBtnRef.current?.focus();
  }, []);

  // The bar's channel checklist. It hangs from the starred Channels button, so
  // it closes whenever that button goes -- folded away, unstarred, a series
  // opening (which switches it off) or a tab without the filters.
  const [channelsOpen, setChannelsOpen] = useState(false);
  useEffect(() => {
    if (!channelsInRow || p.series !== null || !onFeed) setChannelsOpen(false);
  }, [channelsInRow, p.series, onFeed]);
  const closeChannels = useCallback((refocus: boolean) => {
    setChannelsOpen(false);
    if (refocus) channelBtnRef.current?.focus();
  }, []);

  // The starred Length chip's popover, which goes the same way.
  const lengthBtnRef = useRef<HTMLButtonElement | null>(null);
  const [lengthOpen, setLengthOpen] = useState(false);
  useEffect(() => {
    if (!lengthInRow || p.series !== null || !onFeed) setLengthOpen(false);
  }, [lengthInRow, p.series, onFeed]);
  const closeLength = useCallback((refocus: boolean) => {
    setLengthOpen(false);
    if (refocus) lengthBtnRef.current?.focus();
  }, []);

  // A filter just starred lights up as it lands in the bar, so the eye finds
  // where it went.
  const [fresh, setFresh] = useState<string | null>(null);
  useEffect(() => {
    if (!fresh) return;
    const t = window.setTimeout(() => setFresh(null), 1200);
    return () => window.clearTimeout(t);
  }, [fresh]);
  const star = (id: QuickFilter) => {
    const next = toggleStar(p.starred, id);
    setFresh(next.includes(id) ? id : null);
    p.onStarred(next);
  };

  // Everything that narrows the list goes; grouping and the order only arrange
  // it, so they stay.
  const clear = () => {
    for (const [id, t] of Object.entries(toggles)) if (id !== "grouped" && t.on) t.set(false);
    if (p.minLength > 0) p.onMinLength(0);
    if (p.minParts > 0) p.onMinParts(0);
    if (p.channelPick.ids.length > 0) p.onChannelPick(EVERY_CHANNEL);
  };

  /** The folded row's search box is out: asked for, or holding a query. */
  const [searchOpen, setSearchOpen] = useState(false);
  const compactSearch = searchOpen || text !== "";
  // A shortcut can arrive on a tab that has neither the search box nor the
  // filters, so the handler — which is bound once — reads what it needs
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
      // are: neither lives behind the Subscriptions-only filters.
      if (key === "r") {
        if (!polling) onRefresh();
        return;
      }
      if (key === "n") {
        onAdd();
        return;
      }

      // Folded, the box is behind the magnifier: open it, and focus it once
      // it has rendered.
      if (tab === "subscriptions" && foldedRef.current) {
        if (live.current.series) return;
        setSearchOpen(true);
        setFocusWanted(true);
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

  // A series shows every part regardless of these, so rather than let them look
  // live and do nothing, they are switched off and say why. The sort still
  // orders the parts, so it alone stays live.
  const off = p.series !== null;
  const offHint = off ? "Not applied while a series is open" : undefined;

  const openSearch = () => {
    setSearchOpen(true);
    setFocusWanted(true);
  };

  // A view just starred lights up as it lands in the bar, as a filter does.
  const saveViews = (next: SavedView[]) => {
    const was = new Set(p.savedViews.filter((v) => v.starred).map((v) => v.id));
    const added = next.find((v) => v.starred && !was.has(v.id));
    if (added) setFresh(`view:${added.id}`);
    p.onSavedViews(next);
  };

  const viewChip = (v: SavedView) => {
    const showing = viewIsShowing(v, current, p.search);
    return (
      <button
        type="button"
        className={`chip quick-chip view-chip${showing ? " is-on" : ""}`}
        aria-pressed={showing}
        aria-label={v.name}
        disabled={off}
        title={offHint ?? `${v.name}: ${describeFilters(v.filters, channelsLabel(v.filters), v.search).join(" · ")}`}
        onClick={() => p.onApplyView(v)}
      >
        <span className="quick-glyph" aria-hidden="true"><IconView on={showing} /></span>
        <span className="view-chip-name">{v.name}</span>
      </button>
    );
  };

  const quickItem = (id: QuickFilter) => {
    if (id === "sort") {
      return (
        <select
          className="select sort-select"
          value={p.sort}
          aria-label="Sort order"
          title="Sort order"
          onChange={(e) => p.onSort(e.currentTarget.value as SortOrder)}
        >
          {/* Only groups have a part count to rank by; ungrouped this option
              would silently be "newest". */}
          {sortsFor(p.grouped).map((o) => (
            <option key={o.value} value={o.value}>{o.label}</option>
          ))}
        </select>
      );
    }
    if (id === "channel_ids") {
      return (
        <ChannelButton
          channels={p.channels}
          pick={p.channelPick}
          open={channelsOpen}
          onToggle={() => {
            setSideOpen(false);
            setLengthOpen(false);
            setChannelsOpen((o) => !o);
          }}
          disabled={off}
          offHint={offHint}
          buttonRef={channelBtnRef}
        />
      );
    }
    if (id === "length") {
      // It wears its value, like the sort and the channels beside it, so it
      // has no words to give up on a narrow row.
      const value = lengthLabel(p.minLength, p.minParts, p.grouped);
      return (
        <button
          ref={lengthBtnRef}
          type="button"
          className={`chip quick-chip length-chip${value ? " is-on" : ""}${lengthOpen ? " is-open" : ""}`}
          aria-haspopup="dialog"
          aria-expanded={lengthOpen}
          aria-label={`Length: ${value || "any"}`}
          disabled={off}
          title={offHint ?? "Length: how long a show is, and how many parts it has"}
          onClick={() => {
            setSideOpen(false);
            setChannelsOpen(false);
            setLengthOpen((o) => !o);
          }}
        >
          <span className="quick-glyph" aria-hidden="true"><IconLength /></span>
          <span className="length-chip-text">{value || "Any length"}</span>
          <IconCaret />
        </button>
      );
    }
    const def = toggleDef(id);
    const Icon = TOGGLE_ICON[id];
    const t = toggles[id];
    const name = def.chip ?? def.label;
    return (
      <button
        type="button"
        className={`chip quick-chip${t.on ? " is-on" : ""}`}
        aria-pressed={t.on}
        aria-label={name}
        disabled={off}
        // Folded to its glyph the chip has no words, so its tooltip names it.
        title={offHint ?? `${name}. ${def.hint}`}
        onClick={() => t.set(!t.on)}
      >
        <span className="quick-glyph" aria-hidden="true"><Icon /></span>
        <span className="fold-label"><span className="fold-text">{name}</span></span>
      </button>
    );
  };

  return (
    <header className="topnav" ref={navRef}>
      <div className="topnav-row">
        {/* The logo is the mark alone. Which MyTube is running is Settings ›
            About's to say; the row keeps its width for the controls. */}
        <span className="brand-mark" aria-hidden="true" />

        {/* The sections never change: not folded to icons, not replaced by a
            breadcrumb. A series opens a bar of its own below instead. */}
        <nav className="tabs" aria-label="Sections">
          {TABS.map((t) => (
            <button
              key={t.id}
              type="button"
              className={`tab${p.tab === t.id ? " is-active" : ""}`}
              aria-current={p.tab === t.id ? "page" : undefined}
              aria-label={t.label}
              title={t.id === "subscriptions" && p.series ? "Back to all subscriptions" : t.label}
              onClick={() => {
                // A tab opens at its top. From inside a series that makes
                // Subscriptions the way back to the feed, which is why the
                // series bar has no breadcrumb of its own.
                if (p.series) p.onExitSeries();
                p.onTab(t.id);
              }}
            >
              <span className="tab-glyph" aria-hidden="true"><t.icon /></span>
              <span className="tab-label">{t.label}</span>
            </button>
          ))}
          {/* A newer release, beside the tab that leads to it (Settings is
              the last one). It opens About rather than the release page:
              that is where installed and available stand side by side, and
              how to update depends on how MyTube was installed. Hover and
              focus spell it out over the row rather than in it -- see
              .update-chip in App.css for why the row must not move. */}
          {p.update && (
            <button
              type="button"
              className="update-chip"
              onClick={p.onOpenAbout}
              aria-label={`Update available: MyTube ${p.update.version}. Open About`}
              title={`MyTube ${p.update.version} is available. Open Settings › About`}
            >
              <span className="update-chip-face">
                <IconNewRelease />
                <span className="update-chip-word">Update available!</span>
              </span>
            </button>
          )}
        </nav>

        {p.tab === "downloads" ? (
          /* The Downloads tab's search, and nothing else: its list needs no
             filters, only a way to find one file among hundreds. */
          <>
            <div className="topnav-tools">
              <div className="search search-downloads">
                <span className="search-glyph" aria-hidden="true">⌕</span>
                <input
                  ref={searchRef}
                  className="search-input"
                  type="search"
                  value={p.downloadSearch}
                  placeholder="Search downloads"
                  aria-label="Search downloads"
                  title="Titles, channels and series names (Ctrl+F)"
                  onChange={(e) => p.onDownloadSearch(e.currentTarget.value)}
                />
              </div>
            </div>
            <span className="topnav-rule" aria-hidden="true" />
          </>
        ) : onFeed ? (
          <>
            {/* Takes whatever the row leaves and packs to the right: toolbarFit.ts
                measures it to decide how much of it can be spelled out. */}
            <div className="topnav-tools" ref={toolsRef}>
              {!folded ? (
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
              ) : compactSearch ? (
                <div className="search search-compact">
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
                    // Holding a query, the box stays open so the search that
                    // is narrowing the grid is never out of sight.
                    onBlur={() => setSearchOpen(false)}
                    onKeyDown={(e) => {
                      if (e.key !== "Escape") return;
                      // Leaving the box, not the series behind it.
                      e.stopPropagation();
                      e.currentTarget.blur();
                    }}
                  />
                </div>
              ) : (
                <button
                  type="button"
                  className="icon-btn search-toggle"
                  disabled={off}
                  title={offHint ?? "Search (Ctrl+F)"}
                  onClick={openSearch}
                >
                  <IconSearch />
                  <span className="sr-only">Open search</span>
                </button>
              )}

              {/* The starred views and filters. One folded out of a narrow row
                  keeps its slot at zero width -- still measurable, no longer
                  usable. */}
              {slots.length > 0 && (
                <div className={`quick${fit.words ? "" : " is-glyphs"}`} role="group" aria-label="Starred filters">
                  {starredViews.map((v) => {
                    const key = `view:${v.id}`;
                    const gone = !inRow.has(key);
                    return (
                      <span
                        key={key}
                        className={`quick-slot${gone ? " is-folded" : ""}${fresh === key ? " is-new" : ""}`}
                        inert={gone}
                        aria-hidden={gone || undefined}
                      >
                        {viewChip(v)}
                      </span>
                    );
                  })}
                  {quickIds.map((id) => {
                    const gone = !inRow.has(id);
                    return (
                      <span
                        key={id}
                        className={`quick-slot${gone ? " is-folded" : ""}${fresh === id ? " is-new" : ""}`}
                        inert={gone}
                        aria-hidden={gone || undefined}
                      >
                        {quickItem(id)}
                      </span>
                    );
                  })}
                </div>
              )}

              <button
                ref={allBtnRef}
                type="button"
                className={`chip all-filters${sideOpen ? " is-open" : ""}${unseen > 0 ? " is-on" : ""}${fit.tight ? " is-tight" : ""}`}
                aria-haspopup="dialog"
                aria-expanded={sideOpen}
                aria-label={unseen > 0 ? `All filters, ${unseen} more on` : "All filters"}
                title={unseen > 0
                  ? `${unseen} ${unseen === 1 ? "filter is" : "filters are"} on that the top bar is not showing`
                  : "Every filter, and a star to keep any of them in the top bar"}
                onClick={() => {
                  setChannelsOpen(false);
                  setLengthOpen(false);
                  setSideOpen((o) => !o);
                }}
              >
                <IconFilters />
                <span className="fold-label"><span className="fold-text">All filters</span></span>
                {unseen > 0 && <span className="count" aria-hidden="true">{unseen}</span>}
              </button>
            </div>
            <span className="topnav-rule" aria-hidden="true" />
          </>
        ) : (
          <div className="topnav-spacer" />
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
          ref={addRef}
          type="button"
          className={`btn btn-primary btn-add${onFeed && fit.tight ? " is-tight" : ""}`}
          title="Add a channel or video (Ctrl+N)"
          onClick={p.onAdd}
        >
          <span aria-hidden="true">+</span>
          <span className="fold-label"><span className="fold-text">Add</span></span>
        </button>
      </div>

      {/* The open series, in a bar of its own under the row. The row above
          still says Subscriptions, so this names the series and nothing
          above it. */}
      {p.series && onFeed && (
        <section className="series-bar" aria-label="Open series">
          <span className="series-bar-glyph" aria-hidden="true"><IconSeries /></span>
          <span className="series-bar-name" title={p.series.name}>{p.series.name}</span>
          {/* The count lands a moment after the bar does, so it is spoken
              when it arrives rather than sitting there unannounced. */}
          {metaText(p.series) && (
            <span className="series-bar-meta" role="status">{metaText(p.series)}</span>
          )}
          <span className="series-bar-fill" />
          {actions && <SeriesActionButtons a={actions} />}
          <button
            type="button"
            className="series-bar-x"
            onClick={p.onExitSeries}
            aria-label="Leave this series"
            title="Leave this series (Esc)"
          >
            <IconClose />
          </button>
        </section>
      )}

      {onFeed && channelsInRow && channelsOpen && (
        <ChannelPopover
          anchor={channelBtnRef}
          onClose={closeChannels}
          channels={p.channels}
          pick={p.channelPick}
          onPick={p.onChannelPick}
        />
      )}

      {onFeed && lengthInRow && lengthOpen && (
        <LengthPopover
          anchor={lengthBtnRef}
          onClose={closeLength}
          minLength={p.minLength}
          onMinLength={p.onMinLength}
          minParts={p.minParts}
          onMinParts={p.onMinParts}
          grouped={p.grouped}
        />
      )}

      {onFeed && sideDrawn && (
        <FiltersSidebar
          closing={!sideOpen}
          onClose={closeSide}
          nav={navRef}
          off={off}
          toggles={toggles}
          grouped={p.grouped}
          sort={p.sort}
          onSort={p.onSort}
          minLength={p.minLength}
          onMinLength={p.onMinLength}
          minParts={p.minParts}
          onMinParts={p.onMinParts}
          channels={p.channels}
          pick={p.channelPick}
          onPick={p.onChannelPick}
          starred={p.starred}
          onStar={star}
          views={p.savedViews}
          onViews={saveViews}
          onApplyView={p.onApplyView}
          current={current}
          search={p.search}
          channelsLabel={channelsLabel}
          canClear={active.length > 0}
          onClear={clear}
        />
      )}
    </header>
  );
}
