import { useCallback, useLayoutEffect, useMemo, useRef, useState } from "react";
import { useAppVersion } from "../appVersion";
import { SettingsContext, type SectionId, type SettingsCtx } from "./settings/context";
import { useSettingsDoc } from "./settings/doc";
import {
  useBrowserScan, useCookieFile, usePlayers, useSyncLive, useToolsLive, useTransferEstimate,
} from "./settings/live";
import SettingsRail from "./settings/SettingsRail";
import { SECTIONS } from "./settings/sections";

/* The section left open, kept for the session: leaving for Subscriptions and
   coming back lands where you were. Deliberately not persisted. */
let lastSection: SectionId = "downloads";

/**
 * Settings: a rail of sections on the left, one section on the right.
 *
 * The shell owns the settings document and every piece of live machine state
 * (tools, sync, browsers, players, version, library size), because the rail
 * summarises all of it at once; the pages read the same copies through the
 * context, so the rail and the open page can never disagree.
 */
export default function SettingsView({ initialSection }: { initialSection?: SectionId } = {}) {
  const doc = useSettingsDoc();
  const tools = useToolsLive();
  const sync = useSyncLive();
  const browsers = useBrowserScan();
  const cookieFile = useCookieFile(doc.settings?.cookies_file ?? "");
  const players = usePlayers();
  const { estimate, refresh: refreshEstimate } = useTransferEstimate();
  const [version, setVersion] = useAppVersion();
  const [active, setActive] = useState<SectionId>(initialSection ?? lastSection);
  // Every section opened so far stays mounted, hidden while another is open:
  // a page's work in flight -- Turn on sync waiting on its check window, an
  // export's progress, a Join code half typed -- must survive a look at
  // another section, as it did when Settings was one long page.
  const [opened, setOpened] = useState<ReadonlySet<SectionId>>(() => new Set([active]));
  const rootRef = useRef<HTMLDivElement | null>(null);

  const go = useCallback((id: SectionId) => {
    lastSection = id;
    setActive(id);
    setOpened((prev) => (prev.has(id) ? prev : new Set(prev).add(id)));
  }, []);

  // A section opens at its top, not at whatever offset the last one was read to.
  useLayoutEffect(() => {
    rootRef.current?.closest(".content")?.scrollTo?.({ top: 0 });
  }, [active]);

  const { settings, set, save, patch, flush, reload } = doc;
  const ctx = useMemo<SettingsCtx | null>(() => settings && {
    settings, set, save, patch, flush, reload,
    tools, sync, browsers,
    cookieFile: cookieFile.state, setCookieFile: cookieFile.setState,
    players, estimate, refreshEstimate, version, setVersion, go,
  }, [
    settings, set, save, patch, flush, reload, tools, sync, browsers, cookieFile.state,
    cookieFile.setState, players, estimate, refreshEstimate, version, setVersion, go,
  ]);

  if (doc.failed) {
    return (
      <div className="page">
        <div className="empty">
          <h2>Settings unavailable</h2>
          <p>Could not read <code>~/.config/mytube/settings.json</code>.</p>
        </div>
      </div>
    );
  }

  if (!ctx) {
    return (
      <div className="page">
        <div className="grid-loading"><span className="spinner" aria-hidden="true" />Loading…</div>
      </div>
    );
  }

  return (
    <SettingsContext.Provider value={ctx}>
      <div className="st-layout" ref={rootRef}>
        <SettingsRail ctx={ctx} active={active} onSelect={go} savedAt={doc.savedAt} />
        <div className="st-pane">
          {SECTIONS.filter((s) => opened.has(s.id)).map(({ id, Page }) => (
            <div key={id} className="st-pane-page" hidden={id !== active}>
              <Page />
            </div>
          ))}
        </div>
      </div>
    </SettingsContext.Provider>
  );
}
