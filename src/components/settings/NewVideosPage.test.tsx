import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { screen, fireEvent, cleanup, waitFor, within } from "@testing-library/react";

vi.mock("@tauri-apps/api/event", async () => (await import("./testApi")).eventModule);
vi.mock("@tauri-apps/plugin-dialog", async () => (await import("./testApi")).dialogModule);
vi.mock("../../api", async () => (await import("./testApi")).apiModule);

import { api, backend, emit, lastSaved, resetHarness, SETTINGS } from "./testApi";
import { railItem, renderSettings } from "./testRender";
import {
  DEPTH_TICKS, SLIDER_STEPS, depthAfterKey, depthAt, depthPosition, intervalText,
  lastCheckedText, newestPoll, summary,
} from "./NewVideosPage";
import type { SettingsCtx } from "./context";
import type { Channel } from "../../types";

beforeEach(resetHarness);
afterEach(cleanup);

const channel = (id: string, last_polled_at: number | null): Channel => ({
  id, title: id, handle: null, url: `https://www.youtube.com/channel/${id}`, thumb_path: null,
  subscribed: true, member: false, auto_download: false, added_at: 1_700_000_000,
  last_polled_at, terminated: false,
});

const nowSecs = () => Math.floor(Date.now() / 1000);
const radio = (name: string) => screen.getByRole("radio", { name }) as HTMLInputElement;
const minutesField = () => screen.queryByLabelText("Minutes between checks") as HTMLInputElement | null;
const slider = () =>
  screen.getByRole("slider", { name: "How far back each channel is read" }) as HTMLInputElement;
/** Drags the slider to a count, as a browser reports each step of a drag:
 *  `input` events, no release. */
const dragTo = (n: number) =>
  fireEvent.input(slider(), { target: { value: String(Math.round(depthPosition(n) * SLIDER_STEPS)) } });
const lineOf = (label: string) =>
  document.getElementById(railItem(label).getAttribute("aria-describedby")!)!.textContent;

/** The New videos page, once its "Last checked" read has answered. */
async function renderPage() {
  await renderSettings("new-videos");
  await waitFor(() => expect(api.listChannels).toHaveBeenCalled());
}

/** Waits for the `n`th write to land -- the shell marks "Saved" once it has --
 *  and returns what it wrote. */
async function saved(n = 1) {
  await waitFor(() => expect(api.saveSettings).toHaveBeenCalledTimes(n));
  return lastSaved();
}

describe("New videos · check every", () => {
  it("offers the presets and Custom, with the stored one picked", async () => {
    await renderPage();
    const group = screen.getByRole("radiogroup", { name: "Check every" });
    expect(within(group).getAllByRole("radio").map((r) => r.closest("label")!.textContent)).toEqual([
      "15 min", "30 min", "1 hour", "3 hours", "6 hours", "12 hours", "Custom…",
    ]);
    expect(radio("30 min").checked).toBe(true);
    expect(minutesField()).toBeNull();
  });

  it("saves a preset the moment it is picked", async () => {
    await renderPage();
    fireEvent.click(radio("3 hours"));
    expect((await saved()).poll_interval_minutes).toBe(180);
    expect(radio("3 hours").checked).toBe(true);
    expect(lineOf("New videos")).toBe("Every 3 h · 30 back");
  });

  it("opens a minutes field for Custom, holding what is typed until blur", async () => {
    await renderPage();
    fireEvent.click(radio("Custom…"));
    expect(radio("Custom…").checked).toBe(true);
    const field = minutesField()!;
    expect(field.value).toBe("30");

    fireEvent.change(field, { target: { value: "45" } });
    expect(api.saveSettings).not.toHaveBeenCalled();
    fireEvent.blur(field);
    expect((await saved()).poll_interval_minutes).toBe(45);
    expect(minutesField()!.value).toBe("45");
  });

  it("keeps Custom open when the minutes typed match a preset", async () => {
    await renderPage();
    fireEvent.click(radio("Custom…"));
    fireEvent.change(minutesField()!, { target: { value: "60" } });
    fireEvent.blur(minutesField()!);
    expect((await saved()).poll_interval_minutes).toBe(60);
    expect(radio("Custom…").checked).toBe(true);
    expect(minutesField()).not.toBeNull();
  });

  it("commits on Enter", async () => {
    await renderPage();
    fireEvent.click(radio("Custom…"));
    fireEvent.change(minutesField()!, { target: { value: "90" } });
    fireEvent.keyDown(minutesField()!, { key: "Enter" });
    expect((await saved()).poll_interval_minutes).toBe(90);
  });

  it("clamps to 1 – 1440, and takes the minimum for nothing parsable", async () => {
    await renderPage();
    fireEvent.click(radio("Custom…"));
    fireEvent.change(minutesField()!, { target: { value: "5000" } });
    fireEvent.blur(minutesField()!);
    expect((await saved(1)).poll_interval_minutes).toBe(1440);
    expect(minutesField()!.value).toBe("1440");

    fireEvent.change(minutesField()!, { target: { value: "0" } });
    fireEvent.blur(minutesField()!);
    expect((await saved(2)).poll_interval_minutes).toBe(1);

    fireEvent.change(minutesField()!, { target: { value: "120" } });
    fireEvent.blur(minutesField()!);
    await saved(3);
    // A number input reports anything unparsable as empty.
    fireEvent.change(minutesField()!, { target: { value: "" } });
    fireEvent.blur(minutesField()!);
    expect((await saved(4)).poll_interval_minutes).toBe(1);
    expect(minutesField()!.value).toBe("1");
  });

  it("writes nothing for a blur that changed nothing", async () => {
    await renderPage();
    fireEvent.click(radio("Custom…"));
    fireEvent.focus(minutesField()!);
    fireEvent.blur(minutesField()!);
    await waitFor(() => {});
    expect(api.saveSettings).not.toHaveBeenCalled();
  });

  it("shows a stored value no preset matches as Custom, with its field open", async () => {
    backend.settings = { ...SETTINGS, poll_interval_minutes: 45 };
    await renderPage();
    expect(radio("Custom…").checked).toBe(true);
    expect(minutesField()!.value).toBe("45");
  });

  it("closes the field when a preset is picked from Custom", async () => {
    backend.settings = { ...SETTINGS, poll_interval_minutes: 45 };
    await renderPage();
    fireEvent.click(radio("12 hours"));
    expect((await saved()).poll_interval_minutes).toBe(720);
    expect(minutesField()).toBeNull();
  });
});

describe("New videos · last checked", () => {
  it("reads the newest poll across the channels", async () => {
    backend.channels = [
      channel("UCa", nowSecs() - 3 * 3600),
      channel("UCb", nowSecs() - 18 * 60 - 5),
      channel("UCc", null),
    ];
    await renderSettings("new-videos");
    expect(await screen.findByText("Last checked 18 minutes ago")).toBeTruthy();
  });

  it("reads it again when a poll finishes", async () => {
    backend.channels = [channel("UCa", nowSecs() - 2 * 3600 - 10)];
    await renderSettings("new-videos");
    await screen.findByText("Last checked 2 hours ago");
    backend.channels = [channel("UCa", nowSecs())];
    await emit("poll://finished", { channels: 1, newVideos: 0, errors: [] });
    expect(await screen.findByText("Last checked just now")).toBeTruthy();
    expect(api.listChannels).toHaveBeenCalledTimes(2);
  });

  it("says nothing when no channel was ever polled", async () => {
    backend.channels = [channel("UCa", null)];
    await renderPage();
    expect(screen.queryByText(/Last checked/)).toBeNull();
  });

  it("says nothing when the channels cannot be read", async () => {
    api.listChannels.mockImplementation(() => Promise.reject("database is locked"));
    await renderPage();
    expect(screen.queryByText(/Last checked/)).toBeNull();
  });

  it("phrases the age in words", () => {
    const now = 1_790_000_000;
    expect(lastCheckedText(now - 20, now)).toBe("Last checked just now");
    expect(lastCheckedText(now - 60, now)).toBe("Last checked 1 minute ago");
    expect(lastCheckedText(now - 18 * 60, now)).toBe("Last checked 18 minutes ago");
    expect(lastCheckedText(now - 3600, now)).toBe("Last checked 1 hour ago");
    expect(lastCheckedText(now - 5 * 3600 - 59, now)).toBe("Last checked 5 hours ago");
    expect(lastCheckedText(now - 3 * 86400, now)).toBe("Last checked 3 days ago");
    // A poll stamped after the clock last ticked is not in the future.
    expect(lastCheckedText(now + 30, now)).toBe("Last checked just now");
  });

  it("takes the newest stamp and ignores channels never polled", () => {
    expect(newestPoll([])).toBeNull();
    expect(newestPoll([channel("a", null)])).toBeNull();
    expect(newestPoll([channel("a", 10), channel("b", null), channel("c", 30), channel("d", 20)]))
      .toBe(30);
  });
});

describe("New videos · check at startup", () => {
  it("saves the switch at once", async () => {
    await renderPage();
    const sw = screen.getByRole("switch", { name: "Also check when MyTube starts" }) as HTMLInputElement;
    expect(sw.checked).toBe(true);
    fireEvent.click(sw);
    expect(sw.checked).toBe(false);
    expect((await saved()).poll_on_startup).toBe(false);
  });
});

describe("New videos · how far back", () => {
  it("steps one video at a time, saving each press", async () => {
    await renderPage();
    fireEvent.click(screen.getByRole("button", { name: "Increase videos per channel" }));
    expect((await saved(1)).backfill_count).toBe(31);
    fireEvent.click(screen.getByRole("button", { name: "Decrease videos per channel" }));
    fireEvent.click(screen.getByRole("button", { name: "Decrease videos per channel" }));
    // A burst of presses: the newest value is what lands, however many writes
    // the queue needed for it.
    await waitFor(() => expect(lastSaved().backfill_count).toBe(29));
  });

  it("stops the stepper at 500", async () => {
    backend.settings = { ...SETTINGS, backfill_count: 500 };
    await renderPage();
    const up = screen.getByRole("button", { name: "Increase videos per channel" }) as HTMLButtonElement;
    expect(up.disabled).toBe(true);
  });

  it("stops the stepper at 1, and says a single video in the singular", async () => {
    backend.settings = { ...SETTINGS, backfill_count: 1 };
    await renderPage();
    const down = screen.getByRole("button", { name: "Decrease videos per channel" }) as HTMLButtonElement;
    expect(down.disabled).toBe(true);
    expect(screen.getByText("most recent video")).toBeTruthy();
    expect(screen.getByText("starts with its most recent upload.")).toBeTruthy();
  });

  it("follows a drag live and saves once, on release", async () => {
    await renderPage();
    dragTo(60);
    dragTo(80);
    dragTo(100);
    expect(api.saveSettings).not.toHaveBeenCalled();
    expect(slider().getAttribute("aria-valuetext")).toBe("100 videos");
    expect(screen.getByText("starts with its 100 most recent uploads.")).toBeTruthy();
    expect(lineOf("New videos")).toBe("Every 30 min · 100 back");

    fireEvent.pointerUp(slider());
    // Every other way a browser reports letting go follows at once; none of
    // them is a second write.
    fireEvent.change(slider());
    fireEvent.blur(slider());
    expect((await saved(1)).backfill_count).toBe(100);
    await waitFor(() => {});
    expect(api.saveSettings).toHaveBeenCalledTimes(1);
  });

  it("writes nothing for a click on the thumb that moved nothing", async () => {
    await renderPage();
    fireEvent.pointerUp(slider());
    fireEvent.blur(slider());
    await waitFor(() => {});
    expect(api.saveSettings).not.toHaveBeenCalled();
  });

  it("saves on the native change a browser fires when a drag ends off the slider", async () => {
    await renderPage();
    dragTo(250);
    // A release away from the input never reaches its pointerup; the native
    // `change` (not React's onChange, which is `input`) is sent regardless.
    fireEvent(slider(), new Event("change", { bubbles: true }));
    expect((await saved()).backfill_count).toBe(250);
  });

  it("moves a whole video per arrow key, saving when the key comes up", async () => {
    backend.settings = { ...SETTINGS, backfill_count: 1 };
    await renderPage();
    fireEvent.keyDown(slider(), { key: "ArrowRight" });
    fireEvent.keyDown(slider(), { key: "ArrowRight" });
    expect(slider().getAttribute("aria-valuetext")).toBe("3 videos");
    expect(api.saveSettings).not.toHaveBeenCalled();
    fireEvent.keyUp(slider(), { key: "ArrowRight" });
    expect((await saved(1)).backfill_count).toBe(3);
    fireEvent.keyDown(slider(), { key: "End" });
    fireEvent.keyUp(slider(), { key: "End" });
    expect((await saved(2)).backfill_count).toBe(500);
  });

  it("puts the count into the new-channel tile", async () => {
    await renderPage();
    expect(screen.getByText("starts with its 30 most recent uploads.")).toBeTruthy();
    expect(screen.getByText("reads every channel deeper once, on the next check.")).toBeTruthy();
    expect(screen.getByText("removes nothing already in your library.")).toBeTruthy();
  });
});

describe("the square-root scale", () => {
  it("spans 1 to 500", () => {
    expect(depthAt(0)).toBe(1);
    expect(depthAt(1)).toBe(500);
    expect(depthAt(-0.5)).toBe(1);
    expect(depthAt(2)).toBe(500);
    expect(depthPosition(1)).toBe(0);
    expect(depthPosition(500)).toBe(1);
    expect(depthPosition(9999)).toBe(1);
  });

  it("gives 10 – 100, the range most people want, the first half of the track", () => {
    expect(depthAt(0.5)).toBe(126);
    expect(depthPosition(100)).toBeCloseTo(0.445, 3);
    expect(depthPosition(10)).toBeLessThan(0.15);
  });

  it("gives every count from 1 to 500 a position of its own on the native range", () => {
    for (let n = 1; n <= 500; n++) {
      const pos = Math.round(depthPosition(n) * SLIDER_STEPS);
      expect(depthAt(pos / SLIDER_STEPS)).toBe(n);
    }
  });

  it("labels the scale where the mockup does", () => {
    expect([...DEPTH_TICKS]).toEqual([1, 30, 100, 250, 500]);
    expect(DEPTH_TICKS.map((t) => Math.round(depthPosition(t) * 100))).toEqual([0, 24, 45, 71, 100]);
  });

  it("steps by whole videos from the keyboard, inside the bounds", () => {
    expect(depthAfterKey(30, "ArrowRight")).toBe(31);
    expect(depthAfterKey(30, "ArrowUp")).toBe(31);
    expect(depthAfterKey(30, "ArrowLeft")).toBe(29);
    expect(depthAfterKey(30, "PageUp")).toBe(40);
    expect(depthAfterKey(5, "PageDown")).toBe(1);
    expect(depthAfterKey(498, "PageUp")).toBe(500);
    expect(depthAfterKey(1, "ArrowDown")).toBe(1);
    expect(depthAfterKey(30, "Home")).toBe(1);
    expect(depthAfterKey(30, "End")).toBe(500);
    expect(depthAfterKey(30, "Tab")).toBeNull();
  });
});

describe("New videos · rail line", () => {
  const ctx = (over: Partial<typeof SETTINGS>) => ({ settings: { ...SETTINGS, ...over } }) as SettingsCtx;

  it("names the interval and the depth", () => {
    expect(summary(ctx({}))).toEqual({ text: "Every 30 min · 30 back" });
    expect(summary(ctx({ poll_interval_minutes: 120, backfill_count: 250 })))
      .toEqual({ text: "Every 2 h · 250 back" });
    expect(summary(ctx({ poll_interval_minutes: 1440 })).text).toBe("Every 24 h · 30 back");
  });

  it("keeps minutes for an interval that is not whole hours", () => {
    expect(intervalText(15)).toBe("15 min");
    expect(intervalText(90)).toBe("90 min");
    expect(intervalText(60)).toBe("1 h");
    expect(intervalText(720)).toBe("12 h");
  });
});
