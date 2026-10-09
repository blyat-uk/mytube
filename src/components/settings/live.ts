import { useCallback, useEffect, useRef, useState } from "react";
import { api, errText } from "../../api";
import { useSyncStatus, useToolsProgress, useToolsStatus } from "../../events";
import type {
  BrowserScan, CookiesFileInfo, PlayerOption, SyncStatus, ToolKind, ToolProgress, ToolState,
  ToolStatus, TransferEstimate,
} from "../../types";

/*
 * What the Settings view reads from the machine rather than from settings.json.
 * Each hook used to live inside the one section that drew it; the rail now
 * summarises every section at once, so the shell owns one copy of each and
 * hands it to both through the context.
 */

export const toolInFlight = (s: ToolState) => s === "installing" || s === "updating";

export interface ToolsLive {
  tools: ToolStatus[] | null;
  /** The running download of each tool being installed or updated. */
  progress: Partial<Record<ToolKind, ToolProgress>>;
  /** Why the first read failed; said in place, since a toast would be gone
   *  while the section sat on "Checking…" forever. */
  loadError: string | null;
  /** Takes a fresh list, e.g. what `tools_update_now` handed back. */
  apply: (list: ToolStatus[]) => void;
}

export function useToolsLive(): ToolsLive {
  const [tools, setTools] = useState<ToolStatus[] | null>(null);
  const [progress, setProgress] = useState<Partial<Record<ToolKind, ToolProgress>>>({});
  const [loadError, setLoadError] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    api.toolsStatus()
      .then((list) => { if (alive) setTools(list); })
      .catch((err) => { if (alive) setLoadError(errText(err)); });
    return () => { alive = false; };
  }, []);

  const apply = useCallback((list: ToolStatus[]) => {
    setTools(list);
    setLoadError(null);
    // A finished download's last byte count would otherwise sit under the row
    // and reappear, stale, the next time that tool installs.
    setProgress((prev) => {
      const next = { ...prev };
      for (const t of list) if (!toolInFlight(t.state)) delete next[t.kind];
      return next;
    });
  }, []);

  useToolsStatus(apply);
  useToolsProgress((p) => setProgress((prev) => ({ ...prev, [p.tool]: p })));

  return { tools, progress, loadError, apply };
}

export interface SyncLive {
  status: SyncStatus | null;
  loadError: string | null;
  /** Every sync command hands back a fresh status; it goes through here. */
  setStatus: (s: SyncStatus) => void;
}

export function useSyncLive(): SyncLive {
  const [status, setStatusState] = useState<SyncStatus | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  // An event that lands while the first read is out is the newer of the two.
  const heard = useRef(false);

  useEffect(() => {
    let alive = true;
    // Through `Promise.resolve()` so a missing command rejects rather than
    // throwing out of the effect.
    Promise.resolve()
      .then(() => api.syncStatus())
      .then((s) => { if (alive && !heard.current) setStatusState(s); })
      .catch((err) => { if (alive && !heard.current) setLoadError(errText(err)); });
    return () => { alive = false; };
  }, []);

  const setStatus = useCallback((s: SyncStatus) => {
    heard.current = true;
    setStatusState(s);
    setLoadError(null);
  }, []);

  useSyncStatus(setStatus);

  return { status, loadError, setStatus };
}

const NO_BROWSERS: BrowserScan = { browsers: [], automatic: null, accessHint: null };

/** The browsers on this machine, or null while detection runs. A failure reads
 *  as none found: Automatic, None and a file still work without the list. */
export function useBrowserScan(): BrowserScan | null {
  const [scan, setScan] = useState<BrowserScan | null>(null);
  useEffect(() => {
    let alive = true;
    api.detectBrowsers()
      .then((s) => { if (alive) setScan(s); })
      .catch(() => { if (alive) setScan(NO_BROWSERS); });
    return () => { alive = false; };
  }, []);
  return scan;
}

/** What `inspect_cookies_file` said about one path: what it holds, or why it
 *  cannot be used. */
export type CookieFileState = { path: string; info: CookiesFileInfo } | { path: string; error: string };

/**
 * The cookies file in settings, read again whenever it is shown, so one that has
 * since been moved or overwritten says so here rather than at the next download.
 * `state` is null while the read is out, or when no file is set; a state for a
 * path other than `file` is stale and should be ignored by the caller.
 */
export function useCookieFile(file: string): {
  state: CookieFileState | null;
  setState: (s: CookieFileState) => void;
} {
  const [state, setState] = useState<CookieFileState | null>(null);
  useEffect(() => {
    if (!file) return;
    let alive = true;
    api.inspectCookiesFile(file)
      .then((info) => { if (alive) setState({ path: file, info }); })
      .catch((err) => { if (alive) setState({ path: file, error: errText(err) }); });
    return () => { alive = false; };
  }, [file]);
  return { state: state && state.path === file ? state : null, setState };
}

/** What the list holds when detection fails: the one entry that needs nothing
 *  installed. The backend always puts it first. */
export const DEFAULT_PLAYERS: PlayerOption[] = [{ id: "default", label: "System default", command: "" }];

/** Installed players, System default first; null while detection runs. */
export function usePlayers(): PlayerOption[] | null {
  const [players, setPlayers] = useState<PlayerOption[] | null>(null);
  useEffect(() => {
    let alive = true;
    api.detectPlayers()
      .then((list) => { if (alive) setPlayers(list.length ? list : DEFAULT_PLAYERS); })
      .catch(() => { if (alive) setPlayers(DEFAULT_PLAYERS); });
    return () => { alive = false; };
  }, []);
  return players;
}

/** The library's size for Backup & transfer and its rail line. A missing
 *  estimate costs the numbers and nothing else: an export that cannot be
 *  weighed can still be written. */
export function useTransferEstimate(): { estimate: TransferEstimate | null; refresh: () => void } {
  const [estimate, setEstimate] = useState<TransferEstimate | null>(null);
  const [tick, setTick] = useState(0);
  useEffect(() => {
    let alive = true;
    api.transferEstimate()
      .then((e) => { if (alive) setEstimate(e); })
      .catch(() => {});
    return () => { alive = false; };
  }, [tick]);
  const refresh = useCallback(() => setTick((n) => n + 1), []);
  return { estimate, refresh };
}
