import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { screen, fireEvent, cleanup, waitFor } from "@testing-library/react";

vi.mock("@tauri-apps/api/event", async () => (await import("./testApi")).eventModule);
vi.mock("@tauri-apps/plugin-dialog", async () => (await import("./testApi")).dialogModule);
vi.mock("../../api", async () => (await import("./testApi")).apiModule);

import {
  api, backend, BROWSERS, browserOpt, CHROME_NOTE, dialog, FILE_INFO, lastSaved, resetHarness,
  SCAN, SETTINGS,
} from "./testApi";
import { railItem, renderSettings } from "./testRender";
import { summary } from "./SignInPage";
import type { SettingsCtx } from "./context";
import type { CookieFileState } from "./live";
import type { BrowserScan, Settings } from "../../types";

beforeEach(resetHarness);
afterEach(cleanup);

const SIGNED_OUT = "members-only and age-restricted videos will fail to download.";

/** The sign-in page, once browser detection has filled the list. */
async function renderSignIn() {
  await renderSettings("signin");
  await waitFor(() => expect(document.querySelector(".st-si-looking")).toBeNull());
}

const pageEl = () => document.querySelector(".st-pane .st-page") as HTMLElement;

/** The status card: its tone, its sentence and the detail under it. */
function card() {
  const el = document.querySelector(".st-pane .st-status") as HTMLElement;
  return {
    tone: [...el.classList].find((c) => c.startsWith("is-"))!.slice(3),
    title: el.querySelector(".st-status-title")!.textContent,
    detail: el.querySelector(".st-status-detail")?.textContent ?? "",
  };
}

const radio = (name: RegExp) => screen.getByRole("radio", { name }) as HTMLInputElement;
const row = (r: HTMLElement) => r.closest(".st-choice") as HTMLElement;
const pillOf = (r: HTMLElement) => row(r).querySelector(".st-pill")?.textContent ?? null;
/** The row's detail line: the first of the ids describing its radio (the
 *  state pill follows it). */
const detailOf = (r: HTMLElement) =>
  document.getElementById(r.getAttribute("aria-describedby")!.split(" ")[0])?.textContent;
const titles = () =>
  [...pageEl().querySelectorAll(".st-choice-title")].map((t) => t.textContent);

describe("YouTube sign-in", () => {
  it("lists Automatic, every browser found, a cookies file and None, in that order", async () => {
    await renderSignIn();
    expect(screen.getByRole("heading", { level: 2, name: "YouTube sign-in" })).toBeTruthy();
    expect(screen.getByRole("radiogroup", { name: "Sign-in comes from" })).toBeTruthy();
    expect(titles()).toEqual(["Automatic", "Chrome", "Firefox", "Cookies file…", "None"]);
    expect(radio(/^Automatic/).checked).toBe(true);
  });

  it("names the browser Automatic resolves to, whichever it is", async () => {
    await renderSignIn();
    expect(pillOf(radio(/^Automatic/))).toBe("Firefox");
    cleanup();
    backend.scan = {
      ...SCAN,
      browsers: [...BROWSERS, browserOpt({ id: "brave", label: "Brave", signedIn: true })],
      automatic: "brave",
    };
    await renderSignIn();
    expect(pillOf(radio(/^Automatic/))).toBe("Brave");
    expect(pillOf(radio(/^Brave/))).toBe("Signed in");
    expect(card()).toEqual({
      tone: "ok",
      title: "Downloads run signed in, as you",
      detail: "Through Brave, picked automatically: it is the browser signed in to YouTube.",
    });
  });

  it("does not call Automatic's pick the only one signed in when several are", async () => {
    backend.scan = {
      browsers: [
        browserOpt({ id: "brave", label: "Brave", signedIn: true }),
        browserOpt({ id: "firefox", label: "Firefox", signedIn: true }),
      ],
      automatic: "firefox",
      accessHint: null,
    };
    await renderSignIn();
    expect(card().detail).toBe(
      "Through Firefox, picked automatically: of the browsers signed in to YouTube, it was used last.",
    );
  });

  it("expects no browser in particular when Automatic has nothing to use", async () => {
    backend.scan = { browsers: [BROWSERS[0]], automatic: null, accessHint: null };
    await renderSignIn();
    expect(pillOf(radio(/^Automatic/))).toBe("none usable");
    expect(card().detail).toBe(`No browser here can be used, so ${SIGNED_OUT}`);
    cleanup();

    backend.scan = { browsers: [], automatic: null, accessHint: null };
    await renderSignIn();
    expect(pillOf(radio(/^Automatic/))).toBe("none found");
    expect(card()).toEqual({
      tone: "warn", title: "Downloads run signed out", detail: `No browser found, so ${SIGNED_OUT}`,
    });
    // Nothing on the page presumes a browser the machine does not have.
    expect(pageEl().textContent).not.toMatch(/Firefox|Chrome|Safari/);
  });

  it("says when the browser Automatic uses is not signed in to YouTube", async () => {
    await renderSignIn();
    expect(card()).toEqual({
      tone: "warn",
      title: "Downloads run signed out",
      detail: "Uses Firefox, the browser used last, but none MyTube can read is signed in to " +
        `YouTube, so ${SIGNED_OUT}`,
    });
    expect(pillOf(radio(/^Firefox/))).toBe("Not signed in");
  });

  it("says a browser chosen by hand is signed in, or is not", async () => {
    backend.settings = { ...SETTINGS, cookies_browser: "firefox" };
    backend.scan = {
      ...SCAN, browsers: [BROWSERS[0], browserOpt({ id: "firefox", label: "Firefox", signedIn: true })],
    };
    await renderSignIn();
    expect(radio(/^Firefox/).checked).toBe(true);
    expect(card()).toEqual({
      tone: "ok", title: "Downloads run signed in, as you",
      detail: "Through Firefox, which is signed in to YouTube.",
    });
    cleanup();

    backend.scan = SCAN;
    await renderSignIn();
    expect(card()).toEqual({
      tone: "warn", title: "Downloads run signed out",
      detail: `Firefox is not signed in to YouTube, so ${SIGNED_OUT}`,
    });
  });

  it("says nothing it cannot know about a browser it cannot look inside", async () => {
    backend.settings = { ...SETTINGS, cookies_browser: "firefox" };
    backend.scan = { ...SCAN, browsers: [browserOpt({ id: "firefox", label: "Firefox", signedIn: null })] };
    await renderSignIn();
    expect(pillOf(radio(/^Firefox/))).toBeNull();
    expect(card()).toEqual({
      tone: "neutral", title: "Downloads use Firefox's sign-in",
      detail: "Uses Firefox's YouTube sign-in; MyTube can't see whether it is signed in.",
    });
  });

  it("says downloads fail on a browser chosen by hand that cannot be read", async () => {
    backend.settings = { ...SETTINGS, cookies_browser: "chrome" };
    await renderSignIn();
    const chrome = radio(/^Chrome/);
    expect(chrome.checked).toBe(true);
    expect(card()).toEqual({
      tone: "error", title: "Chrome can't be used",
      detail: `${CHROME_NOTE} Downloads fail while it is chosen.`,
    });
    expect(railItem("YouTube sign-in").className).toContain("is-error");
  });

  it("disables a browser yt-dlp cannot read here and says why", async () => {
    await renderSignIn();
    const chrome = radio(/^Chrome/);
    expect(chrome.disabled).toBe(true);
    expect(pillOf(chrome)).toBe("Not supported");
    expect(detailOf(chrome)).toBe(CHROME_NOTE);
    expect(radio(/^Firefox/).disabled).toBe(false);
  });

  it("marks a browser macOS keeps MyTube out of as no access, and says how to allow it", async () => {
    const hintText = "Browser missing, or marked “no access”? … Full Disk Access …";
    backend.scan = {
      browsers: [browserOpt({
        id: "firefox", label: "Firefox", supported: false, blocked: true, signedIn: null,
        note: "macOS is not letting MyTube read it.",
      })],
      automatic: null,
      accessHint: hintText,
    };
    await renderSignIn();
    const firefox = radio(/^Firefox/);
    expect(firefox.disabled).toBe(true);
    expect(pillOf(firefox)).toBe("No access");
    expect(detailOf(firefox)).toBe("macOS is not letting MyTube read it.");
    expect(screen.getByText(hintText)).toBeDefined();
    // MyTube is not in the Full Disk Access list until added, so the way
    // there is one click.
    fireEvent.click(screen.getByRole("button", { name: "Open Full Disk Access settings" }));
    expect(api.openFullDiskAccess).toHaveBeenCalledTimes(1);
  });

  it("toasts a Full Disk Access pane that will not open", async () => {
    backend.scan = { ...SCAN, accessHint: "Browser missing?" };
    api.openFullDiskAccess.mockImplementation(() => Promise.reject("could not open System Settings"));
    await renderSignIn();
    fireEvent.click(screen.getByRole("button", { name: "Open Full Disk Access settings" }));
    expect(await screen.findByText("could not open System Settings")).toBeDefined();
  });

  it("offers Full Disk Access only where the backend gives an access hint", async () => {
    await renderSignIn();
    expect(screen.queryByRole("button", { name: "Open Full Disk Access settings" })).toBeNull();
  });

  it("shows the cookies file when one is set, since it wins, with what it holds", async () => {
    backend.settings = { ...SETTINGS, cookies_browser: "auto", cookies_file: "/home/someone/cookies.json" };
    await renderSignIn();
    const file = radio(/^Cookies file/);
    expect(file.checked).toBe(true);
    expect(radio(/^Automatic/).checked).toBe(false);
    expect(screen.getByText("/home/someone/cookies.json")).toBeDefined();
    expect(api.inspectCookiesFile).toHaveBeenCalledWith("/home/someone/cookies.json");
    await waitFor(() => expect(row(file).querySelector(".st-si-file-hint")?.textContent).toBe(
      "JSON cookie export, 24 cookies, signed in to YouTube. It is used instead of any browser.",
    ));
    expect(pillOf(file)).toBe("Signed in");
    expect(card()).toEqual({
      tone: "ok", title: "Downloads run signed in, as you",
      detail: "Through the cookies file below, used instead of any browser.",
    });
  });

  it.each([
    [
      "holds cookies for YouTube but no sign-in",
      { youtube: true, skipped: 0 },
      `JSON cookie export, 24 cookies, but no YouTube sign-in among them, so ${SIGNED_OUT}`,
      `The cookies file holds no YouTube sign-in, so ${SIGNED_OUT}`,
    ],
    [
      "holds nothing for YouTube",
      { youtube: false, skipped: 1 },
      "JSON cookie export, 24 cookies (1 unreadable entry skipped), none of them for YouTube, " +
        `so ${SIGNED_OUT}`,
      `None of the cookies file's cookies are for YouTube, so ${SIGNED_OUT}`,
    ],
  ])("says downloads run signed out when the file %s", async (_, over, hint, detail) => {
    api.inspectCookiesFile.mockImplementation(
      () => Promise.resolve({ ...FILE_INFO, signedIn: false, ...over }),
    );
    backend.settings = { ...SETTINGS, cookies_file: "/home/someone/cookies.txt" };
    await renderSignIn();
    const file = radio(/^Cookies file/);
    await waitFor(() => expect(row(file).querySelector(".st-si-file-hint")?.textContent).toBe(hint));
    expect(pillOf(file)).toBe("Not signed in");
    expect(card()).toEqual({ tone: "warn", title: "Downloads run signed out", detail });
  });

  it("says why a cookies file already set can no longer be used", async () => {
    const why = "The cookies file /home/someone/cookies.txt no longer exists.";
    api.inspectCookiesFile.mockImplementation(() => Promise.reject(why));
    backend.settings = { ...SETTINGS, cookies_browser: "auto", cookies_file: "/home/someone/cookies.txt" };
    await renderSignIn();
    await waitFor(() => expect(card()).toEqual({
      tone: "error", title: "The cookies file can't be used", detail: why,
    }));
    // Said once, in the card; the row keeps the path and the way to fix it.
    expect(screen.getAllByText(why)).toHaveLength(1);
    expect(pillOf(radio(/^Cookies file/))).toBe("Can't be read");
    expect(screen.getByRole("button", { name: "Change…" })).toBeTruthy();
    expect(railItem("YouTube sign-in").className).toContain("is-error");
  });

  it("says it is still reading a cookies file", async () => {
    api.inspectCookiesFile.mockImplementation(() => new Promise(() => {}));
    backend.settings = { ...SETTINGS, cookies_file: "/home/someone/cookies.txt" };
    await renderSignIn();
    expect(card()).toEqual({
      tone: "neutral", title: "Reading the cookies file…",
      detail: "A cookies file exported from a browser signed in to YouTube. It is used instead " +
        "of any browser.",
    });
  });

  it("says it is still looking while browser detection runs", async () => {
    api.detectBrowsers.mockImplementation(() => new Promise(() => {}));
    await renderSettings("signin");
    expect(screen.getByText("Looking for browsers…", { selector: ".st-si-looking" })).toBeTruthy();
    expect(card()).toEqual({
      tone: "neutral", title: "Looking for browsers…",
      detail: "Uses the browser you are signed in to YouTube with.",
    });
    expect(pillOf(radio(/^Automatic/))).toBeNull();
  });

  it("clears the cookies file when a browser is chosen, in one save", async () => {
    backend.settings = { ...SETTINGS, cookies_browser: "auto", cookies_file: "/home/someone/cookies.txt" };
    await renderSignIn();
    fireEvent.click(radio(/^Firefox/));
    await waitFor(() => expect(api.saveSettings).toHaveBeenCalledTimes(1));
    expect(lastSaved().cookies_browser).toBe("firefox");
    expect(lastSaved().cookies_file).toBe("");
    expect(radio(/^Firefox/).checked).toBe(true);
    expect(screen.queryByText("/home/someone/cookies.txt")).toBeNull();
  });

  it("stores None as an empty browser, and says downloads run signed out", async () => {
    await renderSignIn();
    fireEvent.click(radio(/^None/));
    await waitFor(() => expect(api.saveSettings).toHaveBeenCalledTimes(1));
    expect(lastSaved().cookies_browser).toBe("");
    expect(lastSaved().cookies_file).toBe("");
    expect(card()).toEqual({
      tone: "warn", title: "Downloads run signed out", detail: `No sign-in is used, so ${SIGNED_OUT}`,
    });
  });

  it("picks a cookies file of any kind through the file dialog, checked first", async () => {
    dialog.open.mockResolvedValueOnce("/home/someone/yt-cookies.json");
    await renderSignIn();
    fireEvent.click(radio(/^Cookies file/));
    await waitFor(() => expect(api.saveSettings).toHaveBeenCalledTimes(1));
    expect(dialog.open.mock.calls[0][0]).toEqual({
      multiple: false, directory: false, title: "Choose a cookies file",
    });
    const opts = dialog.open.mock.calls[0][0] as { filters?: unknown };
    expect(opts.filters).toBeUndefined();
    expect(api.inspectCookiesFile).toHaveBeenCalledWith("/home/someone/yt-cookies.json");
    expect(lastSaved().cookies_file).toBe("/home/someone/yt-cookies.json");
    // The browser underneath is left alone; the file simply wins over it.
    expect(lastSaved().cookies_browser).toBe("auto");
    await waitFor(() => expect(radio(/^Cookies file/).checked).toBe(true));
    expect(screen.getByText("/home/someone/yt-cookies.json")).toBeDefined();
    expect(card().title).toBe("Downloads run signed in, as you");
  });

  it("holds the list still while the dialog is open", async () => {
    let answer!: (path: string | null) => void;
    dialog.open.mockImplementationOnce(() => new Promise((r) => { answer = r; }));
    await renderSignIn();
    fireEvent.click(radio(/^Cookies file/));
    await waitFor(() => expect(radio(/^Automatic/).disabled).toBe(true));
    expect(radio(/^None/).disabled).toBe(true);
    answer(null);
    await waitFor(() => expect(radio(/^Automatic/).disabled).toBe(false));
    expect(radio(/^Automatic/).checked).toBe(true);
  });

  it("chooses another file with Change…", async () => {
    backend.settings = { ...SETTINGS, cookies_file: "/home/someone/cookies.txt" };
    dialog.open.mockResolvedValueOnce("/home/someone/other.txt");
    await renderSignIn();
    fireEvent.click(screen.getByRole("button", { name: "Change…" }));
    await waitFor(() => expect(api.saveSettings).toHaveBeenCalledTimes(1));
    expect(lastSaved().cookies_file).toBe("/home/someone/other.txt");
    expect(await screen.findByText("/home/someone/other.txt")).toBeDefined();
  });

  it("refuses a file it cannot read, says why, and saves nothing", async () => {
    const why = "notes.txt can't be used as a cookies file: it is not a Netscape cookies.txt, " +
      "a JSON cookie export, or a list of name=value pairs.";
    api.inspectCookiesFile.mockImplementation(() => Promise.reject(why));
    dialog.open.mockResolvedValueOnce("/home/someone/notes.txt");
    await renderSignIn();
    fireEvent.click(radio(/^Cookies file/));
    expect(await screen.findByText(why)).toBeDefined();
    await waitFor(() => expect(radio(/^Automatic/).disabled).toBe(false));
    expect(api.saveSettings).not.toHaveBeenCalled();
    expect(radio(/^Automatic/).checked).toBe(true);
    expect(radio(/^Cookies file/).checked).toBe(false);
  });

  it("changes nothing when the file dialog is dismissed", async () => {
    await renderSignIn();
    fireEvent.click(radio(/^Cookies file/));
    await waitFor(() => expect(dialog.open).toHaveBeenCalled());
    await waitFor(() => expect(radio(/^Automatic/).disabled).toBe(false));
    expect(api.saveSettings).not.toHaveBeenCalled();
    expect(api.inspectCookiesFile).not.toHaveBeenCalled();
    expect(radio(/^Automatic/).checked).toBe(true);
    expect(radio(/^Cookies file/).checked).toBe(false);
  });

  /** A hand-written spec like `chrome:Profile 1` is legal and must not be
   *  shown as something else, or silently rewritten on the next save. */
  it("keeps a hand-written browser spec on screen as itself", async () => {
    backend.settings = { ...SETTINGS, cookies_browser: "firefox:work" };
    await renderSignIn();
    const spec = radio(/^firefox:work/);
    expect(spec.checked).toBe(true);
    expect(radio(/^Firefox/).checked).toBe(false);
    expect(titles()).toEqual(["Automatic", "Chrome", "Firefox", "firefox:work", "Cookies file…", "None"]);
    expect(card()).toEqual({
      tone: "neutral", title: "Downloads use the browser you named",
      detail: "Uses firefox:work's YouTube sign-in, passed to yt-dlp as written.",
    });
    expect(api.saveSettings).not.toHaveBeenCalled();
  });
});

describe("the sign-in rail line", () => {
  const ctx = (s: Partial<Settings>, browsers: BrowserScan | null = SCAN, cookieFile: CookieFileState | null = null) =>
    ({ settings: { ...SETTINGS, ...s }, browsers, cookieFile }) as unknown as SettingsCtx;
  const signedIn = (id: string) => ({
    ...SCAN, browsers: SCAN.browsers.map((b) => (b.id === id ? { ...b, signedIn: true } : b)),
  });

  it("names the browser in use and whether it is signed in", () => {
    expect(summary(ctx({}))).toEqual({ text: "Firefox · not signed in" });
    expect(summary(ctx({}, signedIn("firefox")))).toEqual({ text: "Firefox · signed in", tone: "ok" });
    expect(summary(ctx({ cookies_browser: "firefox" }, signedIn("firefox"))))
      .toEqual({ text: "Firefox · signed in", tone: "ok" });
    expect(summary(ctx({}, null))).toEqual({ text: "Automatic" });
  });

  it("says signed out for None, and for Automatic with nothing to use", () => {
    expect(summary(ctx({ cookies_browser: "" }))).toEqual({ text: "Signed out" });
    expect(summary(ctx({}, { browsers: [], automatic: null, accessHint: null })))
      .toEqual({ text: "Signed out" });
  });

  it("marks a chosen browser that cannot be read, and shows a hand-written spec as itself", () => {
    expect(summary(ctx({ cookies_browser: "chrome" }))).toMatchObject({
      text: "Chrome · not supported", tone: "error", flag: { text: "!", tone: "error" },
    });
    const blocked = { ...SCAN, browsers: [{ ...BROWSERS[1], supported: false, blocked: true }] };
    expect(summary(ctx({ cookies_browser: "firefox" }, blocked)))
      .toMatchObject({ text: "Firefox · no access", tone: "error" });
    expect(summary(ctx({ cookies_browser: "chrome:Profile 1" }))).toEqual({ text: "chrome:Profile 1" });
  });

  it("follows the cookies file, which wins over any browser", () => {
    const file = { cookies_file: "/c.txt" };
    expect(summary(ctx(file))).toEqual({ text: "Cookies file" });
    expect(summary(ctx(file, SCAN, { path: "/c.txt", info: FILE_INFO })))
      .toEqual({ text: "Cookies file · signed in", tone: "ok" });
    expect(summary(ctx(file, SCAN, { path: "/c.txt", info: { ...FILE_INFO, signedIn: false } })))
      .toEqual({ text: "Cookies file · not signed in" });
    expect(summary(ctx(file, SCAN, { path: "/c.txt", error: "gone" }))).toMatchObject({
      text: "Cookies file can't be read", tone: "error", flag: { text: "!", tone: "error" },
    });
  });
});
