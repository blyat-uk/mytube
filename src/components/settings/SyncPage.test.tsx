import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { screen, fireEvent, cleanup, waitFor, act, within } from "@testing-library/react";

vi.mock("@tauri-apps/api/event", async () => (await import("./testApi")).eventModule);
vi.mock("@tauri-apps/plugin-dialog", async () => (await import("./testApi")).dialogModule);
vi.mock("../../api", async () => (await import("./testApi")).apiModule);

import { api, backend, emit, resetHarness, SYNC_OFF } from "./testApi";
import { railItem, renderSettings } from "./testRender";
import {
  EXPIRED_TEXT, MAX_COMPUTERS, SYNCED_CHANGES_TEXT, SYNCED_TEXT, maskCode, peersLine, statusLine,
  summary,
} from "./SyncPage";
import type { SettingsCtx } from "./context";
import type { SyncStatus } from "../../types";

const CODE = "MYT1-7ABCD-EFGHJ-KMNPQ-RSTVW-XYZ012";
const MASKED = "MYT1-•••••-•••••-•••••-•••••-••••••";
const NOW = 1_790_000_000;

const OFF = SYNC_OFF;
// Relative to the real clock, since the page formats against it.
const real = () => Math.floor(Date.now() / 1000);
const on = (over: Partial<SyncStatus> = {}): SyncStatus => ({
  enabled: true, code: CODE, machineName: "desktop",
  peers: [{ name: "laptop", lastWriteAt: real() - 180 }],
  lastSyncAt: real() - 120, pending: false, error: null, expired: false,
  ...over,
});

const renderSync = () => renderSettings("sync");

const button = (name: string | RegExp) => screen.getByRole("button", { name }) as HTMLButtonElement;
const codeField = () => screen.getByLabelText("Sync code") as HTMLInputElement;
const joinField = () => screen.getByLabelText("Have a code from another computer?") as HTMLInputElement;

/** The page's status card: what is true right now. The rail repeats some of
 *  its words ("Last sync failed"), so tests read it here rather than by text. */
const card = () => document.querySelector<HTMLElement>(".st-pane .st-status");
const cardTitle = () => card()?.querySelector(".st-status-title")?.textContent;
const cardSays = (title: string) => waitFor(() => expect(cardTitle()).toBe(title));

/** The computers, in the order the page draws them. */
const computers = () =>
  within(screen.getByRole("list", { name: "Computers" })).getAllByRole("listitem");

const lineOf = (label: string) =>
  document.getElementById(railItem(label).getAttribute("aria-describedby")!)!.textContent;

/** A call that stays out until the test says otherwise. */
function hold<T>(mock: { mockImplementation: (fn: () => Promise<T>) => unknown }) {
  let resolve!: (v: T) => void;
  let reject!: (e: unknown) => void;
  mock.mockImplementation(() => new Promise<T>((res, rej) => { resolve = res; reject = rej; }));
  return {
    resolve: (v: T) => act(async () => { resolve(v); }),
    reject: (e: unknown) => act(async () => { reject(e); }),
  };
}

const writeText = vi.fn<(text: string) => Promise<void>>(() => Promise.resolve());
function setClipboard(value: unknown) {
  Object.defineProperty(navigator, "clipboard", { value, configurable: true, writable: true });
}

beforeEach(resetHarness);
beforeEach(() => {
  // The old section's defaults: every way on lands on a fresh code.
  api.syncTurnOn.mockImplementation(() => Promise.resolve(on()));
  api.syncJoin.mockImplementation(() => Promise.resolve(on()));
  writeText.mockReset();
  writeText.mockImplementation(() => Promise.resolve());
  setClipboard({ writeText });
});
afterEach(cleanup);

describe("the pure helpers", () => {
  it("masks everything after the version prefix, keeping the code's shape", () => {
    expect(maskCode(CODE)).toBe(MASKED);
    expect(maskCode(CODE)).toHaveLength(CODE.length);
    expect(maskCode("ABCDE")).toBe("•••••");
  });

  it("names the other computers and how recently each synced", () => {
    expect(peersLine([], NOW)).toBe("No other computer has joined yet.");
    expect(peersLine([
      { name: "laptop", lastWriteAt: NOW - 180 },
      { name: "work", lastWriteAt: null },
    ], NOW)).toBe("Also syncing: laptop · 3m ago, work");
  });

  it("puts an error first, then waiting changes, then the last sync", () => {
    const s = on({ lastSyncAt: NOW - 7200 });
    expect(statusLine(s, NOW)).toEqual({ text: "Synced 2h ago", warn: false });
    expect(statusLine({ ...s, lastSyncAt: null }, NOW).text).toBe("Not synced yet");
    expect(statusLine({ ...s, pending: true }, NOW).text)
      .toBe("Changes will be sent within a minute.");
    expect(statusLine({ ...s, pending: true, error: "Can't reach the sync server." }, NOW))
      .toEqual({ text: "Can't reach the sync server.", warn: true });
  });
});

describe("the rail line", () => {
  const ctx = (status: SyncStatus | null, loadError: string | null = null) =>
    ({ sync: { status, loadError, setStatus: () => {} } }) as unknown as SettingsCtx;

  it("says off, on and who with, or that nobody has joined", () => {
    expect(summary(ctx(OFF))).toEqual({ text: "Off" });
    expect(summary(ctx(on({ peers: [] })))).toEqual({ text: "On · no other computer yet" });
    expect(summary(ctx(on({
      peers: [{ name: "laptop", lastWriteAt: null }, { name: "work", lastWriteAt: NOW }],
    })))).toEqual({ text: "On · laptop, work" });
    // Waiting changes are business as usual, not something to look at.
    expect(summary(ctx(on({ pending: true }))).tone).toBeUndefined();
  });

  it("marks an error and an expired code as needing the user", () => {
    const failed = summary(ctx(on({ error: "Can't reach the sync server." })));
    expect(failed.text).toBe("Last sync failed");
    expect(failed.tone).toBe("error");
    expect(failed.flag?.tone).toBe("error");
    const expired = summary(ctx(on({ expired: true })));
    expect(expired.text).toBe("Sync code expired");
    expect(expired.tone).toBe("error");
  });

  it("is quiet while the first read is out, and says when it failed", () => {
    expect(summary(ctx(null))).toEqual({ text: "Checking…" });
    expect(summary(ctx(null, "command sync_status not found"))).toEqual({ text: "Status unavailable" });
  });

  it("marks the rail entry once the last sync fails, and clears it once one works", async () => {
    backend.sync = on();
    await renderSync();
    await waitFor(() => expect(lineOf("Sync")).toBe("On · laptop"));
    expect(railItem("Sync").className).not.toContain("is-error");
    await emit("sync://status", on({ error: "Can't reach the sync server. MyTube will keep trying." }));
    expect(railItem("Sync").className).toContain("is-error");
    expect(lineOf("Sync")).toMatch(/^Last sync failed/);
    await emit("sync://status", on());
    expect(railItem("Sync").className).not.toContain("is-error");
  });
});

describe("while sync is off", () => {
  it("explains sync and offers to turn it on or join", async () => {
    await renderSync();
    expect(await screen.findByRole("button", { name: "Turn on sync" })).toBeDefined();
    expect(screen.getByRole("heading", { level: 2, name: "Sync" })).toBeDefined();
    expect(screen.getByRole("heading", { name: "Keep your computers in step" })).toBeDefined();
    expect(screen.getByText(/Everything is encrypted on this computer first/)).toBeDefined();
    const field = joinField();
    expect(field.placeholder).toBe("MYT1-…");
    expect(field.getAttribute("spellcheck")).toBe("false");
    expect(field.getAttribute("autocomplete")).toBe("off");
    expect(field.className).toMatch(/\bmono\b/);
    expect(screen.queryByLabelText("Sync code")).toBeNull();
    // Nothing to report yet, so no status card and no computers.
    expect(card()).toBeNull();
    expect(screen.queryByRole("list", { name: "Computers" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Sync now" })).toBeNull();
  });

  it("says what would travel before anything is turned on", async () => {
    await renderSync();
    await screen.findByRole("button", { name: "Turn on sync" });
    expect(screen.getByRole("list", { name: "Travels between computers" })).toBeDefined();
    expect(screen.getByRole("list", { name: "Stays on this computer" })).toBeDefined();
  });

  it("turns sync on, saying where to finish while it waits", async () => {
    const call = hold(api.syncTurnOn);
    await renderSync();
    fireEvent.click(await screen.findByRole("button", { name: "Turn on sync" }));
    expect(api.syncTurnOn).toHaveBeenCalledTimes(1);
    expect(await screen.findByText("Finish the check in the window that opened.")).toBeDefined();
    // One verification at a time, and no join racing it.
    expect(button("Waiting…").disabled).toBe(true);
    fireEvent.change(joinField(), { target: { value: CODE } });
    expect(button("Join").disabled).toBe(true);

    await call.resolve(on());
    expect(codeField().value).toBe(MASKED);
    expect(screen.queryByText("Finish the check in the window that opened.")).toBeNull();
    // The rail heard about it too: the page hands every status to the shell.
    expect(lineOf("Sync")).toBe("On · laptop");
  });

  it("toasts a cancelled verification and stays off", async () => {
    api.syncTurnOn.mockImplementation(() => Promise.reject("Verification was cancelled."));
    await renderSync();
    fireEvent.click(await screen.findByRole("button", { name: "Turn on sync" }));
    expect(await screen.findByText("Verification was cancelled.")).toBeDefined();
    expect(button("Turn on sync").disabled).toBe(false);
  });

  it("keeps Join disabled until there is a code, and trims what it sends", async () => {
    await renderSync();
    await screen.findByRole("button", { name: "Turn on sync" });
    expect(button("Join").disabled).toBe(true);
    fireEvent.change(joinField(), { target: { value: "   " } });
    expect(button("Join").disabled).toBe(true);

    fireEvent.change(joinField(), { target: { value: `  ${CODE}\n` } });
    fireEvent.click(button("Join"));
    expect(api.syncJoin).toHaveBeenCalledWith(CODE);
    expect(await screen.findByLabelText("Sync code")).toBeDefined();
  });

  it("joins on Enter too", async () => {
    await renderSync();
    await screen.findByRole("button", { name: "Turn on sync" });
    fireEvent.change(joinField(), { target: { value: CODE } });
    fireEvent.submit(joinField().form!);
    expect(api.syncJoin).toHaveBeenCalledWith(CODE);
    await screen.findByLabelText("Sync code");
  });

  it("shows a refused code under the field, once, until it is edited", async () => {
    const reason = "No sync group uses this code. Check it and try again.";
    api.syncJoin.mockImplementation(() => Promise.reject(reason));
    await renderSync();
    await screen.findByRole("button", { name: "Turn on sync" });
    fireEvent.change(joinField(), { target: { value: "MYT1-WRONG" } });
    fireEvent.click(button("Join"));

    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toBe(reason);
    // Inline instead of a toast, not as well as one.
    expect(screen.getAllByText(reason)).toHaveLength(1);
    expect(joinField().getAttribute("aria-invalid")).toBe("true");
    expect(joinField().getAttribute("aria-describedby")).toBe(alert.id);
    // Still off, and the code is still there to correct.
    expect(joinField().value).toBe("MYT1-WRONG");

    fireEvent.change(joinField(), { target: { value: "MYT1-RIGHT" } });
    expect(screen.queryByRole("alert")).toBeNull();
  });
});

describe("while sync is on", () => {
  beforeEach(() => { backend.sync = on(); });

  it("shows the masked code, the computers and the last sync", async () => {
    await renderSync();
    const field = await screen.findByLabelText("Sync code") as HTMLInputElement;
    expect(field.value).toBe(MASKED);
    expect(field.readOnly).toBe(true);
    // The warning and how to use the code are the field's description.
    const described = field.getAttribute("aria-describedby")!.split(" ")
      .map((id) => document.getElementById(id)!.textContent).join(" ");
    expect(described).toMatch(/Enter this code on your other computers/);
    expect(described).toMatch(/Anyone with this code can see and change your synced library\. Treat it like a password\./);

    const [me, laptop] = computers();
    expect(me.textContent).toBe("desktopThis oneSynced 2m ago");
    expect(laptop.textContent).toBe("laptopLast change 3m ago");

    await cardSays("Synced 2m ago");
    expect(card()!.className).toMatch(/\bis-ok\b/);
    expect(card()!.textContent).toMatch(/Everything is up to date/);
    expect(card()!.querySelector('[role="status"]')!.getAttribute("aria-live")).toBe("polite");
    // The explanation is for deciding; once on, the code's own note says what matters.
    expect(screen.queryByText(/no account is needed/)).toBeNull();
    expect(screen.queryByRole("button", { name: "Turn on sync" })).toBeNull();
    expect(screen.queryByLabelText("Have a code from another computer?")).toBeNull();
  });

  it("counts the computers out of five and keeps a card for adding one", async () => {
    await renderSync();
    await screen.findByLabelText("Sync code");
    expect(screen.getByText(`2 of ${MAX_COMPUTERS}`)).toBeDefined();
    const cards = computers();
    expect(cards).toHaveLength(3);
    expect(cards[2].textContent).toBe("Add a computer: enter the code below on it");
  });

  it("drops the add card once the group is full", async () => {
    backend.sync = on({
      peers: ["laptop", "work", "nas", "den"].map((name) => ({ name, lastWriteAt: null })),
    });
    await renderSync();
    await screen.findByLabelText("Sync code");
    expect(screen.getByText("5 of 5")).toBeDefined();
    const cards = computers();
    expect(cards.map((c) => c.textContent)).toEqual([
      "desktopThis oneSynced 2m ago",
      "laptopNo changes yet", "workNo changes yet", "nasNo changes yet", "denNo changes yet",
    ]);
    expect(screen.queryByText(/Add a computer/)).toBeNull();
  });

  it("says so when no other computer has joined", async () => {
    backend.sync = on({ peers: [], lastSyncAt: null });
    await renderSync();
    expect(await screen.findByText("No other computer has joined yet.")).toBeDefined();
    expect(screen.getByText("1 of 5")).toBeDefined();
    const [me, add] = computers();
    expect(me.textContent).toBe("desktopThis oneNot synced yet");
    expect(add.textContent).toMatch(/Add a computer: enter the code below on it/);
    await cardSays("Not synced yet");
    expect(card()!.className).toMatch(/\bis-neutral\b/);
  });

  it("says what travels and what stays on this computer", async () => {
    await renderSync();
    await screen.findByLabelText("Sync code");
    const items = (name: string) => within(screen.getByRole("list", { name }))
      .getAllByRole("listitem").map((li) => li.textContent);
    expect(items("Travels between computers")).toEqual([
      "Subscriptions and memberships", "Watched and hidden videos", "Series you linked by hand",
      "Videos you added yourself",
    ]);
    expect(items("Stays on this computer")).toEqual([
      "Settings and downloaded files", "Auto-download choices",
      "Thumbnails (each computer fetches its own)",
    ]);
  });

  it("says changes are waiting, and shows an error as a warning", async () => {
    backend.sync = on({ pending: true });
    await renderSync();
    await cardSays("Changes will be sent within a minute.");
    expect(card()!.className).toMatch(/\bis-busy\b/);

    const err = "Can't reach the sync server. MyTube will keep trying.";
    await emit("sync://status", on({ pending: true, error: err }));
    const line = await screen.findByText(err);
    expect(line.closest(".st-status")!.className).toMatch(/\bis-error\b/);
    expect(cardTitle()).toBe("Last sync failed");
    expect(screen.queryByText("Changes will be sent within a minute.")).toBeNull();
  });

  it("redraws how long ago every half minute", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval", "Date"] });
    try {
      backend.sync = on();
      await renderSync();
      await cardSays("Synced 2m ago");
      act(() => { vi.advanceTimersByTime(60_000); });
      expect(cardTitle()).toBe("Synced 3m ago");
      expect(computers()[0].textContent).toBe("desktopThis oneSynced 3m ago");
    } finally {
      vi.useRealTimers();
    }
  });

  it("shows and hides the code", async () => {
    await renderSync();
    await screen.findByLabelText("Sync code");
    const show = button("Show");
    expect(show.getAttribute("aria-pressed")).toBe("false");
    fireEvent.click(show);
    expect(codeField().value).toBe(CODE);
    expect(button("Hide").getAttribute("aria-pressed")).toBe("true");
    fireEvent.click(button("Hide"));
    expect(codeField().value).toBe(MASKED);
  });

  it("copies the real code, masked or not", async () => {
    await renderSync();
    await screen.findByLabelText("Sync code");
    fireEvent.click(button("Copy"));
    await waitFor(() => expect(writeText).toHaveBeenCalledWith(CODE));
    expect(await screen.findByText("Sync code copied.")).toBeDefined();
    expect(codeField().value).toBe(MASKED);
  });

  it("shows and selects the code when the clipboard refuses", async () => {
    writeText.mockImplementation(() => Promise.reject(new Error("NotAllowedError")));
    await renderSync();
    await screen.findByLabelText("Sync code");
    fireEvent.click(button("Copy"));

    expect(await screen.findByText(/Copying didn’t work, so the code is shown and selected/))
      .toBeDefined();
    const field = codeField();
    expect(field.value).toBe(CODE);
    await waitFor(() => expect(document.activeElement).toBe(field));
    expect([field.selectionStart, field.selectionEnd]).toEqual([0, CODE.length]);
    expect(screen.queryByText("Sync code copied.")).toBeNull();
  });

  it("falls back the same way where there is no clipboard at all", async () => {
    setClipboard(undefined);
    await renderSync();
    await screen.findByLabelText("Sync code");
    fireEvent.click(button("Copy"));
    expect(await screen.findByText(/the code is shown and selected/)).toBeDefined();
    expect(codeField().value).toBe(CODE);
  });

  it("masks a code that changed under it, and drops the copy note", async () => {
    writeText.mockImplementation(() => Promise.reject(new Error("NotAllowedError")));
    await renderSync();
    await screen.findByLabelText("Sync code");
    fireEvent.click(button("Copy"));
    await screen.findByText(/the code is shown and selected/);

    const fresh = "MYT1-0ZZZZ-ZZZZZ-ZZZZZ-ZZZZZ-ZZZZZZ";
    await emit("sync://status", on({ code: fresh }));
    expect(codeField().value).toBe(maskCode(fresh));
    expect(button("Show").getAttribute("aria-pressed")).toBe("false");
    expect(screen.queryByText(/the code is shown and selected/)).toBeNull();
  });

  it("is busy for as long as Sync now runs, then says it is up to date", async () => {
    const call = hold(api.syncNow);
    await renderSync();
    await cardSays("Synced 2m ago");
    fireEvent.click(button("Sync now"));
    expect(api.syncNow).toHaveBeenCalledTimes(1);
    // Disabled at once, and visibly working once it is slow enough to notice.
    const busy = await screen.findByRole("button", { name: "Syncing…" });
    expect((busy as HTMLButtonElement).disabled).toBe(true);
    expect(busy.getAttribute("aria-busy")).toBe("true");
    expect(busy.className).toMatch(/\bis-spinning\b/);
    fireEvent.click(busy);
    expect(api.syncNow).toHaveBeenCalledTimes(1);

    // The backend sends the status before it answers the button.
    await emit("sync://status", on({ lastSyncAt: real() }));
    await cardSays("Synced just now");
    await call.resolve(false);
    expect(await screen.findByText(SYNCED_TEXT)).toBeDefined();
    expect(button("Sync now").disabled).toBe(false);
    expect(button("Sync now").getAttribute("aria-busy")).toBe("false");
  });

  it("says so when Sync now brought other computers' changes in", async () => {
    api.syncNow.mockImplementation(() => Promise.resolve(true));
    await renderSync();
    fireEvent.click(await screen.findByRole("button", { name: "Sync now" }));
    expect(await screen.findByText(SYNCED_CHANGES_TEXT)).toBeDefined();
    expect(screen.queryByText(SYNCED_TEXT)).toBeNull();
  });

  it("toasts why Sync now failed and gives the button back", async () => {
    api.syncNow.mockImplementation(() => Promise.reject("Could not reach the sync server. Retrying in a minute."));
    await renderSync();
    fireEvent.click(await screen.findByRole("button", { name: "Sync now" }));
    expect(await screen.findByText("Could not reach the sync server. Retrying in a minute.")).toBeDefined();
    expect(screen.queryByText(SYNCED_TEXT)).toBeNull();
    await waitFor(() => expect(button("Sync now").disabled).toBe(false));
  });

  it("asks before resetting the code, and does nothing on Cancel", async () => {
    await renderSync();
    await screen.findByLabelText("Sync code");
    fireEvent.click(button("Reset code…"));
    const dialog = screen.getByRole("dialog", { name: "Create a new sync code?" });
    expect(dialog.textContent).toMatch(
      /Your other computers stop syncing with this one until you enter the new code on them\. Nothing in your library is deleted\./,
    );
    fireEvent.click(button("Cancel"));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(api.syncTurnOn).not.toHaveBeenCalled();
  });

  it("resets the code through Turn on, and says it worked", async () => {
    const call = hold(api.syncTurnOn);
    await renderSync();
    await screen.findByLabelText("Sync code");
    fireEvent.click(button("Show"));
    fireEvent.click(button("Reset code…"));
    fireEvent.click(button("Create new code"));
    expect(api.syncTurnOn).toHaveBeenCalledTimes(1);
    expect(await screen.findByText("Finish the check in the window that opened.")).toBeDefined();
    // One key for every way to Turn on: while it waits, nothing else changes sync.
    expect(button("Waiting…").disabled).toBe(true);
    expect(button("Turn off sync…").disabled).toBe(true);
    expect(button("Sync now").disabled).toBe(true);

    const fresh = "MYT1-0ZZZZ-ZZZZZ-ZZZZZ-ZZZZZ-ZZZZZZ";
    await call.resolve(on({ code: fresh }));
    expect(await screen.findByText(/New sync code created/)).toBeDefined();
    // A new code starts out hidden like the first one.
    expect(codeField().value).toBe(maskCode(fresh));
    fireEvent.click(button("Show"));
    expect(codeField().value).toBe(fresh);
  });

  it("asks before turning sync off, then shows the off state", async () => {
    await renderSync();
    await screen.findByLabelText("Sync code");
    fireEvent.click(button("Turn off sync…"));
    const dialog = screen.getByRole("dialog", { name: "Turn off sync on this computer?" });
    expect(dialog.textContent).toMatch(
      /It forgets the sync code and stops syncing\. Your library stays exactly as it is, and your other computers keep syncing with each other\./,
    );
    fireEvent.click(within(dialog).getByRole("button", { name: "Turn off sync" }));
    expect(api.syncLeave).toHaveBeenCalledTimes(1);
    expect(await screen.findByRole("button", { name: "Turn on sync" })).toBeDefined();
    expect(screen.queryByLabelText("Sync code")).toBeNull();
    expect(lineOf("Sync")).toBe("Off");
  });
});

describe("once the code has expired", () => {
  beforeEach(() => { backend.sync = on({ expired: true }); });

  it("explains, and offers to start again", async () => {
    await renderSync();
    expect(await screen.findByText(EXPIRED_TEXT)).toBeDefined();
    expect(card()!.className).toMatch(/\bis-error\b/);
    expect(screen.queryByLabelText("Sync code")).toBeNull();
    expect(screen.queryByRole("button", { name: "Sync now" })).toBeNull();
    expect(screen.queryByRole("list", { name: "Computers" })).toBeNull();
    fireEvent.click(button("Turn on sync again"));
    expect(api.syncTurnOn).toHaveBeenCalledTimes(1);
    expect(await screen.findByLabelText("Sync code")).toBeDefined();
  });

  it("says where to finish while starting again waits", async () => {
    const call = hold(api.syncTurnOn);
    await renderSync();
    fireEvent.click(await screen.findByRole("button", { name: "Turn on sync again" }));
    expect(await screen.findByText("Finish the check in the window that opened.")).toBeDefined();
    expect(button("Waiting…").disabled).toBe(true);
    expect(button("Turn off sync").disabled).toBe(true);
    await call.resolve(on());
    expect(screen.queryByText("Finish the check in the window that opened.")).toBeNull();
  });

  it("turns off without asking", async () => {
    await renderSync();
    fireEvent.click(await screen.findByRole("button", { name: "Turn off sync" }));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(api.syncLeave).toHaveBeenCalledTimes(1);
    expect(await screen.findByRole("button", { name: "Turn on sync" })).toBeDefined();
  });
});

describe("following sync://status", () => {
  it("moves from off to on when another path turns sync on", async () => {
    await renderSync();
    await screen.findByRole("button", { name: "Turn on sync" });
    await emit("sync://status", on());
    expect(await screen.findByLabelText("Sync code")).toBeDefined();
  });

  it("keeps an event that lands before the first read over that older read", async () => {
    const read = hold(api.syncStatus);
    await renderSync();
    await emit("sync://status", on());
    await read.resolve(OFF);
    expect(screen.getByLabelText("Sync code")).toBeDefined();
  });

  it("says in place when the status cannot be read", async () => {
    api.syncStatus.mockImplementation(() => Promise.reject("command sync_status not found"));
    await renderSync();
    expect(await screen.findByText("Could not read the sync status: command sync_status not found"))
      .toBeDefined();
  });
});
