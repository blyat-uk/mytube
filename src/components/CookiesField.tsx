import { useEffect, useState, type ReactNode } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import Field, { hintId } from "./Field";
import { useToast } from "./Toast";
import { api, errText } from "../api";
import { COOKIES_AUTO, type BrowserOption, type Settings } from "../types";

/** The select's value for the cookies.txt entry. No browser spec can take it:
 *  yt-dlp's names are plain words, and a profile suffix follows a colon. */
const FILE = "__file";

const SIGNED_OUT = "members-only and age-restricted videos will fail to download.";

interface Props {
  browser: string;
  file: string;
  /** Every choice here saves at once, and a browser choice writes two keys. */
  onPick: (patch: Partial<Settings>) => void;
}

/**
 * Where yt-dlp gets its YouTube sign-in. A cookies.txt, when set, wins over
 * any browser — that is the backend's rule, so the select shows the file
 * whatever `cookies_browser` still says underneath it, and choosing a browser
 * clears the file rather than leaving it to keep winning out of sight.
 */
export default function CookiesField({ browser, file, onPick }: Props) {
  const toast = useToast();
  const [browsers, setBrowsers] = useState<BrowserOption[] | null>(null);
  const [picking, setPicking] = useState(false);

  useEffect(() => {
    let alive = true;
    api.detectBrowsers()
      .then((list) => { if (alive) setBrowsers(list); })
      // Detection only fills the list; with none, Automatic, None and the file
      // still work, and a browser already chosen is shown as its own entry.
      .catch(() => { if (alive) setBrowsers([]); });
    return () => { alive = false; };
  }, []);

  const firefox = browsers?.find((b) => b.id === "firefox" && b.supported);
  const autoLabel = browsers === null
    ? "Automatic"
    : `Automatic (${firefox ? firefox.label : "none found"})`;

  // A hand-written spec (`chrome:Profile 1`), or a browser whose profile has
  // since gone, is still a legal value: shown as itself, never rewritten.
  const known = browser === COOKIES_AUTO || browser === "" ||
    (browsers ?? []).some((b) => b.id === browser);
  const selected = file ? FILE : browser;
  const current = browsers?.find((b) => b.id === browser);
  const unsupported = (browsers ?? []).filter((b) => !b.supported && b.note);

  async function pickFile() {
    setPicking(true);
    try {
      const path = await open({
        multiple: false,
        directory: false,
        title: "Choose a cookies.txt",
        filters: [{ name: "Cookies", extensions: ["txt"] }],
      });
      if (typeof path === "string" && path) onPick({ cookies_file: path });
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
    hint = "A Netscape-format cookies.txt exported from a signed-in browser. It is used " +
      "instead of any browser.";
  } else if (browser === COOKIES_AUTO) {
    hint = firefox
      ? "Uses Firefox's YouTube sign-in, so members-only and age-restricted videos download as you."
      : browsers === null
        ? "Uses Firefox's YouTube sign-in when there is one."
        : `No Firefox profile here, so downloads run signed out — ${SIGNED_OUT}`;
  } else if (browser === "") {
    hint = `Downloads run signed out — ${SIGNED_OUT}`;
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
          {(browsers ?? []).map((b) => (
            <option key={b.id} value={b.id} disabled={!b.supported}>
              {b.supported ? b.label : `${b.label} (not supported here)`}
            </option>
          ))}
          {!known && <option value={browser}>{browser}</option>}
          <option value={FILE}>cookies.txt file…</option>
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
      {unsupported.map((b) => (
        <span key={b.id} className="field-hint cookies-unsupported">
          {b.label}: {b.note}
        </span>
      ))}
    </Field>
  );
}
