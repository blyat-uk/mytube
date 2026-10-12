import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { screen, fireEvent, cleanup, waitFor, within, act } from "@testing-library/react";

vi.mock("@tauri-apps/api/event", async () => (await import("./testApi")).eventModule);
vi.mock("@tauri-apps/plugin-dialog", async () => (await import("./testApi")).dialogModule);
vi.mock("../../api", async () => (await import("./testApi")).apiModule);

import { api, backend, emit, lastSaved, resetHarness, SETTINGS } from "./testApi";
import { railItem, renderSettings } from "./testRender";
import { aboutStatus, summary } from "./AboutPage";
import type { SettingsCtx } from "./context";
import type { VersionInfo } from "../../types";

const UP_TO_DATE: VersionInfo = {
  current: "2.1.0", update: null, checkedAt: 1_790_000_000, checksEnabled: true,
};
const RELEASE = "https://github.com/blyat-uk/mytube/releases/tag/v2.2.0";
const BEHIND: VersionInfo = { ...UP_TO_DATE, update: { version: "2.2.0", url: RELEASE } };

/* The harness types this mock by its default answer, whose `checkedAt` is a
   number; a real check can answer any `VersionInfo`. */
const checkAnswers = (fn: () => Promise<VersionInfo>) =>
  api.checkAppUpdateNow.mockImplementation(fn as never);

beforeEach(() => {
  resetHarness();
  backend.version = UP_TO_DATE;
  checkAnswers(() => Promise.resolve(UP_TO_DATE));
});
afterEach(cleanup);

const page = () => screen.getByRole("region", { name: "About" });

/** The About page, once the version has been read. With a newer release the
 *  running version shows twice, in the hero and in the comparison row. */
async function renderAbout() {
  await renderSettings("about");
  await within(page()).findAllByText("2.1.0");
  return page();
}

const checkNow = () => screen.getByRole("button", { name: "Check now" }) as HTMLButtonElement;
const releasePage = () => screen.getByRole("button", { name: "Open release page…" });
/** The Installed → Available row; null while there is nothing newer to name. */
const comparison = () => page().querySelector<HTMLElement>(".st-ab-compare");
const releaseSwitch = () =>
  screen.getByRole("switch", { name: "Check GitHub for new releases" }) as HTMLInputElement;

describe("aboutStatus", () => {
  const now = 1_790_000_000 + 2 * 3600;

  it("says nothing before the first read", () => {
    expect(aboutStatus(null)).toBeNull();
  });

  it("names a newer release, and how to get it", () => {
    expect(aboutStatus(BEHIND, now)).toEqual({
      tone: "news",
      title: "MyTube 2.2.0 is available",
      detail: "MyTube never updates itself: get it the same way you installed this one.",
    });
  });

  it("says when it last looked", () => {
    expect(aboutStatus(UP_TO_DATE, now)).toEqual({ tone: "ok", title: "Up to date", detail: "Checked 2h ago" });
    expect(aboutStatus({ ...UP_TO_DATE, checkedAt: null }, now))
      .toEqual({ tone: "neutral", title: "Not checked yet." });
  });

  it("says checks are off rather than claiming anything is current", () => {
    expect(aboutStatus({ ...UP_TO_DATE, checksEnabled: false }, now))
      .toEqual({ tone: "neutral", title: "Update checks are off." });
    expect(aboutStatus({ ...BEHIND, checksEnabled: false }, now)?.title).toBe("Update checks are off.");
  });
});

describe("the About page", () => {
  it("heads the page with a real heading, while the hero is what is drawn", async () => {
    await renderAbout();
    expect(screen.getByRole("heading", { level: 2, name: "About" })).toBeDefined();
    const hero = page().querySelector(".st-ab-hero")!;
    expect(hero.textContent).toMatch(/^MyTube 2\.1\.0/);
    expect(hero.textContent).toMatch(/Nothing is streamed and nothing is written back to YouTube\./);
  });

  it("shows the running version and links to its release notes", async () => {
    await renderAbout();
    expect(within(page()).getByText("2.1.0").className).toBe("st-ab-version");
    fireEvent.click(screen.getByRole("button", { name: "Release notes for 2.1.0…" }));
    expect(api.openExternal).toHaveBeenCalledWith(
      "https://github.com/blyat-uk/mytube/releases/tag/v2.1.0",
    );
  });

  it("links to the project on GitHub", async () => {
    await renderAbout();
    fireEvent.click(screen.getByRole("button", { name: "GitHub…" }));
    expect(api.openExternal).toHaveBeenCalledWith("https://github.com/blyat-uk/mytube");
  });

  it("toasts a link that cannot be opened", async () => {
    api.openExternal.mockImplementation(() => Promise.reject("no browser"));
    await renderAbout();
    fireEvent.click(screen.getByRole("button", { name: "GitHub…" }));
    expect(await screen.findByText("no browser")).toBeDefined();
  });

  it("says it is up to date, and when it looked", async () => {
    await renderAbout();
    const status = page().querySelector(".st-status")!;
    expect(status.className).toContain("is-ok");
    expect(status.textContent).toMatch(/^Up to dateChecked .+ ago$/);
  });

  it("offers a newer release's page", async () => {
    backend.version = BEHIND;
    await renderAbout();
    const status = page().querySelector(".st-status")!;
    expect(status.className).toContain("is-news");
    expect(status.textContent).toMatch(/MyTube 2\.2\.0 is available/);
    // The comparison comes first, and the sentence about how to update stays.
    expect(status.querySelector(".st-status-detail")!.textContent).toBe(
      "Installed2.1.0Available2.2.0MyTube never updates itself: get it the same way you installed this one.",
    );
    const open = releasePage();
    expect(open.className).toContain("btn-primary");
    fireEvent.click(open);
    expect(api.openExternal).toHaveBeenCalledWith(RELEASE);
  });

  it("sets the installed version beside the newer one", async () => {
    backend.version = BEHIND;
    await renderAbout();
    const row = comparison()!;
    expect(row.closest(".st-status")?.className).toContain("is-news");
    // A term and its value each, so a screen reader hears "Installed 2.1.0",
    // not two bare numbers.
    expect(within(row).getAllByRole("term").map((t) => t.textContent)).toEqual(["Installed", "Available"]);
    const values = within(row).getAllByRole("definition");
    expect(values.map((v) => v.textContent)).toEqual(["2.1.0", "2.2.0"]);
    expect(values.every((v) => v.className.includes("st-ab-compare-value"))).toBe(true);
    expect(values[1].className).toContain("is-new");
  });

  it("has nothing to compare while up to date", async () => {
    await renderAbout();
    expect(comparison()).toBeNull();
    expect(screen.queryByRole("button", { name: "Open release page…" })).toBeNull();
  });

  it("has nothing to compare while checks are off, even with a newer release stored", async () => {
    backend.settings = { ...SETTINGS, check_app_updates: false };
    backend.version = { ...BEHIND, checksEnabled: false };
    await renderAbout();
    expect(within(page()).getByText("Update checks are off.")).toBeDefined();
    expect(comparison()).toBeNull();
    expect(screen.queryByRole("button", { name: "Open release page…" })).toBeNull();
  });

  it("marks the rail's About entry with a green ↑ pill, named for a screen reader", async () => {
    backend.version = BEHIND;
    await renderAbout();
    const item = railItem("About");
    const flag = item.querySelector(".st-flag")!;
    expect(flag.className).toContain("is-news");
    expect(flag.textContent).toBe("↑ 2.2.0");
    expect(flag.getAttribute("aria-hidden")).toBe("true");
    expect(item.querySelector(".st-marker.is-news")).not.toBeNull();
    expect(document.getElementById(item.getAttribute("aria-describedby")!)!.textContent)
      .toBe("MyTube 2.1.0. MyTube 2.2.0 is available");
  });

  it("says when it has never looked", async () => {
    backend.version = { ...UP_TO_DATE, checkedAt: null };
    await renderAbout();
    expect(within(page()).getByText("Not checked yet.")).toBeDefined();
  });

  it("checks now, and says so when nothing is newer", async () => {
    await renderAbout();
    fireEvent.click(checkNow());
    expect(await screen.findByText("MyTube 2.1.0 is up to date.")).toBeDefined();
    expect(api.checkAppUpdateNow).toHaveBeenCalledTimes(1);
  });

  it("shows what a check found without a toast", async () => {
    checkAnswers(() => Promise.resolve(BEHIND));
    await renderAbout();
    fireEvent.click(checkNow());
    expect(await screen.findByRole("button", { name: "Open release page…" })).toBeDefined();
    expect(comparison()?.textContent).toMatch(/Installed.*2\.1\.0.*Available.*2\.2\.0/);
    expect(screen.queryByText(/is up to date/)).toBeNull();
    // The rail follows the same answer: one copy of the version, shared.
    expect(screen.getByRole("button", { name: "About" }).textContent).toMatch(/2\.2\.0/);
  });

  /** The backend answers a check made while switched off with nothing; that
   *  is not "up to date" either. */
  it("does not call MyTube up to date when the check answered that checks are off", async () => {
    checkAnswers(() => Promise.resolve({ ...UP_TO_DATE, checkedAt: null, checksEnabled: false }));
    await renderAbout();
    fireEvent.click(checkNow());
    expect(await within(page()).findByText("Update checks are off.")).toBeDefined();
    expect(screen.queryByText(/is up to date/)).toBeNull();
  });

  it("sends one check for a double click, and says Checking… once it is slow", async () => {
    let resolve!: (v: VersionInfo) => void;
    checkAnswers(() => new Promise((r) => { resolve = r; }));
    await renderAbout();
    const btn = checkNow();
    fireEvent.click(btn);
    fireEvent.click(btn);
    expect(api.checkAppUpdateNow).toHaveBeenCalledTimes(1);
    expect(btn.disabled).toBe(true);
    expect(await screen.findByRole("button", { name: "Checking…" })).toBe(btn);
    await act(async () => { resolve(UP_TO_DATE); });
    expect(await screen.findByText("MyTube 2.1.0 is up to date.")).toBeDefined();
    expect(btn.textContent).toBe("Check now");
  });

  it("toasts a failed check", async () => {
    checkAnswers(() => Promise.reject("GitHub answered 403 Forbidden"));
    await renderAbout();
    fireEvent.click(checkNow());
    expect(await screen.findByText("GitHub answered 403 Forbidden")).toBeDefined();
  });

  it("cannot check while checks are off, and follows the switch flipping on", async () => {
    backend.settings = { ...SETTINGS, check_app_updates: false };
    backend.version = { ...UP_TO_DATE, checksEnabled: false };
    await renderAbout();
    expect(checkNow().disabled).toBe(true);
    expect(checkNow().title).toMatch(/Turn the switch on/);
    expect(within(page()).getByText("Update checks are off.")).toBeDefined();
    const status = page().querySelector(".st-status")!;
    expect(status.className).toContain("is-neutral");

    fireEvent.click(releaseSwitch());
    await waitFor(() => expect(api.saveSettings).toHaveBeenCalledTimes(1));
    expect(lastSaved().check_app_updates).toBe(true);
    // Beside the switch, Check now follows it at once; the card waits for
    // the backend's word, which saving the setting makes it send.
    expect(checkNow().disabled).toBe(false);
    expect(checkNow().title).toBe("");
    await emit("app://version-info", UP_TO_DATE);
    expect(within(page()).getByText("Up to date")).toBeDefined();
  });

  it("follows app://version-info", async () => {
    await renderAbout();
    await emit("app://version-info", BEHIND);
    expect(releasePage()).toBeDefined();
    expect(comparison()).not.toBeNull();
    await emit("app://version-info", { ...BEHIND, checksEnabled: false });
    expect(screen.queryByRole("button", { name: "Open release page…" })).toBeNull();
    expect(comparison()).toBeNull();
    expect(within(page()).getByText("Update checks are off.")).toBeDefined();
  });
});

describe("checking for MyTube releases", () => {
  it("is on by default and saves when switched off", async () => {
    await renderAbout();
    const check = releaseSwitch();
    expect(check.checked).toBe(true);
    expect(document.getElementById(check.getAttribute("aria-describedby")!)?.textContent)
      .toBe("Once a day. MyTube only tells you; it never updates itself.");
    fireEvent.click(check);
    await waitFor(() => expect(api.saveSettings).toHaveBeenCalledTimes(1));
    expect(lastSaved().check_app_updates).toBe(false);
    expect(checkNow().disabled).toBe(true);
  });
});

describe("summary", () => {
  const ctx = (version: VersionInfo | null) => ({ version }) as unknown as SettingsCtx;

  it("is the bare name before the version is known", () => {
    expect(summary(ctx(null))).toEqual({ text: "MyTube" });
  });

  it("names the running version", () => {
    expect(summary(ctx(UP_TO_DATE))).toEqual({ text: "MyTube 2.1.0" });
  });

  it("flags a newer release with the update signal, ↑ and the version", () => {
    expect(summary(ctx(BEHIND))).toEqual({
      text: "MyTube 2.1.0",
      flag: { text: "↑ 2.2.0", tone: "news", label: "MyTube 2.2.0 is available" },
    });
  });

  it("keeps quiet about a newer release while checks are off", () => {
    expect(summary(ctx({ ...BEHIND, checksEnabled: false }))).toEqual({ text: "MyTube 2.1.0" });
  });
});
