import { useCallback, useEffect, useRef, useState, type ReactElement } from "react";
import {
  IconBusy, IconCaret, IconClose, IconContinue, IconDownload, IconFilters, IconGroupsOnly,
  IconHidden, IconNewRelease, IconSearch, IconSeries, IconSettings, IconShow, IconSubscriptions,
  IconUnwatched,
} from "./Icons";
import FiltersPanel, { activeFilterCount, SORTS } from "./FiltersPanel";
import { ChannelButton, ChannelPopover } from "./ChannelPicker";
import type { CallState, SeriesActions } from "./SubscriptionsView";
import { useToolbarFit } from "../toolbarFit";
import { singleClick } from "../pending";
import type { AppUpdate, Channel, SortOrder } from "../types";

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

/**
 * One of the crumb's series actions: a glyph and its words, the words folding
 * away on a narrow window (App.css) while the glyph, the name and the tooltip
 * stay. Disabled the moment its call goes out; the glyph turns only once the
 * call is slow enough to notice.
 */
function CrumbAction({ glyph, label, title, state, danger, onClick }: {
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
      className={["crumb-act", danger && "is-danger", spinning && "is-spinning"].filter(Boolean).join(" ")}
      aria-label={label}
      title={title}
      aria-busy={state !== "idle" || undefined}
      disabled={state !== "idle"}
      // Each press changes what is under the pointer -- Hide turns into
      // Un-hide as soon as the call is back -- so a double-click's second half
      // would undo the first.
      onClick={singleClick(onClick)}
    >
      <span className="crumb-act-glyph" aria-hidden="true">{spinning ? <IconBusy /> : glyph}</span>
      <span className="crumb-act-label">{label}</span>
    </button>
  );
}

/**
 * What the series card's menu offers, on the crumb of the series it stands
 * for. Hide (or Un-hide) leads because it is always there; Download comes and
 * goes with what is left to fetch, and last in the row its going moves
 * nothing. Ahead of Hide it would slide Hide under the pointer that had just
 * pressed Download.
 */
function SeriesActionButtons({ a }: { a: SeriesActions }) {
  const parts = `${a.due} ${a.due === 1 ? "part" : "parts"}`;
  return (
    <div className="crumb-actions" role="group" aria-label="Series actions">
      {a.marked ? (
        <CrumbAction
          glyph={<IconShow />}
          label="Un-hide series"
          title="Bring every part back, and let parts that join the series later into the feed"
          state={a.hiding}
          onClick={a.onUnhide}
        />
      ) : (
        <CrumbAction
          glyph={<IconHidden />}
          label="Hide series"
          title="Hide every part, and any part that joins the series later"
          state={a.hiding}
          danger
          onClick={a.onHide}
        />
      )}
      {a.due > 0 && (
        <CrumbAction
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

interface Props {
  tab: Tab;
  onTab: (t: Tab) => void;
  channels: Channel[];
  /** The channels the feed is narrowed to, sorted; empty for every channel. */
  channelIds: string[];
  onChannelIds: (ids: string[]) => void;
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
  groupsOnly: boolean;
  onGroupsOnly: (v: boolean) => void;
  inProgress: boolean;
  onInProgress: (v: boolean) => void;
  /** The open series, or null for the ordinary feed. Replaces the tab strip. */
  series: SeriesCrumb | null;
  /** What the crumb can do with the open series, once its parts are in. Absent
   *  or null, it offers nothing -- as for a series of one. */
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

  const searchRef = useRef<HTMLInputElement | null>(null);
  const compactSearchRef = useRef<HTMLInputElement | null>(null);
  const rowRef = useRef<HTMLDivElement | null>(null);
  const fullRef = useRef<HTMLDivElement | null>(null);
  const compactRef = useRef<HTMLDivElement | null>(null);
  const filtersBtnRef = useRef<HTMLButtonElement | null>(null);
  const channelBtnRef = useRef<HTMLButtonElement | null>(null);

  const showFilters = p.tab === "subscriptions";
  const actions = p.series ? p.seriesActions ?? null : null;
  const fit = useToolbarFit(
    { row: rowRef, full: fullRef, compact: compactRef },
    showFilters,
    // The actions are a row child of their own, and they arrive after the
    // crumb does, once the parts are in: a new child to watch.
    p.series ? (actions ? "crumb+actions" : "crumb") : "tabs",
  );
  const folded = fit === "folded";
  const foldedRef = useRef(folded);
  foldedRef.current = folded;

  const [panelOpen, setPanelOpen] = useState(false);
  // The panel belongs to the folded row; unfolding puts its controls back in
  // the row, so it closes rather than waiting to reappear on the next fold.
  useEffect(() => {
    if (!folded) setPanelOpen(false);
  }, [folded]);
  const closePanel = useCallback((refocus: boolean) => {
    setPanelOpen(false);
    if (refocus) filtersBtnRef.current?.focus();
  }, []);

  // The full row's channel checklist. It belongs to that row the way the
  // panel belongs to the folded one, so folding closes it -- as does a series
  // opening or a tab without the filters, either of which takes its button
  // away or switches it off.
  const [channelsOpen, setChannelsOpen] = useState(false);
  useEffect(() => {
    if (folded || p.series !== null || !showFilters) setChannelsOpen(false);
  }, [folded, p.series, showFilters]);
  const closeChannels = useCallback((refocus: boolean) => {
    setChannelsOpen(false);
    if (refocus) channelBtnRef.current?.focus();
  }, []);

  /** The folded row's search box is out: asked for, or holding a query. */
  const [searchOpen, setSearchOpen] = useState(false);
  const compactSearch = searchOpen || text !== "";
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
    const el = (foldedRef.current ? compactSearchRef : searchRef).current;
    if (!el || el.disabled) return;
    el.focus();
    el.select();
  }, [focusWanted, p.tab]);

  // A series shows every part regardless of these, so rather than let them look
  // live and do nothing, they are switched off and say why.
  const off = p.series !== null;
  const offHint = off ? "Not applied while a series is open" : undefined;

  const openSearch = () => {
    setSearchOpen(true);
    setFocusWanted(true);
  };
  const filterCount = activeFilterCount(p);

  return (
    <header className={`topnav fit-${fit}`}>
      <div className="topnav-row" ref={rowRef}>
        {/* The logo is the mark alone. Which MyTube is running is Settings ›
            About's to say; the row keeps its width for the controls. */}
        <span className="brand-mark" aria-hidden="true" />

        {p.series ? (
          /* The series replaces the tab strip rather than adding a bar of its
             own: it is where you are, not a notice about where you are. */
          <>
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
            {/* Beside the crumb rather than in it: the crumb is navigation, and
                these act on the series. As a child of the row they are counted
                by toolbarFit.ts like everything else that is not a cluster. */}
            {actions && <SeriesActionButtons a={actions} />}
          </>
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
        )}

        <div className="topnav-spacer" />

        {showFilters && (
          <>
            {/* The full controls. Folded, they stay in the layout at zero width
                -- that is what lets the fold animate, and what lets the row
                measure what they would need to come back -- but inert. */}
            <div className="tools tools-full" ref={fullRef} inert={folded} aria-hidden={folded || undefined}>
              <div className="tools-inner">
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
                  {/* Filters over cards that only exist grouped, so they are offered
                      only then -- ungrouped, every card is a lone video. */}
                  {p.grouped && (
                    <>
                    <button
                      type="button"
                      className={`seg-btn${p.groupsOnly ? " is-on" : ""}`}
                      aria-pressed={p.groupsOnly}
                      aria-label="Only groups"
                      disabled={off}
                      title={offHint ?? "Show only multi-part series, not lone videos"}
                      onClick={() => p.onGroupsOnly(!p.groupsOnly)}
                    >
                      <span className="seg-glyph" aria-hidden="true"><IconGroupsOnly /></span>
                      <span className="seg-label">Only groups</span>
                    </button>
                    <button
                      type="button"
                      className={`seg-btn${p.inProgress ? " is-on" : ""}`}
                      aria-pressed={p.inProgress}
                      aria-label="Continue watching"
                      disabled={off}
                      title={offHint ?? "Show only series you have started and not finished"}
                      onClick={() => p.onInProgress(!p.inProgress)}
                    >
                      <span className="seg-glyph" aria-hidden="true"><IconContinue /></span>
                      <span className="seg-label">Continue watching</span>
                    </button>
                    </>
                  )}
                </div>

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
                  {/* Only groups have a part count to rank by; ungrouped this
                      option would silently be "newest". */}
                  {SORTS.filter((o) => o.value !== "parts" || p.grouped).map((o) => (
                    <option key={o.value} value={o.value}>{o.label}</option>
                  ))}
                </select>
                <ChannelButton
                  channels={p.channels}
                  picked={p.channelIds}
                  open={channelsOpen}
                  onToggle={() => setChannelsOpen((o) => !o)}
                  disabled={off}
                  offHint={offHint}
                  buttonRef={channelBtnRef}
                />
              </div>
            </div>

            {/* ...and the same, folded: search behind a magnifier, everything
                else behind one button that says how many filters are on. */}
            <div className="tools tools-compact" ref={compactRef} inert={!folded} aria-hidden={!folded || undefined}>
              <div className="tools-inner">
                {folded && compactSearch ? (
                  <div className="search search-compact">
                    <span className="search-glyph" aria-hidden="true">⌕</span>
                    <input
                      ref={compactSearchRef}
                      className="search-input"
                      type="search"
                      value={text}
                      placeholder="Search titles and channels"
                      aria-label="Search videos"
                      disabled={off}
                      title={offHint ?? "Ctrl+F to focus"}
                      onChange={(e) => setText(e.currentTarget.value)}
                      // Holding a query, the box stays open so the search
                      // that is narrowing the grid is never out of sight.
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
                <button
                  type="button"
                  ref={filtersBtnRef}
                  className={`chip filters-btn${panelOpen ? " is-open" : ""}`}
                  aria-haspopup="dialog"
                  aria-expanded={panelOpen}
                  aria-label={filterCount > 0 ? `Filters, ${filterCount} applied` : "Filters"}
                  onClick={() => setPanelOpen((o) => !o)}
                >
                  <IconFilters />
                  <span>Filters</span>
                  {filterCount > 0 && <span className="count">{filterCount}</span>}
                  <IconCaret />
                </button>
              </div>
            </div>
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

      {showFilters && !folded && channelsOpen && (
        <ChannelPopover
          anchor={channelBtnRef}
          onClose={closeChannels}
          channels={p.channels}
          picked={p.channelIds}
          onPick={p.onChannelIds}
        />
      )}

      {showFilters && folded && panelOpen && (
        <FiltersPanel
          anchor={filtersBtnRef}
          onClose={closePanel}
          off={off}
          offHint={offHint}
          channels={p.channels}
          channelIds={p.channelIds}
          onChannelIds={p.onChannelIds}
          hideWatched={p.hideWatched}
          onHideWatched={p.onHideWatched}
          downloadedOnly={p.downloadedOnly}
          onDownloadedOnly={p.onDownloadedOnly}
          showHidden={p.showHidden}
          onShowHidden={p.onShowHidden}
          grouped={p.grouped}
          onGrouped={p.onGrouped}
          groupsOnly={p.groupsOnly}
          onGroupsOnly={p.onGroupsOnly}
          inProgress={p.inProgress}
          onInProgress={p.onInProgress}
          sort={p.sort}
          onSort={p.onSort}
        />
      )}
    </header>
  );
}
