import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, cleanup, waitFor, act } from "@testing-library/react";
import type { SyncStatus } from "../types";

// Every listener by event name, so a test can deliver `sync://status` when it wants.
const listeners = new Map<string, (e: { payload: unknown }) => void>();
vi.mock("@tauri-apps/api/event", () => ({
  listen: (name: string, fn: (e: { payload: unknown }) => void) => {
    listeners.set(name, fn);
    return Promise.resolve(() => listeners.delete(name));
  },
}));

const CODE = "MYT1-7ABCD-EFGHJ-KMNPQ-RSTVW-XYZ012";
const MASKED = "MYT1-•••••-•••••-•••••-•••••-••••••";
const NOW = 1_790_000_000;

const OFF: SyncStatus = {
  enabled: false, code: null, machineName: "desktop", peers: [], lastSyncAt: null,
  pending: false, error: null, expired: false,
};
// Relative to the real clock, since the section formats against it.
const real = () => Math.floor(Date.now() / 1000);
const on = (over: Partial<SyncStatus> = {}): SyncStatus => ({
  enabled: true, code: CODE, machineName: "desktop",
  peers: [{ name: "laptop", lastWriteAt: real() - 180 }],
  lastSyncAt: real() - 120, pending: false, error: null, expired: false,
  ...over,
});

let initial: SyncStatus = OFF;
const syncStatus = vi.fn<() => Promise<SyncStatus>>(() => Promise.resolve(initial));
const syncTurnOn = vi.fn<() => Promise<SyncStatus>>(() => Promise.resolve(on()));
const syncJoin = vi.fn<(code: string) => Promise<SyncStatus>>(() => Promise.resolve(on()));
const syncLeave = vi.fn<() => Promise<SyncStatus>>(() => Promise.resolve(OFF));
const syncNow = vi.fn<() => Promise<void>>(() => Promise.resolve());

vi.mock("../api", () => ({
  api: {
    syncStatus: () => syncStatus(),
    syncTurnOn: () => syncTurnOn(),
    syncJoin: (code: string) => syncJoin(code),
    syncLeave: () => syncLeave(),
    syncNow: () => syncNow(),
  },
  errText: (e: unknown) => String(e),
}));

import SyncSection, { EXPIRED_TEXT, maskCode, peersLine, statusLine } from "./SyncSection";
import { ToastProvider } from "./Toast";

function renderSync() {
  render(<ToastProvider><SyncSection /></ToastProvider>);
}

const button = (name: string | RegExp) => screen.getByRole("button", { name }) as HTMLButtonElement;
const codeField = () => screen.getByLabelText("Sync code") as HTMLInputElement;
const joinField = () => screen.getByLabelText("Have a code from another computer?") as HTMLInputElement;

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

beforeEach(() => {
  initial = OFF;
  vi.clearAllMocks();
  syncStatus.mockImplementation(() => Promise.resolve(initial));
  syncTurnOn.mockImplementation(() => Promise.resolve(on()));
  syncJoin.mockImplementation(() => Promise.resolve(on()));
  syncLeave.mockImplementation(() => Promise.resolve(OFF));
  syncNow.mockImplementation(() => Promise.resolve());
  writeText.mockImplementation(() => Promise.resolve());
  setClipboard({ writeText });
  listeners.clear();
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

describe("while sync is off", () => {
  it("explains sync and offers to turn it on or join", async () => {
    renderSync();
    expect(await screen.findByRole("button", { name: "Turn on sync" })).toBeDefined();
    expect(screen.getByRole("heading", { name: "Sync" })).toBeDefined();
    expect(screen.getByText(/Everything is encrypted on this computer first/)).toBeDefined();
    const field = joinField();
    expect(field.placeholder).toBe("MYT1-…");
    expect(field.getAttribute("spellcheck")).toBe("false");
    expect(field.getAttribute("autocomplete")).toBe("off");
    expect(field.className).toMatch(/\bmono\b/);
    expect(screen.queryByLabelText("Sync code")).toBeNull();
  });

  it("turns sync on, saying where to finish while it waits", async () => {
    const call = hold(syncTurnOn);
    renderSync();
    fireEvent.click(await screen.findByRole("button", { name: "Turn on sync" }));
    expect(syncTurnOn).toHaveBeenCalledTimes(1);
    expect(await screen.findByText("Finish the check in the window that opened.")).toBeDefined();
    // One verification at a time, and no join racing it.
    expect(button("Waiting…").disabled).toBe(true);
    fireEvent.change(joinField(), { target: { value: CODE } });
    expect(button("Join").disabled).toBe(true);

    await call.resolve(on());
    expect(codeField().value).toBe(MASKED);
    expect(screen.queryByText("Finish the check in the window that opened.")).toBeNull();
  });

  it("toasts a cancelled verification and stays off", async () => {
    syncTurnOn.mockImplementation(() => Promise.reject("Verification was cancelled."));
    renderSync();
    fireEvent.click(await screen.findByRole("button", { name: "Turn on sync" }));
    expect(await screen.findByText("Verification was cancelled.")).toBeDefined();
    expect(button("Turn on sync").disabled).toBe(false);
  });

  it("keeps Join disabled until there is a code, and trims what it sends", async () => {
    renderSync();
    await screen.findByRole("button", { name: "Turn on sync" });
    expect(button("Join").disabled).toBe(true);
    fireEvent.change(joinField(), { target: { value: "   " } });
    expect(button("Join").disabled).toBe(true);

    fireEvent.change(joinField(), { target: { value: `  ${CODE}\n` } });
    fireEvent.click(button("Join"));
    expect(syncJoin).toHaveBeenCalledWith(CODE);
    expect(await screen.findByLabelText("Sync code")).toBeDefined();
  });

  it("joins on Enter too", async () => {
    renderSync();
    await screen.findByRole("button", { name: "Turn on sync" });
    fireEvent.change(joinField(), { target: { value: CODE } });
    fireEvent.submit(joinField().form!);
    expect(syncJoin).toHaveBeenCalledWith(CODE);
    await screen.findByLabelText("Sync code");
  });

  it("shows a refused code under the field, once, until it is edited", async () => {
    const reason = "No sync group uses this code. Check it and try again.";
    syncJoin.mockImplementation(() => Promise.reject(reason));
    renderSync();
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
  beforeEach(() => { initial = on(); });

  it("shows the masked code, this computer, the others and the last sync", async () => {
    renderSync();
    const field = await screen.findByLabelText("Sync code") as HTMLInputElement;
    expect(field.value).toBe(MASKED);
    expect(field.readOnly).toBe(true);
    expect(screen.getByText(/Anyone with it can see and change your synced library/)).toBeDefined();
    expect(screen.getByText("This computer:").textContent).toBe("This computer: desktop");
    expect(screen.getByText("Also syncing: laptop · 3m ago")).toBeDefined();
    const line = screen.getByText("Synced 2m ago");
    expect(line.getAttribute("aria-live")).toBe("polite");
    expect(line.className).not.toMatch(/is-warn/);
    // The explanation is for deciding; once on, the code's own note says what matters.
    expect(screen.queryByText(/no account is needed/)).toBeNull();
    expect(screen.queryByRole("button", { name: "Turn on sync" })).toBeNull();
  });

  it("says so when no other computer has joined", async () => {
    initial = on({ peers: [], lastSyncAt: null });
    renderSync();
    expect(await screen.findByText("No other computer has joined yet.")).toBeDefined();
    expect(screen.getByText("Not synced yet")).toBeDefined();
  });

  it("says changes are waiting, and shows an error as a warning", async () => {
    initial = on({ pending: true });
    renderSync();
    expect(await screen.findByText("Changes will be sent within a minute.")).toBeDefined();

    const err = "Can't reach the sync server. MyTube will keep trying.";
    act(() => listeners.get("sync://status")!({ payload: on({ pending: true, error: err }) }));
    const line = await screen.findByText(err);
    expect(line.className).toMatch(/is-warn/);
    expect(screen.queryByText("Changes will be sent within a minute.")).toBeNull();
  });

  it("shows and hides the code", async () => {
    renderSync();
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
    renderSync();
    await screen.findByLabelText("Sync code");
    fireEvent.click(button("Copy"));
    await waitFor(() => expect(writeText).toHaveBeenCalledWith(CODE));
    expect(await screen.findByText("Sync code copied.")).toBeDefined();
    expect(codeField().value).toBe(MASKED);
  });

  it("shows and selects the code when the clipboard refuses", async () => {
    writeText.mockImplementation(() => Promise.reject(new Error("NotAllowedError")));
    renderSync();
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
    renderSync();
    await screen.findByLabelText("Sync code");
    fireEvent.click(button("Copy"));
    expect(await screen.findByText(/the code is shown and selected/)).toBeDefined();
    expect(codeField().value).toBe(CODE);
  });

  it("nudges a sync, and lets the next status answer it", async () => {
    renderSync();
    await screen.findByText("Synced 2m ago");
    fireEvent.click(button("Sync now"));
    expect(syncNow).toHaveBeenCalledTimes(1);
    expect(await screen.findByText("Syncing…")).toBeDefined();

    act(() => listeners.get("sync://status")!({ payload: on({ lastSyncAt: real() }) }));
    expect(await screen.findByText("Synced just now")).toBeDefined();
    expect(screen.queryByText("Syncing…")).toBeNull();
  });

  it("asks before resetting the code, and does nothing on Cancel", async () => {
    renderSync();
    await screen.findByLabelText("Sync code");
    fireEvent.click(button("Reset code…"));
    const dialog = screen.getByRole("dialog", { name: "Create a new sync code?" });
    expect(dialog.textContent).toMatch(
      /Your other computers stop syncing with this one until you enter the new code on them\. Nothing in your library is deleted\./,
    );
    fireEvent.click(button("Cancel"));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(syncTurnOn).not.toHaveBeenCalled();
  });

  it("resets the code through Turn on, and says it worked", async () => {
    const call = hold(syncTurnOn);
    renderSync();
    await screen.findByLabelText("Sync code");
    fireEvent.click(button("Show"));
    fireEvent.click(button("Reset code…"));
    fireEvent.click(button("Create new code"));
    expect(syncTurnOn).toHaveBeenCalledTimes(1);
    expect(await screen.findByText("Finish the check in the window that opened.")).toBeDefined();

    const fresh = "MYT1-0ZZZZ-ZZZZZ-ZZZZZ-ZZZZZ-ZZZZZZ";
    await call.resolve(on({ code: fresh }));
    expect(await screen.findByText(/New sync code created/)).toBeDefined();
    // A new code starts out hidden like the first one.
    expect(codeField().value).toBe(maskCode(fresh));
    fireEvent.click(button("Show"));
    expect(codeField().value).toBe(fresh);
  });

  it("asks before turning sync off, then shows the off state", async () => {
    renderSync();
    await screen.findByLabelText("Sync code");
    fireEvent.click(button("Turn off sync…"));
    const dialog = screen.getByRole("dialog", { name: "Turn off sync on this computer?" });
    expect(dialog.textContent).toMatch(
      /It forgets the sync code and stops syncing\. Your library stays exactly as it is, and your other computers keep syncing with each other\./,
    );
    fireEvent.click(button("Turn off sync"));
    expect(syncLeave).toHaveBeenCalledTimes(1);
    expect(await screen.findByRole("button", { name: "Turn on sync" })).toBeDefined();
    expect(screen.queryByLabelText("Sync code")).toBeNull();
  });
});

describe("once the code has expired", () => {
  beforeEach(() => { initial = on({ expired: true }); });

  it("explains, and offers to start again", async () => {
    renderSync();
    expect(await screen.findByText(EXPIRED_TEXT)).toBeDefined();
    expect(screen.queryByLabelText("Sync code")).toBeNull();
    expect(screen.queryByRole("button", { name: "Sync now" })).toBeNull();
    fireEvent.click(button("Turn on sync again"));
    expect(syncTurnOn).toHaveBeenCalledTimes(1);
    expect(await screen.findByLabelText("Sync code")).toBeDefined();
  });

  it("turns off without asking", async () => {
    renderSync();
    fireEvent.click(await screen.findByRole("button", { name: "Turn off sync" }));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(syncLeave).toHaveBeenCalledTimes(1);
    expect(await screen.findByRole("button", { name: "Turn on sync" })).toBeDefined();
  });
});

describe("following sync://status", () => {
  it("moves from off to on when another path turns sync on", async () => {
    renderSync();
    await screen.findByRole("button", { name: "Turn on sync" });
    await waitFor(() => expect(listeners.has("sync://status")).toBe(true));
    act(() => listeners.get("sync://status")!({ payload: on() }));
    expect(await screen.findByLabelText("Sync code")).toBeDefined();
  });

  it("keeps an event that lands before the first read over that older read", async () => {
    const read = hold(syncStatus);
    renderSync();
    await waitFor(() => expect(listeners.has("sync://status")).toBe(true));
    act(() => listeners.get("sync://status")!({ payload: on() }));
    await read.resolve(OFF);
    expect(screen.getByLabelText("Sync code")).toBeDefined();
  });

  it("says in place when the status cannot be read", async () => {
    syncStatus.mockImplementation(() => Promise.reject("command sync_status not found"));
    renderSync();
    expect(await screen.findByText("Could not read the sync status: command sync_status not found"))
      .toBeDefined();
  });
});
