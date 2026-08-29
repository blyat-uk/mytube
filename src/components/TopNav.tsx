import { useEffect, useRef, useState } from "react";
import type { Channel, SortOrder } from "../types";

export type Tab = "subscriptions" | "downloads" | "settings";

const TABS: { id: Tab; label: string }[] = [
  { id: "subscriptions", label: "Subscriptions" },
  { id: "downloads", label: "Downloads" },
  { id: "settings", label: "Settings" },
];

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

  const showFilters = p.tab === "subscriptions";

  return (
    <header className="topnav">
      <div className="topnav-row">
        <div className="brand">
          <span className="brand-mark" aria-hidden="true" />
          <span className="brand-name">MyTube</span>
        </div>

        <nav className="tabs" aria-label="Sections">
          {TABS.map((t) => (
            <button
              key={t.id}
              type="button"
              className={`tab${p.tab === t.id ? " is-active" : ""}`}
              aria-current={p.tab === t.id ? "page" : undefined}
              onClick={() => p.onTab(t.id)}
            >
              {t.label}
            </button>
          ))}
        </nav>

        <div className="topnav-spacer" />

        <div className="search">
          <span className="search-glyph" aria-hidden="true">⌕</span>
          <input
            className="search-input"
            type="search"
            value={text}
            placeholder="Search titles and channels"
            aria-label="Search videos"
            onChange={(e) => setText(e.currentTarget.value)}
          />
        </div>

        <label className="channel-picker">
          <span className="channel-picker-label">Channel</span>
          <select
            className="select"
            value={p.channelId ?? ""}
            aria-label="Filter by channel"
            onChange={(e) => p.onChannelId(e.currentTarget.value || null)}
          >
            <option value="">All</option>
            {p.channels.map((c) => (
              <option key={c.id} value={c.id}>{c.title}</option>
            ))}
          </select>
        </label>

        <button
          type="button"
          className={`icon-btn${p.polling ? " is-spinning" : ""}`}
          onClick={p.onRefresh}
          disabled={p.polling}
          title={p.polling ? "Checking for new videos…" : "Check subscriptions for new videos"}
        >
          <span aria-hidden="true">↻</span>
          <span className="sr-only">Refresh subscriptions</span>
        </button>

        <button type="button" className="btn btn-primary" onClick={p.onAdd}>
          Add
        </button>
      </div>

      {showFilters && (
        <div className="filterbar">
          <button
            type="button"
            className={`chip${p.hideWatched ? " is-on" : ""}`}
            aria-pressed={p.hideWatched}
            onClick={() => p.onHideWatched(!p.hideWatched)}
          >
            Hide watched
          </button>
          <button
            type="button"
            className={`chip${p.downloadedOnly ? " is-on" : ""}`}
            aria-pressed={p.downloadedOnly}
            onClick={() => p.onDownloadedOnly(!p.downloadedOnly)}
          >
            Downloaded only
          </button>

          <div className="filterbar-spacer" />

          <label className="sort-picker">
            <span className="sort-picker-label">Sort</span>
            <select
              className="select"
              value={p.sort}
              aria-label="Sort order"
              onChange={(e) => p.onSort(e.currentTarget.value as SortOrder)}
            >
              <option value="newest">Newest first</option>
              <option value="oldest">Oldest first</option>
              <option value="channel">By channel</option>
            </select>
          </label>
        </div>
      )}
    </header>
  );
}
