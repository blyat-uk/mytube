import { createContext, useContext } from "react";
import type { SettingsDoc } from "./doc";
import type { CookieFileState, SyncLive, ToolsLive } from "./live";
import type {
  BrowserScan, PlayerOption, Settings, TransferEstimate, VersionInfo,
} from "../../types";

export type SectionId =
  | "downloads" | "signin" | "player" | "new-videos" | "sync" | "backup" | "tools" | "about";

/**
 * Everything a settings page needs: the settings document and its save verbs,
 * plus every piece of live machine state the rail also summarises. The shell
 * builds it once; pages and the rail only read it.
 */
export interface SettingsCtx extends Omit<SettingsDoc, "settings" | "failed" | "savedAt"> {
  settings: Settings;
  tools: ToolsLive;
  sync: SyncLive;
  /** Null while detection runs. */
  browsers: BrowserScan | null;
  /** The configured cookies file's inspection; null while it runs or with none set. */
  cookieFile: CookieFileState | null;
  setCookieFile: (s: CookieFileState) => void;
  /** Null while detection runs. */
  players: PlayerOption[] | null;
  estimate: TransferEstimate | null;
  refreshEstimate: () => void;
  version: VersionInfo | null;
  setVersion: (v: VersionInfo) => void;
  /** Opens another section, e.g. a hint that points at a setting elsewhere. */
  go: (id: SectionId) => void;
}

/**
 * A section's second line in the rail, and whether it needs the user.
 * `tone: "ok"` puts a green dot before the text; `"error"` turns the text pink
 * and marks the item (a pink "!" pill, a red dot on the folded glyph). `flag`
 * is a pill at the item's end: `news` blue (a newer release), `error` pink.
 */
export interface RailLine {
  text: string;
  tone?: "ok" | "error";
  flag?: { text: string; tone: "news" | "error"; label?: string };
}

export const SettingsContext = createContext<SettingsCtx | null>(null);

export function useSettingsCtx(): SettingsCtx {
  const ctx = useContext(SettingsContext);
  if (!ctx) throw new Error("useSettingsCtx outside the Settings view");
  return ctx;
}
