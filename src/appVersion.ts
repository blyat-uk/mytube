import { useEffect, useRef, useState } from "react";
import { api } from "./api";
import { useVersionInfo } from "./events";
import type { VersionInfo } from "./types";

export const REPO_URL = "https://github.com/blyat-uk/mytube";

/** This build's own release page, tagged `v{version}` like every release. */
export const releaseNotesUrl = (version: string) => `${REPO_URL}/releases/tag/v${version}`;

/**
 * This build's version and any newer release, kept current: read once at
 * mount, then followed through `app://version-info`.
 *
 * An event that lands while the first read is still in flight wins over it —
 * the read may have left before the check that sent the event finished, and
 * applying it second would put the older answer back on screen.
 */
export function useAppVersion(): [VersionInfo | null, (info: VersionInfo) => void] {
  const [info, setInfo] = useState<VersionInfo | null>(null);
  const heard = useRef(false);

  useEffect(() => {
    let alive = true;
    api.appVersionInfo()
      .then((v) => { if (alive && !heard.current) setInfo(v); })
      // Without it the pill and the About block simply stay empty; nothing
      // else depends on knowing the version.
      .catch(() => {});
    return () => { alive = false; };
  }, []);

  useVersionInfo((v) => {
    heard.current = true;
    setInfo(v);
  });

  return [info, setInfo];
}
