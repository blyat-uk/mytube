import { useId, type ReactNode } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api, errText } from "../../api";
import { usePending } from "../../pending";
import { useToast } from "../Toast";
import {
  COOKIES_AUTO, type BrowserOption, type BrowserScan, type CookiesFileInfo,
} from "../../types";
import {
  IconAutomatic, IconBrowser, IconCheck, IconFile, IconInfo, IconNone, IconSectionSignIn,
  IconWarning,
} from "../Icons";
import { useSettingsCtx, type RailLine, type SettingsCtx } from "./context";
import type { CookieFileState } from "./live";
import {
  Block, Cap, Choice, ChoiceGap, ChoiceList, Pill, SettingsPage, StatusCard, type StatusTone,
} from "./parts";

const SIGNED_OUT = "members-only and age-restricted videos will fail to download.";

/** The `usePending` key for choosing a cookies file: the dialog, then the read. */
const PICK = "cookies-file";

const plural = (n: number, one: string) => `${n} ${one}${n === 1 ? "" : "s"}`;

const NEEDS_ATTENTION = { text: "!", tone: "error", label: "Needs attention" } as const;

/** "Firefox · signed in" (green dot), "Cookies file · signed in", "Signed out". */
export function summary({ settings: s, browsers, cookieFile }: SettingsCtx): RailLine {
  if (s.cookies_file) {
    if (!cookieFile) return { text: "Cookies file" };
    if ("error" in cookieFile) {
      return { text: "Cookies file can't be read", tone: "error", flag: NEEDS_ATTENTION };
    }
    return cookieFile.info.signedIn
      ? { text: "Cookies file · signed in", tone: "ok" }
      : { text: "Cookies file · not signed in" };
  }
  if (s.cookies_browser === "") return { text: "Signed out" };
  if (s.cookies_browser === COOKIES_AUTO) {
    if (!browsers) return { text: "Automatic" };
    const auto = browsers.browsers.find((b) => b.id === browsers.automatic);
    if (!auto) return { text: "Signed out" };
    if (auto.signedIn === true) return { text: `${auto.label} · signed in`, tone: "ok" };
    if (auto.signedIn === false) return { text: `${auto.label} · not signed in` };
    return { text: auto.label };
  }
  const b = browsers?.browsers.find((x) => x.id === s.cookies_browser);
  if (!b) return { text: s.cookies_browser };
  // A browser chosen by hand that cannot be read is not "signed out": yt-dlp
  // is still handed it, and every download fails on it -- the same failure as
  // an unreadable cookies file, so the same mark.
  if (b.blocked) return { text: `${b.label} · no access`, tone: "error", flag: NEEDS_ATTENTION };
  if (!b.supported) return { text: `${b.label} · not supported`, tone: "error", flag: NEEDS_ATTENTION };
  if (b.signedIn === true) return { text: `${b.label} · signed in`, tone: "ok" };
  if (b.signedIn === false) return { text: `${b.label} · not signed in` };
  return { text: b.label };
}

/** What a readable cookies file holds, in the words the file's row shows. */
function fileHint(info: CookiesFileInfo): string {
  const what = `${info.format}, ${plural(info.cookies, "cookie")}` +
    (info.skipped ? ` (${plural(info.skipped, "unreadable entry")} skipped)` : "");
  if (info.signedIn) return `${what}, signed in to YouTube. It is used instead of any browser.`;
  return info.youtube
    ? `${what}, but no YouTube sign-in among them, so ${SIGNED_OUT}`
    : `${what}, none of them for YouTube, so ${SIGNED_OUT}`;
}

interface Status {
  tone: StatusTone;
  /** None draws the tone's dot: a state still being found out. */
  icon?: ReactNode;
  title: string;
  detail: string;
}

const withNote = (said: string, b: BrowserOption) => (b.note ? `${said} ${b.note}` : said);

const SIGNED_IN = "Downloads run signed in, as you";
const RUNS_SIGNED_OUT = "Downloads run signed out";

/** What a usable browser means for downloads, picked by hand or by Automatic. */
function browserStatus(b: BrowserOption, scan: BrowserScan, automatic: boolean): Status {
  if (b.signedIn === true) {
    // "The" browser only while it is the one: between several, Automatic
    // takes the one used last, and the sentence should not claim otherwise.
    const others = scan.browsers.filter((x) => x.supported && x.signedIn === true).length > 1;
    const said = automatic
      ? others
        ? `Through ${b.label}, picked automatically: of the browsers signed in to YouTube, it ` +
          "was used last."
        : `Through ${b.label}, picked automatically: it is the browser signed in to YouTube.`
      : `Through ${b.label}, which is signed in to YouTube.`;
    return { tone: "ok", icon: <IconCheck />, title: SIGNED_IN, detail: withNote(said, b) };
  }
  if (b.signedIn === false) {
    const said = automatic
      ? `Uses ${b.label}, the browser used last, but none MyTube can read is signed in to ` +
        `YouTube, so ${SIGNED_OUT}`
      : `${b.label} is not signed in to YouTube, so ${SIGNED_OUT}`;
    return { tone: "warn", icon: <IconWarning />, title: RUNS_SIGNED_OUT, detail: withNote(said, b) };
  }
  const said = automatic
    ? "Picked automatically; MyTube can't see whether it is signed in."
    : `Uses ${b.label}'s YouTube sign-in; MyTube can't see whether it is signed in.`;
  return {
    tone: "neutral", icon: <IconInfo />, title: `Downloads use ${b.label}'s sign-in`,
    detail: withNote(said, b),
  };
}

/**
 * The status card: what happens to a download right now, in one sentence.
 * A cookies file, when set, wins over any browser -- that is the backend's
 * rule -- so it is asked about first, whatever `cookies_browser` still holds.
 */
function statusOf(
  browser: string, file: string, scan: BrowserScan | null, fileState: CookieFileState | null,
): Status {
  if (file) {
    if (!fileState) {
      return {
        tone: "neutral", title: "Reading the cookies file…",
        detail: "A cookies file exported from a browser signed in to YouTube. It is used " +
          "instead of any browser.",
      };
    }
    if ("error" in fileState) {
      // Not "signed out": the backend refuses to run a download on a file it
      // cannot read rather than quietly dropping the sign-in.
      return {
        tone: "error", icon: <IconWarning />, title: "The cookies file can't be used",
        detail: fileState.error,
      };
    }
    const { info } = fileState;
    if (info.signedIn) {
      return {
        tone: "ok", icon: <IconCheck />, title: SIGNED_IN,
        detail: "Through the cookies file below, used instead of any browser.",
      };
    }
    return {
      tone: "warn", icon: <IconWarning />, title: RUNS_SIGNED_OUT,
      detail: info.youtube
        ? `The cookies file holds no YouTube sign-in, so ${SIGNED_OUT}`
        : `None of the cookies file's cookies are for YouTube, so ${SIGNED_OUT}`,
    };
  }
  if (browser === "") {
    return {
      tone: "warn", icon: <IconWarning />, title: RUNS_SIGNED_OUT,
      detail: `No sign-in is used, so ${SIGNED_OUT}`,
    };
  }
  if (scan === null) {
    return {
      tone: "neutral", title: "Looking for browsers…",
      detail: browser === COOKIES_AUTO
        ? "Uses the browser you are signed in to YouTube with."
        : `Uses ${browser}'s YouTube sign-in.`,
    };
  }
  if (browser === COOKIES_AUTO) {
    const auto = scan.browsers.find((b) => b.id === scan.automatic);
    if (auto) return browserStatus(auto, scan, true);
    return {
      tone: "warn", icon: <IconWarning />, title: RUNS_SIGNED_OUT,
      detail: `${scan.browsers.length ? "No browser here can be used" : "No browser found"}, ` +
        `so ${SIGNED_OUT}`,
    };
  }
  const current = scan.browsers.find((b) => b.id === browser);
  if (!current) {
    // A hand-written spec (`chrome:Profile 1`), or a browser since removed:
    // yt-dlp is handed it verbatim, and nothing here can look inside it.
    return {
      tone: "neutral", icon: <IconInfo />, title: "Downloads use the browser you named",
      detail: `Uses ${browser}'s YouTube sign-in, passed to yt-dlp as written.`,
    };
  }
  if (!current.supported) {
    // Chosen before it became unreadable (a macOS update, a settings.json
    // carried over from Linux): yt-dlp is still handed it and fails on it.
    return {
      tone: "error", icon: <IconWarning />, title: `${current.label} can't be used`,
      detail: `${current.note ?? "yt-dlp can't read its cookies here."} Downloads fail while ` +
        "it is chosen.",
    };
  }
  return browserStatus(current, scan, false);
}

/** A browser row's state at its right end. */
function browserPill(b: BrowserOption): ReactNode {
  if (b.blocked) return <Pill tone="warn">No access</Pill>;
  if (!b.supported) return <Pill tone="warn">Not supported</Pill>;
  if (b.signedIn === true) return <Pill tone="ok">Signed in</Pill>;
  if (b.signedIn === false) return <Pill tone="dim">Not signed in</Pill>;
  return null;
}

function filePill(state: CookieFileState | null): ReactNode {
  if (!state) return null;
  if ("error" in state) return <Pill tone="error">Can't be read</Pill>;
  return state.info.signedIn
    ? <Pill tone="ok">Signed in</Pill>
    : <Pill tone="dim">Not signed in</Pill>;
}

/**
 * Where yt-dlp gets its YouTube sign-in, as a list of every source this machine
 * has. Choosing a browser clears the cookies file rather than leaving it to
 * keep winning out of sight. Automatic is whichever browser the backend finds
 * signed in to YouTube (else the one used last), and nothing here names a
 * browser as expected: every row comes from what is on this machine.
 */
export default function SignInPage() {
  const { settings, patch, browsers: scan, cookieFile, setCookieFile } = useSettingsCtx();
  const toast = useToast();
  const { pending, run } = usePending();
  const name = useId();
  const capId = useId();

  const browser = settings.cookies_browser;
  const file = settings.cookies_file;
  // The radios are controlled by settings alone, so a dismissed dialog or a
  // refused file leaves the row that was checked checked.
  const picking = pending.has(PICK);

  const list = scan?.browsers ?? [];
  const auto = list.find((b) => b.id === scan?.automatic);
  // A hand-written spec (`chrome:Profile 1`), or a browser whose profile has
  // since gone, is still a legal value: shown as itself, never rewritten.
  const known = browser === COOKIES_AUTO || browser === "" || list.some((b) => b.id === browser);
  const status = statusOf(browser, file, scan, cookieFile);

  function choose(value: string) {
    // One save for both keys: two would race, and the file must not keep
    // winning over the browser just chosen.
    patch({ cookies_browser: value, cookies_file: "" });
  }

  function pickFile() {
    void run(PICK, async () => {
      // No filter: an export is as often .json, or has no extension at all,
      // and the backend reads what is inside rather than trusting the name.
      const path = await open({ multiple: false, directory: false, title: "Choose a cookies file" });
      if (typeof path !== "string" || !path) return;
      // Checked before it is saved: a file that cannot be used is refused here,
      // with the reason, instead of failing every download later.
      const info = await api.inspectCookiesFile(path);
      setCookieFile({ path, info });
      patch({ cookies_file: path });
    });
  }

  // Opening System Settings is fire-and-forget, like About's links: there is
  // nothing to wait for, so no pending state.
  function openFullDiskAccess() {
    api.openFullDiskAccess().catch((err) => toast.error(errText(err)));
  }

  const autoPill = scan === null
    ? null
    : auto
      ? <Pill tone="blue">{auto.label}</Pill>
      : <Pill tone="dim">{list.length ? "none usable" : "none found"}</Pill>;

  return (
    <SettingsPage
      icon={<IconSectionSignIn />}
      title="YouTube sign-in"
      intro={
        "Members-only and age-restricted videos only download for someone signed in. MyTube " +
        "borrows that sign-in from a browser, or from a cookies file you export."
      }
    >
      <StatusCard tone={status.tone} icon={status.icon} title={status.title}>
        {status.detail}
      </StatusCard>

      <Block>
        <Cap id={capId}>Sign-in comes from</Cap>
        <ChoiceList labelledBy={capId}>
          <Choice
            name={name}
            checked={!file && browser === COOKIES_AUTO}
            onSelect={() => choose(COOKIES_AUTO)}
            disabled={picking}
            icon={<IconAutomatic />}
            title="Automatic"
            detail="Whichever browser is signed in to YouTube; between several, the one used last"
            pill={autoPill}
          />
          <ChoiceGap />
          {scan === null && <div className="st-si-looking">Looking for browsers…</div>}
          {list.map((b) => (
            <Choice
              key={b.id}
              name={name}
              checked={!file && browser === b.id}
              onSelect={() => choose(b.id)}
              // Said in the row, not in a tooltip: a disabled control cannot be
              // hovered in most webviews, and without the reason it reads as a bug.
              disabled={picking || !b.supported}
              icon={<IconBrowser />}
              title={b.label}
              detail={b.note ?? undefined}
              pill={browserPill(b)}
            />
          ))}
          {scan !== null && !known && (
            <Choice
              name={name}
              checked={!file}
              onSelect={() => choose(browser)}
              disabled={picking}
              icon={<IconBrowser />}
              title={<span className="st-si-spec">{browser}</span>}
              detail="Named in settings.json; passed to yt-dlp as written"
            />
          )}
          <ChoiceGap />
          <Choice
            name={name}
            checked={!!file}
            onSelect={pickFile}
            disabled={picking}
            icon={<IconFile />}
            title={file ? "Cookies file" : "Cookies file…"}
            detail={file ? undefined : "A Netscape, JSON or name=value export; used instead of any browser"}
            pill={file ? filePill(cookieFile) : null}
          >
            <div className="st-si-file">
              <code className="st-si-path" title={file}>{file}</code>
              <button type="button" className="btn" disabled={picking} onClick={pickFile}>
                Change…
              </button>
            </div>
            {/* An unreadable file's reason is the status card's, in pink, once. */}
            {cookieFile && "info" in cookieFile && (
              <p className="st-si-file-hint">{fileHint(cookieFile.info)}</p>
            )}
          </Choice>
          <Choice
            name={name}
            checked={!file && browser === ""}
            onSelect={() => choose("")}
            disabled={picking}
            icon={<IconNone />}
            title="None"
            detail="Download signed out; members-only and age-restricted videos will fail"
          />
        </ChoiceList>
      </Block>

      {scan?.accessHint && (
        <Block className="st-si-access">
          <p className="st-si-access-text">{scan.accessHint}</p>
          <button type="button" className="btn" onClick={openFullDiskAccess}>
            Open Full Disk Access settings
          </button>
        </Block>
      )}
    </SettingsPage>
  );
}
