import { api, errText } from "../api";
import { releaseNotesUrl, REPO_URL, useAppVersion } from "../appVersion";
import { formatRelative } from "../format";
import { usePending } from "../pending";
import { useToast } from "./Toast";
import type { VersionInfo } from "../types";

const CHECK = "check";

/** The line under the version, or "" before the first read lands. */
export function aboutStatus(info: VersionInfo | null, now?: number): string {
  if (!info) return "";
  if (!info.checksEnabled) return "Update checks are off.";
  if (info.update) return `MyTube ${info.update.version} is available.`;
  if (info.checkedAt) return `Up to date · checked ${formatRelative(info.checkedAt, now)}`;
  return "Not checked yet.";
}

/**
 * Settings' closing block: which MyTube this is, whether a newer one is out,
 * and the way to its release page. Notify only, like the tray item and the
 * nav's pill — how to update depends on how MyTube was installed.
 */
export default function AboutSection() {
  const toast = useToast();
  const [info, setInfo] = useAppVersion();
  const { pending, slow, run } = usePending();

  const open = (url: string) => {
    api.openExternal(url).catch((err) => toast.error(errText(err)));
  };

  const checkNow = () => run(CHECK, async () => {
    const next = await api.checkAppUpdateNow();
    setInfo(next);
    // A newer release speaks through the status line and its button; only
    // "nothing new" would otherwise look like the click did nothing.
    if (!next.update) toast.info(`MyTube ${next.current} is up to date.`);
  });

  return (
    <section className="settings-block about-block" aria-labelledby="about-title">
      <h3 className="field-label" id="about-title">About</h3>
      <p className="about-version">
        MyTube <span className="about-number">{info?.current ?? ""}</span>
      </p>
      <p className="field-hint about-status" role="status">{aboutStatus(info)}</p>

      <div className="row-inline about-actions">
        {info?.update && (
          <button type="button" className="btn btn-primary" onClick={() => open(info.update!.url)}>
            Get {info.update.version}…
          </button>
        )}
        <button
          type="button"
          className="btn"
          disabled={!info?.checksEnabled || pending.has(CHECK)}
          aria-busy={pending.has(CHECK)}
          title={info && !info.checksEnabled
            ? "Turn on “Check GitHub for new MyTube releases” above"
            : undefined}
          onClick={() => void checkNow()}
        >
          {slow.has(CHECK) ? "Checking…" : "Check now"}
        </button>
        {info && (
          <button type="button" className="btn" onClick={() => open(releaseNotesUrl(info.current))}>
            Release notes…
          </button>
        )}
        <button type="button" className="btn" onClick={() => open(REPO_URL)}>
          GitHub…
        </button>
      </div>
    </section>
  );
}
