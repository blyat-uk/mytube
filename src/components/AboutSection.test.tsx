import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, cleanup, waitFor, act } from "@testing-library/react";
import type { VersionInfo } from "../types";

const listeners = new Map<string, (e: { payload: unknown }) => void>();
vi.mock("@tauri-apps/api/event", () => ({
  listen: (name: string, fn: (e: { payload: unknown }) => void) => {
    listeners.set(name, fn);
    return Promise.resolve(() => listeners.delete(name));
  },
}));

const UP_TO_DATE: VersionInfo = {
  current: "2.1.0", update: null, checkedAt: 1_790_000_000, checksEnabled: true,
};
const RELEASE = "https://github.com/blyat-uk/mytube/releases/tag/v2.2.0";
const BEHIND: VersionInfo = { ...UP_TO_DATE, update: { version: "2.2.0", url: RELEASE } };

let initial: VersionInfo = UP_TO_DATE;
const checkAppUpdateNow = vi.fn<() => Promise<VersionInfo>>(() => Promise.resolve(UP_TO_DATE));
const openExternal = vi.fn<(url: string) => Promise<void>>(() => Promise.resolve());

vi.mock("../api", () => ({
  api: {
    appVersionInfo: () => Promise.resolve(initial),
    checkAppUpdateNow: () => checkAppUpdateNow(),
    openExternal: (url: string) => openExternal(url),
  },
  errText: (e: unknown) => String(e),
}));

import AboutSection, { aboutStatus } from "./AboutSection";
import { ToastProvider } from "./Toast";

function renderAbout() {
  render(<ToastProvider><AboutSection /></ToastProvider>);
}

beforeEach(() => {
  initial = UP_TO_DATE;
  checkAppUpdateNow.mockReset();
  checkAppUpdateNow.mockImplementation(() => Promise.resolve(UP_TO_DATE));
  openExternal.mockClear();
  listeners.clear();
});

afterEach(cleanup);

describe("aboutStatus", () => {
  const now = 1_790_000_000 + 2 * 3600;

  it("says nothing before the first read", () => {
    expect(aboutStatus(null)).toBe("");
  });

  it("names a newer release", () => {
    expect(aboutStatus(BEHIND, now)).toBe("MyTube 2.2.0 is available.");
  });

  it("says when it last looked", () => {
    expect(aboutStatus(UP_TO_DATE, now)).toBe("Up to date · checked 2h ago");
    expect(aboutStatus({ ...UP_TO_DATE, checkedAt: null }, now)).toBe("Not checked yet.");
  });

  it("says checks are off rather than claiming anything is current", () => {
    expect(aboutStatus({ ...UP_TO_DATE, checksEnabled: false }, now)).toBe("Update checks are off.");
  });
});

describe("the About block", () => {
  it("shows the running version and links to its release notes", async () => {
    renderAbout();
    expect(await screen.findByText("2.1.0")).toBeDefined();
    fireEvent.click(screen.getByRole("button", { name: "Release notes…" }));
    expect(openExternal).toHaveBeenCalledWith(
      "https://github.com/blyat-uk/mytube/releases/tag/v2.1.0",
    );
  });

  it("offers a newer release's page", async () => {
    initial = BEHIND;
    renderAbout();
    fireEvent.click(await screen.findByRole("button", { name: "Get 2.2.0…" }));
    expect(openExternal).toHaveBeenCalledWith(RELEASE);
  });

  it("checks now, and says so when nothing is newer", async () => {
    renderAbout();
    await screen.findByText("2.1.0");
    fireEvent.click(screen.getByRole("button", { name: "Check now" }));
    expect(await screen.findByText("MyTube 2.1.0 is up to date.")).toBeDefined();
    expect(checkAppUpdateNow).toHaveBeenCalledTimes(1);
  });

  it("shows what a check found without a toast", async () => {
    checkAppUpdateNow.mockImplementation(() => Promise.resolve(BEHIND));
    renderAbout();
    await screen.findByText("2.1.0");
    fireEvent.click(screen.getByRole("button", { name: "Check now" }));
    expect(await screen.findByRole("button", { name: "Get 2.2.0…" })).toBeDefined();
    expect(screen.queryByText(/is up to date/)).toBeNull();
  });

  it("sends one check for a double click", async () => {
    let resolve!: (v: VersionInfo) => void;
    checkAppUpdateNow.mockImplementation(() => new Promise((r) => { resolve = r; }));
    renderAbout();
    await screen.findByText("2.1.0");
    const btn = screen.getByRole("button", { name: "Check now" });
    fireEvent.click(btn);
    fireEvent.click(btn);
    expect(checkAppUpdateNow).toHaveBeenCalledTimes(1);
    await act(async () => { resolve(UP_TO_DATE); });
  });

  it("toasts a failed check", async () => {
    checkAppUpdateNow.mockImplementation(() => Promise.reject("GitHub answered 403 Forbidden"));
    renderAbout();
    await screen.findByText("2.1.0");
    fireEvent.click(screen.getByRole("button", { name: "Check now" }));
    expect(await screen.findByText("GitHub answered 403 Forbidden")).toBeDefined();
  });

  it("cannot check while checks are off, and follows the setting flipping on", async () => {
    initial = { ...UP_TO_DATE, checksEnabled: false };
    renderAbout();
    const btn = await screen.findByRole("button", { name: "Check now" }) as HTMLButtonElement;
    await waitFor(() => expect(btn.disabled).toBe(true));
    expect(screen.getByText("Update checks are off.")).toBeDefined();

    await waitFor(() => expect(listeners.has("app://version-info")).toBe(true));
    act(() => listeners.get("app://version-info")!({ payload: UP_TO_DATE }));
    await waitFor(() => expect(btn.disabled).toBe(false));
  });
});
