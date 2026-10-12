import { api, errText } from "../../api";
import { releaseNotesUrl, REPO_URL } from "../../appVersion";
import { formatRelative } from "../../format";
import { usePending } from "../../pending";
import { useToast } from "../Toast";
import { IconExternal, IconNewRelease, IconSectionAbout } from "../Icons";
import { useSettingsCtx, type RailLine, type SettingsCtx } from "./context";
import { Block, SettingsPage, StatusCard, Switch, type StatusTone } from "./parts";
import type { VersionInfo } from "../../types";

/**
 * "MyTube 4.0.0", with a green "↑ 4.1.0" pill while checks are on and a newer
 * release is known: green and ↑ are the update signal everywhere, the chip
 * beside the Settings tab included. The pill is drawn only; `label` is what a
 * screen reader hears in its place.
 */
export function summary({ version }: SettingsCtx): RailLine {
  if (!version) return { text: "MyTube" };
  const line: RailLine = { text: `MyTube ${version.current}` };
  if (version.checksEnabled && version.update) {
    const next = version.update.version;
    line.flag = { text: `↑ ${next}`, tone: "news", label: `MyTube ${next} is available` };
  }
  return line;
}

export interface AboutStatus {
  tone: StatusTone;
  title: string;
  detail?: string;
}

/**
 * The status card's sentence, or null before the first read lands. Checks
 * being off wins over everything: a stored "newer release" is not news the
 * user asked for, and "up to date" would claim something nobody looked at.
 */
export function aboutStatus(info: VersionInfo | null, now?: number): AboutStatus | null {
  if (!info) return null;
  if (!info.checksEnabled) return { tone: "neutral", title: "Update checks are off." };
  if (info.update) {
    return {
      tone: "news",
      title: `MyTube ${info.update.version} is available`,
      detail: "MyTube never updates itself: get it the same way you installed this one.",
    };
  }
  if (info.checkedAt) {
    return { tone: "ok", title: "Up to date", detail: `Checked ${formatRelative(info.checkedAt, now)}` };
  }
  return { tone: "neutral", title: "Not checked yet." };
}

const CHECK = "check";

/**
 * Installed → Available: the two numbers an update is about, each named. Terms
 * and values rather than two bare numbers, so a screen reader hears "Installed
 * 4.2.0, Available 4.3.0"; the arrow between them is CSS, which a `<dl>` needs
 * anyway (it cannot hold a loose element) and which keeps it from being read.
 */
function VersionCompare({ installed, available }: { installed: string; available: string }) {
  return (
    <dl className="st-ab-compare">
      <div className="st-ab-compare-item">
        <dt className="st-ab-compare-label">Installed</dt>
        <dd className="st-ab-compare-value">{installed}</dd>
      </div>
      <div className="st-ab-compare-item">
        <dt className="st-ab-compare-label">Available</dt>
        <dd className="st-ab-compare-value is-new">{available}</dd>
      </div>
    </dl>
  );
}

/**
 * Which MyTube this is, whether a newer one is out, and the way to its release
 * page. Every route to an update ends here: the nav has no version pill any
 * more, so the green ↑ chip beside the Settings tab opens this page, and so do
 * the once-per-release toast and the tray's "Update available" item. Notify
 * only — how to update depends on how MyTube was installed (deb, rpm, AppImage,
 * NSIS, dmg) — which is why a newer release is spelled out as Installed →
 * Available and handed over as its release page.
 *
 * The hero stands in for the page header, which stays as the page's heading
 * for a screen reader but is not drawn: a second red tile above the big mark
 * would say "About" twice.
 */
export default function AboutPage() {
  const { settings, save, version, setVersion } = useSettingsCtx();
  const toast = useToast();
  const { pending, slow, run } = usePending();
  const status = aboutStatus(version);
  const checksOn = settings.check_app_updates;
  // Only while the card says so: a release stored with checks off is not news.
  const update = status?.tone === "news" && version?.update ? version.update : null;

  const open = (url: string) => {
    api.openExternal(url).catch((err) => toast.error(errText(err)));
  };

  const checkNow = () => run(CHECK, async () => {
    const next = await api.checkAppUpdateNow();
    setVersion(next);
    // A newer release speaks through the status card and its button; only
    // "nothing new" would otherwise look like the click did nothing. A check
    // the backend answered as switched off found nothing either way.
    if (next.checksEnabled && !next.update) toast.info(`MyTube ${next.current} is up to date.`);
  });

  return (
    <SettingsPage icon={<IconSectionAbout />} title="About" className="st-ab-page">
      <div className="st-ab-hero">
        <span className="st-ab-mark" aria-hidden="true" />
        <div className="st-ab-hero-text">
          <p className="st-ab-name">
            MyTube{" "}
            {version && <span className="st-ab-version">{version.current}</span>}
          </p>
          <p className="st-ab-tag">
            Your subscriptions, on your own computer. Nothing is streamed and nothing is written
            back to YouTube.
          </p>
        </div>
      </div>

      {status && (
        <StatusCard
          tone={status.tone}
          icon={status.tone === "news" ? <IconNewRelease /> : undefined}
          title={status.title}
          action={update ? (
            <button type="button" className="btn btn-primary" onClick={() => open(update.url)}>
              Open release page…
            </button>
          ) : undefined}
        >
          {update && version ? (
            <>
              <VersionCompare installed={version.current} available={update.version} />
              {status.detail}
            </>
          ) : status.detail}
        </StatusCard>
      )}

      <Block>
        <Switch
          label="Check GitHub for new releases"
          hint="Once a day. MyTube only tells you; it never updates itself."
          checked={checksOn}
          onChange={(on) => save("check_app_updates", on)}
          extra={
            // Follows the switch beside it rather than the last check's answer,
            // which arrives a network round trip after the switch is flipped.
            <button
              type="button"
              className="btn"
              disabled={!checksOn || pending.has(CHECK)}
              aria-busy={pending.has(CHECK)}
              title={checksOn ? undefined : "Turn the switch on to check"}
              onClick={() => void checkNow()}
            >
              {slow.has(CHECK) ? "Checking…" : "Check now"}
            </button>
          }
        />
      </Block>

      <div className="st-ab-links">
        {version && (
          <button type="button" className="btn" onClick={() => open(releaseNotesUrl(version.current))}>
            <span className="st-ab-link-icon" aria-hidden="true"><IconExternal /></span>
            Release notes for {version.current}…
          </button>
        )}
        <button type="button" className="btn" onClick={() => open(REPO_URL)}>
          <span className="st-ab-link-icon" aria-hidden="true"><IconExternal /></span>
          GitHub…
        </button>
      </div>
    </SettingsPage>
  );
}
