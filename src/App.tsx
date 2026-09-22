import { useCallback, useEffect, useRef, useState } from "react";
import { CARD_DEFAULT, clampCardSize } from "./format";
import TopNav, { type Tab } from "./components/TopNav";
import SubscriptionsView, { type SeriesTally } from "./components/SubscriptionsView";
import DownloadsView from "./components/DownloadsView";
import SettingsView from "./components/SettingsView";
import AddChannelDialog from "./components/AddChannelDialog";
import { ToastProvider, useToast } from "./components/Toast";
import { api, errText } from "./api";
import { usePollEvents } from "./events";
import type { Channel, SortOrder, Video, ViewState } from "./types";
import "./App.css";

export default function App() {
  return (
    <ToastProvider>
      <Shell />
    </ToastProvider>
  );
}

// The one place a `ViewState` object literal is built, so hydration's seed
// and the writer's comparison always produce identically-ordered JSON
// regardless of the field order `config.rs`'s `ViewState` happens to
// serialise in -- `JSON.stringify` order follows a JS object's own key
// insertion order, and that order now comes from this literal, not from
// whatever order the IPC payload's keys arrived in.
function buildViewState(
  channelId: string | null, search: string, hideWatched: boolean,
  downloadedOnly: boolean, showHidden: boolean, grouped: boolean, sort: SortOrder,
): ViewState {
  return {
    channel_id: channelId, search,
    hide_watched: hideWatched, downloaded_only: downloadedOnly, show_hidden: showHidden,
    grouped, sort,
  };
}

function Shell() {
  const toast = useToast();

  const [tab, setTab] = useState<Tab>("subscriptions");
  const [channels, setChannels] = useState<Channel[]>([]);
  const [channelId, setChannelId] = useState<string | null>(null);
  const [search, setSearch] = useState("");
  const [hideWatched, setHideWatched] = useState(false);
  const [downloadedOnly, setDownloadedOnly] = useState(false);
  const [showHidden, setShowHidden] = useState(false);
  // Collapse each channel's multi-part uploads behind one card.
  const [grouped, setGrouped] = useState(false);
  const [cardSize, setCardSize] = useState(CARD_DEFAULT);
  const [sort, setSort] = useState<SortOrder>("newest");
  // The video whose series the grid is pinned to, or null for the normal feed.
  const [siblingOf, setSiblingOf] = useState<Video | null>(null);
  // The open series' name, known at the moment it is opened: a group card
  // carries the shared stem, a right-click on a lone card has only its title.
  const [seriesName, setSeriesName] = useState("");
  // What the series turned out to hold. Only the view can count it, so it is
  // reported back up once each page settles.
  const [tally, setTally] = useState<SeriesTally | null>(null);
  const [polling, setPolling] = useState(false);
  const [addOpen, setAddOpen] = useState(false);
  // Every bump forces the mounted views to refetch.
  const [reloadToken, setReloadToken] = useState(0);

  const reload = useCallback(() => setReloadToken((n) => n + 1), []);

  // "Most parts first" only exists while the cards are groups, so ungrouping
  // has to take the sort back with it -- otherwise the picker is left showing
  // a value it no longer offers, and the feed silently sorts by date anyway.
  const setGroupedAndSort = useCallback((on: boolean) => {
    setGrouped(on);
    if (!on) setSort((s) => (s === "parts" ? "newest" : s));
  }, []);

  // Opening a series and leaving it both touch three pieces of state, so they
  // are one call each rather than three the callers have to remember.
  const openSeries = useCallback((video: Video, stem?: string | null) => {
    setSiblingOf(video);
    setSeriesName(stem?.trim() || video.title);
    setTally(null);
  }, []);

  const closeSeries = useCallback(() => {
    setSiblingOf(null);
    setTally(null);
  }, []);

  // Escape leaves a series. It is bound last on purpose: a dialog or the card
  // menu closes on Escape too, and whichever is on top should get it first --
  // both of those sit above the grid, so they are checked for in the DOM.
  useEffect(() => {
    if (!siblingOf) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      if (document.querySelector(".modal-backdrop, .context-menu")) return;
      closeSeries();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [siblingOf, closeSeries]);

  // Set once the channel list has actually been fetched -- not once it is
  // non-empty, since a user with no subscriptions at all must still have a
  // stale channel filter pruned below. A ref, not state: it only needs to be
  // readable by the time `channels` itself changes, which already re-renders.
  // On a failed fetch it is left false, so a restored channel filter stays
  // unpruned for the rest of the session rather than being cleared on a
  // transient error; the next successful load heals it.
  const channelsFetched = useRef(false);

  const loadChannels = useCallback(async () => {
    try {
      const list = await api.listChannels();
      channelsFetched.current = true;
      setChannels(list);
    } catch (err) {
      toast.error(errText(err));
    }
  }, [toast]);

  useEffect(() => { void loadChannels(); }, [loadChannels]);

  // A poll can also start on its own (startup and the interval timer), so the
  // spinner and the refetch are driven by events rather than by the button.
  usePollEvents({
    onStarted: () => setPolling(true),
    onFinished: (summary) => {
      setPolling(false);
      void loadChannels();
      reload();
      if (summary.newVideos > 0) {
        toast.success(
          `${summary.newVideos} new video${summary.newVideos === 1 ? "" : "s"}` +
          ` from ${summary.channelsPolled} channel${summary.channelsPolled === 1 ? "" : "s"}.`,
        );
      }
      if (summary.errors.length > 0) {
        toast.error(`Poll problems: ${summary.errors.slice(0, 3).join("; ")}`);
      }
    },
  });

  const refresh = useCallback(async () => {
    if (polling) return;
    setPolling(true);
    try {
      const summary = await api.pollAll();
      // Belt and braces: if the backend emitted no event, still settle the UI.
      setPolling(false);
      void loadChannels();
      reload();
      if (summary.newVideos === 0 && summary.errors.length === 0) {
        toast.info("No new videos.");
      }
    } catch (err) {
      setPolling(false);
      toast.error(errText(err));
    }
  }, [polling, loadChannels, reload, toast]);

  // A channel filter pointing at a channel that no longer exists shows nothing.
  // Gated on the list having actually been fetched: hydration can restore
  // `channelId` from settings.json before `listChannels` resolves, and until
  // then `channels` is its unfetched `[]` default -- indistinguishable from
  // "you have no channels" -- so pruning against it would silently discard a
  // channel filter that the list, once it arrives, would have kept.
  useEffect(() => {
    if (!channelsFetched.current) return;
    if (channelId && !channels.some((c) => c.id === channelId)) setChannelId(null);
  }, [channels, channelId]);

  // Card size and the feed's filters both live in settings.json, so both are
  // read up front and rendering waits for them -- see the early return below.
  const [hydrated, setHydrated] = useState(false);
  // The view last known to be on disk, so hydration flipping this effect's own
  // dependency does not read as a change and write the defaults back over it.
  const persistedView = useRef("");

  useEffect(() => {
    api.getSettings()
      .then((s) => {
        setCardSize(clampCardSize(s.card_size ?? CARD_DEFAULT));
        const v = s.view;
        setChannelId(v.channel_id);
        setSearch(v.search);
        setHideWatched(v.hide_watched);
        setDownloadedOnly(v.downloaded_only);
        setShowHidden(v.show_hidden);
        setGrouped(v.grouped);
        setSort(v.sort);
        persistedView.current = JSON.stringify(
          buildViewState(v.channel_id, v.search, v.hide_watched, v.downloaded_only, v.show_hidden, v.grouped, v.sort),
        );
      })
      .catch(() => {
        // get_settings failed -- or threw on a malformed `s.view` dereferenced
        // just above -- so none of the setters above ran and every filter is
        // still at its useState default. Seed the guard from that same
        // default state, not from "", so the writer effect below sees
        // nothing has changed and does not write the defaults over a view it
        // never actually managed to read.
        persistedView.current = JSON.stringify(
          buildViewState(channelId, search, hideWatched, downloadedOnly, showHidden, grouped, sort),
        );
      })
      .finally(() => setHydrated(true));
  }, []);

  // The feed's scroll container, handed to SubscriptionsView so leaving a series
  // -- or riding out a poll's refetch -- can put the grid back where it was.
  const contentRef = useRef<HTMLElement | null>(null);

  const sizeTimer = useRef<number | undefined>(undefined);
  const saveCardSize = useCallback((px: number) => {
    setCardSize(px);
    window.clearTimeout(sizeTimer.current);
    // Zooming fires many wheel events; only the resting value is worth writing.
    sizeTimer.current = window.setTimeout(() => {
      api.getSettings()
        .then((s) => api.saveSettings({ ...s, card_size: px }))
        .catch(() => {});
    }, 400);
  }, []);

  // Debounced like saveCardSize, but there is no explicit call site for a
  // filter change -- every setter above is reachable straight from TopNav --
  // so this watches the seven values instead of wrapping each setter.
  useEffect(() => {
    if (!hydrated) return;
    const view = buildViewState(channelId, search, hideWatched, downloadedOnly, showHidden, grouped, sort);
    const serialised = JSON.stringify(view);
    if (serialised === persistedView.current) return;
    const t = window.setTimeout(() => {
      api.saveViewState(view)
        .then(() => { persistedView.current = serialised; })
        .catch(() => {});
    }, 400);
    return () => window.clearTimeout(t);
  }, [hydrated, channelId, search, hideWatched, downloadedOnly, showHidden, grouped, sort]);

  // SubscriptionsView and TopNav both seed themselves from props at mount, so
  // rendering before the saved view lands would spend a query on the
  // unfiltered feed and leave the search box briefly out of sync with it.
  if (!hydrated) return <div className="app" />;

  return (
    <div className="app">
      <TopNav
        tab={tab}
        onTab={setTab}
        channels={channels}
        channelId={channelId}
        onChannelId={setChannelId}
        search={search}
        onSearch={setSearch}
        hideWatched={hideWatched}
        onHideWatched={setHideWatched}
        downloadedOnly={downloadedOnly}
        onDownloadedOnly={setDownloadedOnly}
        showHidden={showHidden}
        onShowHidden={setShowHidden}
        grouped={grouped}
        onGrouped={setGroupedAndSort}
        sort={sort}
        onSort={setSort}
        series={siblingOf ? { name: seriesName, parts: 0, runtime: "", unknown: 0, ...tally } : null}
        onExitSeries={closeSeries}
        polling={polling}
        onRefresh={() => void refresh()}
        onAdd={() => setAddOpen(true)}
      />

      <main className="content" ref={contentRef}>
        {tab === "subscriptions" && (
          <SubscriptionsView
            channelId={channelId}
            search={search}
            hideWatched={hideWatched}
            downloadedOnly={downloadedOnly}
            showHidden={showHidden}
            grouped={grouped}
            cardSize={cardSize}
            onCardSize={saveCardSize}
            scrollRef={contentRef}
            sort={sort}
            siblingOf={siblingOf}
            onFindSiblings={openSeries}
            onSeriesTally={setTally}
            reloadToken={reloadToken}
            channelCount={channels.length}
            onAdd={() => setAddOpen(true)}
          />
        )}
        {tab === "downloads" && (
          <DownloadsView reloadToken={reloadToken} cardSize={cardSize} scrollRef={contentRef} />
        )}
        {tab === "settings" && <SettingsView />}
      </main>

      <AddChannelDialog
        open={addOpen}
        onClose={() => setAddOpen(false)}
        channels={channels}
        onChanged={() => {
          void loadChannels();
          reload();
        }}
      />
    </div>
  );
}
