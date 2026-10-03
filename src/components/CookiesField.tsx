import { useEffect, useState, type ReactNode } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import Field, { hintId } from "./Field";
import { useToast } from "./Toast";
import { api, errText } from "../api";
import {
  COOKIES_AUTO, type BrowserOption, type BrowserScan, type CookiesFileInfo, type Settings,
} from "../types";

/** The select's value for the cookies file entry. No browser spec can take it:
 *  yt-dlp's names are plain words, and a profile suffix follows a colon. */
const FILE = "__file";

const SIGNED_OUT = "members-only and age-restricted videos will fail to download.";

interface Props {
  browser: string;
  file: string;
  /** Every choice here saves at once, and a browser choice writes two keys. */
  onPick: (patch: Partial<Settings>) => void;
}

/** What `inspect_cookies_file` said about one path: what it holds, or why it
 *  cannot be used. */
type FileState = { path: string; info: CookiesFileInfo } | { path: string; error: string };

const plural = (n: number, one: string) => `${n} ${one}${n === 1 ? "" : "s"}`;

/** A browser in the list: what it is, and why it cannot be picked if so. */
function optionLabel(b: BrowserOption): string {
  if (b.blocked) return `${b.label} (no access)`;
  if (!b.supported) return `${b.label} (not supported here)`;
  return b.signedIn ? `${b.label} (signed in)` : b.label;
}

/** The hint for a browser that can be used, chosen by hand or by Automatic. */
function browserHint(b: BrowserOption, automatic: boolean): string {
  const said = b.signedIn === true
    ? `Uses ${b.label}, which is signed in to YouTube, so members-only and age-restricted ` +
      "videos download as you."
    : b.signedIn === false
      ? automatic
        ? `Uses ${b.label}, the browser used last, but none MyTube can read is signed in to ` +
          `YouTube, so ${SIGNED_OUT}`
        : `${b.label} is not signed in to YouTube, so ${SIGNED_OUT}`
      : `Uses ${b.label}'s YouTube sign-in.`;
  return b.note ? `${said} ${b.note}` : said;
}

function fileHint(state: FileState | null): ReactNode {
  if (!state) {
    return "A cookies file exported from a browser signed in to YouTube. It is used instead " +
      "of any browser.";
  }
  if ("error" in state) return <span className="cookies-error">{state.error}</span>;
  const { info } = state;
  const what = `${info.format}, ${plural(info.cookies, "cookie")}` +
    (info.skipped ? ` (${plural(info.skipped, "unreadable entry")} skipped)` : "");
  if (info.signedIn) return `${what}, signed in to YouTube. It is used instead of any browser.`;
  return info.youtube
    ? `${what}, but no YouTube sign-in among them, so ${SIGNED_OUT}`
    : `${what}, none of them for YouTube, so ${SIGNED_OUT}`;
}

/**
 * Where yt-dlp gets its YouTube sign-in. A cookies file, when set, wins over
 * any browser — that is the backend's rule, so the select shows the file
 * whatever `cookies_browser` still says underneath it, and choosing a browser
 * clears the file rather than leaving it to keep winning out of sight.
 *
 * Automatic is whichever browser the backend finds signed in to YouTube (else
 * the one used last), and the list names no browser as expected: what it says
 * comes from what is on this machine.
 */
export default function CookiesField({ browser, file, onPick }: Props) {
  const toast = useToast();
  const [scan, setScan] = useState<BrowserScan | null>(null);
  const [picking, setPicking] = useState(false);
  const [fileState, setFileState] = useState<FileState | null>(null);

  useEffect(() => {
    let alive = true;
    api.detectBrowsers()
      .then((s) => { if (alive) setScan(s); })
      // Detection only fills the list; with none, Automatic, None and the file
      // still work, and a browser already chosen is shown as its own entry.
      .catch(() => { if (alive) setScan({ browsers: [], automatic: null, accessHint: null }); });
    return () => { alive = false; };
  }, []);

  // The file is read again whenever it is shown, so one that has since been
  // moved or overwritten says so here rather than at the next download.
  useEffect(() => {
    if (!file) return;
    let alive = true;
    api.inspectCookiesFile(file)
      .then((info) => { if (alive) setFileState({ path: file, info }); })
      .catch((err) => { if (alive) setFileState({ path: file, error: errText(err) }); });
    return () => { alive = false; };
  }, [file]);

  const browsers = scan?.browsers ?? [];
  const auto = browsers.find((b) => b.id === scan?.automatic);
  const autoLabel = scan === null
    ? "Automatic"
    : `Automatic (${auto ? auto.label : browsers.length ? "none usable" : "none found"})`;

  // A hand-written spec (`chrome:Profile 1`), or a browser whose profile has
  // since gone, is still a legal value: shown as itself, never rewritten.
  const known = browser === COOKIES_AUTO || browser === "" ||
    browsers.some((b) => b.id === browser);
  const selected = file ? FILE : browser;
  const current = browsers.find((b) => b.id === browser);
  const unusable = browsers.filter((b) => !b.supported && b.note);

  async function pickFile() {
    setPicking(true);
    try {
      // No filter: an export is as often .json, or has no extension at all,
      // and the backend reads what is inside rather than trusting the name.
      const path = await open({ multiple: false, directory: false, title: "Choose a cookies file" });
      if (typeof path !== "string" || !path) return;
      // Checked before it is saved: a file that cannot be used is refused here,
      // with the reason, instead of failing every download later.
      const info = await api.inspectCookiesFile(path);
      setFileState({ path, info });
      onPick({ cookies_file: path });
    } catch (err) {
      toast.error(errText(err));
    } finally {
      setPicking(false);
    }
  }

  function choose(value: string) {
    if (value === FILE) {
      void pickFile();
      return;
    }
    onPick({ cookies_browser: value, cookies_file: "" });
  }

  let hint: ReactNode;
  if (file) {
    hint = fileHint(fileState?.path === file ? fileState : null);
  } else if (browser === COOKIES_AUTO) {
    hint = scan === null
      ? "Uses the browser you are signed in to YouTube with."
      : auto
        ? browserHint(auto, true)
        : `${browsers.length ? "No browser here can be used" : "No browser found"}, so ` +
          `downloads run signed out — ${SIGNED_OUT}`;
  } else if (browser === "") {
    hint = `Downloads run signed out — ${SIGNED_OUT}`;
  } else if (current && current.supported) {
    hint = browserHint(current, false);
  } else {
    hint = current?.note ?? `Uses ${current?.label ?? browser}'s YouTube sign-in.`;
  }

  return (
    <Field label="YouTube cookies" htmlFor="cookies-select" hint={hint}>
      <div className="row-inline">
        <select
          id="cookies-select"
          className="select settings-select"
          value={selected}
          disabled={picking}
          aria-describedby={hint ? hintId("cookies-select") : undefined}
          onChange={(e) => choose(e.currentTarget.value)}
        >
          <option value={COOKIES_AUTO}>{autoLabel}</option>
          <option value="">None</option>
          {browsers.map((b) => (
            <option key={b.id} value={b.id} disabled={!b.supported}>{optionLabel(b)}</option>
          ))}
          {!known && <option value={browser}>{browser}</option>}
          <option value={FILE}>Cookies file…</option>
        </select>
        {file && (
          <button
            type="button"
            className="btn"
            disabled={picking}
            onClick={() => void pickFile()}
          >
            Change…
          </button>
        )}
      </div>
      {file && <code className="cookies-file" title={file}>{file}</code>}
      {/* Said beside the list, not inside it: a disabled option cannot be
          hovered for a tooltip in most webviews, and without the reason it
          reads as a bug. */}
      {unusable.map((b) => (
        <span key={b.id} className="field-hint cookies-note">
          {b.label}: {b.note}
        </span>
      ))}
      {scan?.accessHint && <span className="field-hint cookies-note">{scan.accessHint}</span>}
    </Field>
  );
}
