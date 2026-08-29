import { useCallback, useEffect, useRef, useState } from "react";
import { CARD_DEFAULT, clampCardSize } from "./format";
import TopNav, { type Tab } from "./components/TopNav";
import SubscriptionsView from "./components/SubscriptionsView";
import DownloadsView from "./components/DownloadsView";
import SettingsView from "./components/SettingsView";
import AddChannelDialog from "./components/AddChannelDialog";
import { ToastProvider, useToast } from "./components/Toast";
import { api, errText } from "./api";
import { usePollEvents } from "./events";
import type { Channel, SortOrder } from "./types";
import "./App.css";

export default function App() {
  return (
    <ToastProvider>
      <Shell />
    </ToastProvider>
  );
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
  const [cardSize, setCardSize] = useState(CARD_DEFAULT);
  const [sort, setSort] = useState<SortOrder>("newest");
  const [polling, setPolling] = useState(false);
  const [addOpen, setAddOpen] = useState(false);
  // Every bump forces the mounted views to refetch.
  const [reloadToken, setReloadToken] = useState(0);

  const reload = useCallback(() => setReloadToken((n) => n + 1), []);

  const loadChannels = useCallback(async () => {
    try {
      setChannels(await api.listChannels());
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
  useEffect(() => {
    if (channelId && !channels.some((c) => c.id === channelId)) setChannelId(null);
  }, [channels, channelId]);

  // Card size lives in settings.json so the zoom level survives a restart.
  useEffect(() => {
    api.getSettings()
      .then((s) => setCardSize(clampCardSize(s.card_size ?? CARD_DEFAULT)))
      .catch(() => {});
  }, []);

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
        sort={sort}
        onSort={setSort}
        polling={polling}
        onRefresh={() => void refresh()}
        onAdd={() => setAddOpen(true)}
      />

      <main className="content">
        {tab === "subscriptions" && (
          <SubscriptionsView
            channelId={channelId}
            search={search}
            hideWatched={hideWatched}
            downloadedOnly={downloadedOnly}
            showHidden={showHidden}
            cardSize={cardSize}
            onCardSize={saveCardSize}
            sort={sort}
            reloadToken={reloadToken}
            channelCount={channels.length}
            onAdd={() => setAddOpen(true)}
          />
        )}
        {tab === "downloads" && <DownloadsView reloadToken={reloadToken} />}
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
