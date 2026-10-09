import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { screen, fireEvent, cleanup, waitFor, within, act } from "@testing-library/react";

vi.mock("@tauri-apps/api/event", async () => (await import("./testApi")).eventModule);
vi.mock("@tauri-apps/plugin-dialog", async () => (await import("./testApi")).dialogModule);
vi.mock("../../api", async () => (await import("./testApi")).apiModule);

import {
  api, backend, emit, lastSaved, resetHarness, SETTINGS, tool, TOOLS_BROKEN, TOOLS_OK,
} from "./testApi";
import { renderSettings } from "./testRender";
import { summary } from "./ToolsPage";
import type { SettingsCtx } from "./context";
import type { ToolStatus } from "../../types";

beforeEach(resetHarness);
afterEach(cleanup);

/** The page itself, so a query never lands on the rail's copy of a line. */
const page = () => screen.getByRole("region", { name: "Tools" });

async function renderTools() {
  await renderSettings("tools");
  return page();
}

/** Once the first status has landed and the cards are up. */
async function renderCards() {
  const region = await renderTools();
  await within(region).findByText("yt-dlp");
  return region;
}

const card = (label: string) => within(page()).getByText(label).closest(".st-tl-card") as HTMLElement;
const dotOf = (label: string) => card(label).querySelector(".st-tl-dot") as HTMLElement;

describe("Tools page", () => {
  it("heads the page with a real heading", async () => {
    await renderTools();
    expect(screen.getByRole("heading", { level: 2, name: "Tools" })).toBeDefined();
    expect(page().textContent).toMatch(/checked against its checksum before it is used/);
  });

  it("names where each tool comes from", async () => {
    await renderCards();
    expect(await screen.findByText("Managed by MyTube")).toBeDefined();
    expect(screen.getByText("System")).toBeDefined();
    expect(screen.getByText("Custom path (settings.json)")).toBeDefined();
    expect(screen.getByText("2026.09.20")).toBeDefined();
    expect(screen.getByText("/usr/bin/ffmpeg")).toBeDefined();
  });

  it("says what each tool is for", async () => {
    await renderCards();
    expect(card("yt-dlp").textContent).toMatch(/Finds each video's streams and downloads them/);
    expect(card("ffmpeg").textContent).toMatch(/Joins video and audio into one file/);
    expect(card("deno").textContent).toMatch(/Answers the JavaScript check YouTube sets/);
  });

  it("keeps the whole path in a tooltip, since a long one is cut short", async () => {
    await renderCards();
    const path = screen.getByText("/home/someone/.local/share/mytube/bin/yt-dlp");
    expect(path.getAttribute("title")).toBe("/home/someone/.local/share/mytube/bin/yt-dlp");
  });

  it("shows a failed install's error and offers Retry only then", async () => {
    backend.tools = TOOLS_BROKEN;
    await renderCards();
    expect(await screen.findByText("Not installed")).toBeDefined();
    expect(screen.getByText("checksum mismatch for deno")).toBeDefined();
    const retry = within(card("deno")).getByRole("button", { name: "Retry" });
    fireEvent.click(retry);
    await waitFor(() => expect(api.toolsUpdateNow).toHaveBeenCalledTimes(1));
    // The fresh status the command answered with replaces the broken card.
    await waitFor(() => expect(screen.queryByText("checksum mismatch for deno")).toBeNull());
    expect(screen.queryByRole("button", { name: "Retry" })).toBeNull();
  });

  it("has no Retry while every tool is fine", async () => {
    await renderCards();
    await screen.findByText("Managed by MyTube");
    expect(screen.queryByRole("button", { name: "Retry" })).toBeNull();
  });

  /** Retry is on the broken card only: a working tool has nothing to retry. */
  it("puts Retry on the broken card, described by its error", async () => {
    backend.tools = TOOLS_BROKEN;
    await renderCards();
    const retry = within(card("deno")).getByRole("button", { name: "Retry" });
    expect(retry.className).toContain("btn-primary");
    expect(document.getElementById(retry.getAttribute("aria-describedby")!)?.textContent)
      .toBe("checksum mismatch for deno");
    expect(within(card("yt-dlp")).queryByRole("button", { name: "Retry" })).toBeNull();
  });

  it("checks for updates and reports a refusal", async () => {
    api.toolsUpdateNow.mockImplementationOnce(() =>
      Promise.reject("busy: a download is using yt-dlp"),
    );
    await renderCards();
    fireEvent.click(screen.getByRole("button", { name: "Check for updates" }));
    expect(await screen.findByText("busy: a download is using yt-dlp")).toBeDefined();
  });

  it("says Checking… once the check is slow, and sends one for a double click", async () => {
    let resolve!: (list: ToolStatus[]) => void;
    api.toolsUpdateNow.mockImplementation(() => new Promise((r) => { resolve = r; }));
    await renderCards();
    const btn = screen.getByRole("button", { name: "Check for updates" }) as HTMLButtonElement;
    fireEvent.click(btn);
    fireEvent.click(btn);
    expect(api.toolsUpdateNow).toHaveBeenCalledTimes(1);
    expect(btn.disabled).toBe(true);
    expect(await screen.findByRole("button", { name: "Checking…" })).toBe(btn);
    await act(async () => { resolve(TOOLS_OK); });
    expect(await screen.findByText("yt-dlp is up to date.")).toBeDefined();
    expect(btn.textContent).toBe("Check for updates");
  });

  /** An update check that fails leaves a managed yt-dlp working at the version
   *  it has: said quietly on the card, with no alert and no Retry. */
  it("shows a working tool's error as a note, not an alert", async () => {
    backend.tools = [
      { ...TOOLS_OK[0], error: "Update check failed: HTTP 503" },
      TOOLS_OK[1],
      TOOLS_OK[2],
    ];
    await renderCards();
    const note = await screen.findByText("Update check failed: HTTP 503");
    expect(note.className).toBe("st-tl-note");
    expect(note.getAttribute("role")).toBeNull();
    expect(note.closest(".st-tl-card")?.classList.contains("is-error")).toBe(false);
    expect(screen.queryByRole("button", { name: "Retry" })).toBeNull();
  });

  it("keeps the red alert for a tool that is actually broken", async () => {
    backend.tools = TOOLS_BROKEN;
    await renderCards();
    const alert = await screen.findByText("checksum mismatch for deno");
    expect(alert.className).toBe("st-tl-error");
    expect(alert.getAttribute("role")).toBe("alert");
    expect(alert.closest(".st-tl-card")?.classList.contains("is-error")).toBe(true);
  });

  it("announces an update to a yt-dlp that was already managed", async () => {
    backend.tools = [{ ...TOOLS_OK[0], version: "2026.09.10" }, TOOLS_OK[1], TOOLS_OK[2]];
    await renderCards();
    await screen.findByText("2026.09.10");
    fireEvent.click(screen.getByRole("button", { name: "Check for updates" }));
    expect(await screen.findByText("yt-dlp updated to 2026.09.20.")).toBeDefined();
  });

  /** A system yt-dlp is replaced by a managed install, which the shell already
   *  toasts as "yt-dlp is ready." — saying "updated to" as well is one event
   *  told twice. */
  it("says nothing of its own when the check installs a managed yt-dlp", async () => {
    backend.tools = [
      tool({ kind: "ytdlp", source: "system", version: "2025.01.01", path: "/usr/bin/yt-dlp" }),
      TOOLS_OK[1],
      TOOLS_OK[2],
    ];
    await renderCards();
    await screen.findByText("2025.01.01");
    fireEvent.click(screen.getByRole("button", { name: "Check for updates" }));
    await waitFor(() => expect(api.toolsUpdateNow).toHaveBeenCalledTimes(1));
    // The card taking the answer is the sign the call has fully settled.
    expect(await screen.findByText("Managed by MyTube")).toBeDefined();
    expect(screen.queryByText(/yt-dlp updated to/)).toBeNull();
    expect(screen.queryByText("yt-dlp is up to date.")).toBeNull();
  });

  it("says when yt-dlp was last checked, beside the button", async () => {
    await renderCards();
    const block = screen.getByRole("heading", { level: 3, name: "yt-dlp updates" }).closest(".st-block")!;
    expect(block.querySelector(".st-block-aside")?.textContent).toMatch(/^Last checked .+ ago/);
  });

  it("says nothing of a last check that never happened", async () => {
    backend.tools = [{ ...TOOLS_OK[0], lastCheck: null }, TOOLS_OK[1], TOOLS_OK[2]];
    await renderCards();
    expect(screen.queryByText(/Last checked/)).toBeNull();
  });

  it("links the update channel's hint to its choice", async () => {
    await renderCards();
    const group = screen.getByRole("radiogroup", { name: "yt-dlp channel" });
    const hint = document.getElementById(group.getAttribute("aria-describedby")!);
    expect(hint?.textContent).toMatch(/reach nightly first/);
  });

  it("saves the update channel and the auto-update switch", async () => {
    await renderCards();
    const nightly = screen.getByRole("radio", { name: /Nightly/ }) as HTMLInputElement;
    const stable = screen.getByRole("radio", { name: "Stable" }) as HTMLInputElement;
    expect(nightly.checked).toBe(true);
    expect(nightly.closest("label")?.textContent).toMatch(/recommended/);
    fireEvent.click(stable);
    await waitFor(() => expect(api.saveSettings).toHaveBeenCalledTimes(1));
    expect(lastSaved().ytdlp_channel).toBe("stable");
    expect(stable.checked).toBe(true);

    const auto = screen.getByRole("switch", { name: "Update automatically" }) as HTMLInputElement;
    expect(auto.checked).toBe(true);
    fireEvent.click(auto);
    await waitFor(() => expect(api.saveSettings).toHaveBeenCalledTimes(2));
    expect(lastSaved().ytdlp_auto_update).toBe(false);
  });

  /** Only "stable" means stable to the backend; anything else is nightly. */
  it("reads an unknown channel as nightly", async () => {
    backend.settings = { ...SETTINGS, ytdlp_channel: "master" };
    await renderCards();
    expect((screen.getByRole("radio", { name: /Nightly/ }) as HTMLInputElement).checked).toBe(true);
  });

  it("says that a path in settings.json wins", async () => {
    await renderCards();
    const foot = page().querySelector(".st-tl-foot")!;
    expect(foot.textContent).toMatch(/A path you set yourself in settings\.json \(ytdlp_path and so on\) always wins/);
    expect([...foot.querySelectorAll("code")].map((c) => c.textContent)).toEqual(["settings.json", "ytdlp_path"]);
  });
});

describe("before the status is known", () => {
  it("says it is checking until the first read lands", async () => {
    api.toolsStatus.mockImplementation(() => new Promise(() => {}));
    const region = await renderTools();
    expect(within(region).getByText("Checking…")).toBeDefined();
    expect(within(region).queryByRole("list")).toBeNull();
    expect((screen.getByRole("button", { name: "Check for updates" }) as HTMLButtonElement).disabled).toBe(true);
  });

  /** Said in place: a toast would be gone while the page sat on "Checking…". */
  it("says why the status could not be read", async () => {
    api.toolsStatus.mockImplementation(() => Promise.reject("tools lock poisoned"));
    const region = await renderTools();
    expect(await within(region).findByText("Could not read the tools' status: tools lock poisoned"))
      .toBeDefined();
    expect(within(region).queryByText("Checking…")).toBeNull();
  });

  it("takes the list from tools://status once it arrives", async () => {
    api.toolsStatus.mockImplementation(() => new Promise(() => {}));
    await renderTools();
    await emit("tools://status", TOOLS_OK);
    expect(within(page()).getByText("Managed by MyTube")).toBeDefined();
  });
});

describe("health", () => {
  it("is a green dot for a tool that is ready", async () => {
    await renderCards();
    expect(card("yt-dlp").className).toContain("is-ready");
    expect(dotOf("yt-dlp").getAttribute("aria-label")).toBe("Ready");
  });

  it("is a red dot on a pink card for a broken one", async () => {
    backend.tools = TOOLS_BROKEN;
    await renderCards();
    expect(card("deno").className).toContain("is-error");
    expect(dotOf("deno").getAttribute("aria-label")).toBe("Not working");
  });

  /** Absent and not yet attempted is not a fault: the install loop simply has
   *  not reached it. */
  it("is a grey dot for a tool missing and not yet attempted", async () => {
    backend.tools = [TOOLS_OK[0], tool({ kind: "ffmpeg" }), TOOLS_OK[2]];
    await renderCards();
    expect(card("ffmpeg").className).toContain("is-missing");
    expect(dotOf("ffmpeg").getAttribute("aria-label")).toBe("Not installed yet");
    expect(within(card("ffmpeg")).getByText("Not installed")).toBeDefined();
    expect(screen.queryByRole("button", { name: "Retry" })).toBeNull();
  });

  it("is a blue dot while a tool installs or updates", async () => {
    backend.tools = [
      { ...TOOLS_OK[0], state: "updating" },
      tool({ kind: "ffmpeg", state: "installing" }),
      TOOLS_OK[2],
    ];
    await renderCards();
    expect(card("yt-dlp").className).toContain("is-busy");
    expect(dotOf("yt-dlp").getAttribute("aria-label")).toBe("Updating");
    expect(dotOf("ffmpeg").getAttribute("aria-label")).toBe("Installing");
  });
});

describe("an install or update in progress", () => {
  const MB = 1024 * 1024;
  const UPDATING: ToolStatus[] = [{ ...TOOLS_OK[0], state: "updating" }, TOOLS_OK[1], TOOLS_OK[2]];

  it("draws a bar that counts the bytes against the total", async () => {
    backend.tools = UPDATING;
    await renderCards();
    await emit("tools://progress", { tool: "ytdlp", phase: "download", received: 17 * MB, total: 34 * MB });
    const bar = within(card("yt-dlp")).getByRole("progressbar", { name: "Updating yt-dlp" });
    expect(bar.getAttribute("aria-valuenow")).toBe("50");
    expect(bar.className).not.toContain("is-indeterminate");
    expect(card("yt-dlp").textContent).toMatch(/Updating · 17 MB of 34 MB/);
    // The bar takes the place of the role and the path while it runs.
    expect(card("yt-dlp").textContent).not.toMatch(/Finds each video's streams/);
    expect(screen.queryByText("/home/someone/.local/share/mytube/bin/yt-dlp")).toBeNull();
  });

  it("travels without a number when there is no total, and while unpacking", async () => {
    backend.tools = [TOOLS_OK[0], tool({ kind: "ffmpeg", state: "installing" }), TOOLS_OK[2]];
    await renderCards();
    const bar = within(card("ffmpeg")).getByRole("progressbar", { name: "Installing ffmpeg" });
    expect(bar.className).toContain("is-indeterminate");
    expect(bar.getAttribute("aria-valuenow")).toBeNull();
    expect(card("ffmpeg").textContent).toMatch(/Installing · Starting…/);

    await emit("tools://progress", { tool: "ffmpeg", phase: "download", received: 3 * MB, total: null });
    expect(card("ffmpeg").textContent).toMatch(/Installing · 3\.0 MB/);
    expect(bar.getAttribute("aria-valuenow")).toBeNull();

    await emit("tools://progress", { tool: "ffmpeg", phase: "extract", received: 0, total: null });
    expect(card("ffmpeg").textContent).toMatch(/Installing · Unpacking…/);
  });

  it("holds Check for updates and Retry while anything is busy", async () => {
    backend.tools = [
      { ...TOOLS_OK[0], state: "updating" },
      TOOLS_OK[1],
      TOOLS_BROKEN[2],
    ];
    await renderCards();
    expect((screen.getByRole("button", { name: "Check for updates" }) as HTMLButtonElement).disabled).toBe(true);
    expect((within(card("deno")).getByRole("button", { name: "Retry" }) as HTMLButtonElement).disabled).toBe(true);
  });

  /** A finished download's byte count would otherwise reappear, stale, the
   *  next time that tool installs. */
  it("drops the bar when the tool is ready, and starts afresh next time", async () => {
    backend.tools = UPDATING;
    await renderCards();
    await emit("tools://progress", { tool: "ytdlp", phase: "download", received: 34 * MB, total: 34 * MB });
    await emit("tools://status", TOOLS_OK);
    expect(within(card("yt-dlp")).queryByRole("progressbar")).toBeNull();
    expect(card("yt-dlp").className).toContain("is-ready");

    await emit("tools://status", UPDATING);
    expect(card("yt-dlp").textContent).toMatch(/Updating · Starting…/);
  });
});

/**
 * No control writes the three path overrides, so the only way to lose one is a
 * save that rebuilt the object from the fields it knows about.
 */
it("carries hand-edited tool paths through a save untouched", async () => {
  backend.settings = {
    ...SETTINGS, ytdlp_path: "/opt/yt-dlp", ffmpeg_path: "C:\\ffmpeg\\bin\\ffmpeg.exe",
    deno_path: "/opt/deno",
  };
  await renderCards();
  fireEvent.click(screen.getByRole("radio", { name: "Stable" }));
  await waitFor(() => expect(api.saveSettings).toHaveBeenCalledTimes(1));
  expect(lastSaved()).toMatchObject({
    ytdlp_path: "/opt/yt-dlp", ffmpeg_path: "C:\\ffmpeg\\bin\\ffmpeg.exe", deno_path: "/opt/deno",
  });
});

describe("summary", () => {
  const ctx = (tools: ToolStatus[] | null, loadError: string | null = null) =>
    ({ tools: { tools, progress: {}, loadError, apply: () => {} } }) as unknown as SettingsCtx;

  it("says it is checking before the first read, and when the read failed", () => {
    expect(summary(ctx(null))).toEqual({ text: "Checking…" });
    expect(summary(ctx(null, "nope"))).toEqual({ text: "Status unavailable" });
  });

  it("lists the tools when every one is ready", () => {
    expect(summary(ctx(TOOLS_OK))).toEqual({ text: "yt-dlp, ffmpeg, deno · ready" });
  });

  it("names what is installing or updating", () => {
    expect(summary(ctx([{ ...TOOLS_OK[0], state: "updating" }, TOOLS_OK[1], TOOLS_OK[2]])).text)
      .toBe("Updating yt-dlp…");
    expect(summary(ctx([TOOLS_OK[0], tool({ kind: "ffmpeg", state: "installing" }), TOOLS_OK[2]])).text)
      .toBe("Installing ffmpeg…");
  });

  it("marks the rail when a tool is broken, before anything busy", () => {
    const line = summary(ctx([{ ...TOOLS_OK[0], state: "updating" }, TOOLS_OK[1], TOOLS_BROKEN[2]]));
    expect(line).toEqual({
      text: "deno failed to install",
      tone: "error",
      flag: { text: "!", tone: "error", label: "Needs attention" },
    });
  });

  it("counts several broken tools", () => {
    const broken = (t: ToolStatus): ToolStatus => ({ ...t, state: "error", error: "x" });
    expect(summary(ctx([broken(TOOLS_OK[0]), TOOLS_OK[1], broken(TOOLS_OK[2])])).text)
      .toBe("2 tools failed to install");
  });

  /** A working tool's note is not the rail's business. */
  it("stays calm for a working tool that carries an error", () => {
    const line = summary(ctx([{ ...TOOLS_OK[0], error: "Update check failed: HTTP 503" }, TOOLS_OK[1], TOOLS_OK[2]]));
    expect(line.tone).toBeUndefined();
    expect(line.text).toBe("yt-dlp, ffmpeg, deno · ready");
  });
});
