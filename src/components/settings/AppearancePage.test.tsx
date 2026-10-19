import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, cleanup, waitFor, within } from "@testing-library/react";

vi.mock("@tauri-apps/api/event", async () => (await import("./testApi")).eventModule);
vi.mock("@tauri-apps/plugin-dialog", async () => (await import("./testApi")).dialogModule);
vi.mock("../../api", async () => (await import("./testApi")).apiModule);

import { api, backend, lastSaved, resetHarness, SETTINGS } from "./testApi";
import { railItem, renderSettings } from "./testRender";
import { scaleAfterKey, summary } from "./AppearancePage";
import type { SettingsCtx } from "./context";
import SettingsView from "../SettingsView";
import { ToastProvider } from "../Toast";

beforeEach(() => {
  resetHarness();
  document.documentElement.style.removeProperty("--text-scale");
});
afterEach(cleanup);

const scale = () => document.documentElement.style.getPropertyValue("--text-scale");
const slider = () => screen.getByRole("slider", { name: "Text size" }) as HTMLInputElement;
const resetButton = () => screen.getByRole("button", { name: "Reset" }) as HTMLButtonElement;
/** Drags the slider to a size, as a browser reports each step of a drag:
 *  `input` events, no release. */
const dragTo = (pct: number) => fireEvent.input(slider(), { target: { value: String(pct) } });
const lineOf = (label: string) =>
  document.getElementById(railItem(label).getAttribute("aria-describedby")!)!.textContent;
const page = () => screen.getByRole("region", { name: "Appearance" });

/** Waits for the `n`th write to land and returns what it wrote. */
async function saved(n = 1) {
  await waitFor(() => expect(api.saveSettings).toHaveBeenCalledTimes(n));
  return lastSaved();
}

describe("Appearance · text size", () => {
  it("shows the stored size on the slider and in words", async () => {
    backend.settings = { ...SETTINGS, text_scale: 130 };
    await renderSettings("appearance");
    expect(slider().value).toBe("130");
    expect(slider().min).toBe("80");
    expect(slider().max).toBe("200");
    expect(slider().step).toBe("10");
    expect(slider().getAttribute("aria-valuetext")).toBe("130%");
    expect(within(page()).getByText("130%")).toBeTruthy();
    // The ends of the scale, labelled.
    expect(within(page()).getByText("80%")).toBeTruthy();
    expect(within(page()).getByText("200%")).toBeTruthy();
  });

  it("says what it scales, and what it leaves alone", async () => {
    await renderSettings("appearance");
    const hint = slider().getAttribute("aria-describedby");
    expect(document.getElementById(hint!)!.textContent).toMatch(
      /icons, thumbnails and spacing stay as they are.*Ctrl\+scroll/,
    );
  });

  it("applies a drag to the whole app as it goes, and saves nothing yet", async () => {
    await renderSettings("appearance");
    dragTo(110);
    expect(scale()).toBe("1.1");
    dragTo(130);
    dragTo(150);
    expect(scale()).toBe("1.5");
    expect(slider().getAttribute("aria-valuetext")).toBe("150%");
    expect(lineOf("Appearance")).toBe("Text size 150%");
    expect(api.saveSettings).not.toHaveBeenCalled();
  });

  it("saves once, on release, with where the drag ended", async () => {
    await renderSettings("appearance");
    dragTo(140);
    dragTo(160);
    fireEvent.pointerUp(slider());
    // Every other way a browser reports letting go follows at once; none of
    // them is a second write.
    fireEvent(slider(), new Event("change", { bubbles: true }));
    fireEvent.blur(slider());
    expect((await saved(1)).text_scale).toBe(160);
    await waitFor(() => {});
    expect(api.saveSettings).toHaveBeenCalledTimes(1);
    // The rest of the file rides along untouched.
    expect(lastSaved().player_command).toBe(SETTINGS.player_command);
  });

  it("saves on the native change a browser fires when a drag ends off the slider", async () => {
    await renderSettings("appearance");
    dragTo(180);
    fireEvent(slider(), new Event("change", { bubbles: true }));
    expect((await saved()).text_scale).toBe(180);
  });

  it("writes nothing for a click on the thumb that moved nothing", async () => {
    await renderSettings("appearance");
    fireEvent.pointerUp(slider());
    fireEvent.blur(slider());
    await waitFor(() => {});
    expect(api.saveSettings).not.toHaveBeenCalled();
  });

  it("steps 10% per arrow key, applying each step, saving when the key comes up", async () => {
    await renderSettings("appearance");
    fireEvent.keyDown(slider(), { key: "ArrowRight" });
    expect(scale()).toBe("1.1");
    fireEvent.keyDown(slider(), { key: "ArrowUp" });
    expect(scale()).toBe("1.2");
    expect(slider().getAttribute("aria-valuetext")).toBe("120%");
    expect(api.saveSettings).not.toHaveBeenCalled();
    fireEvent.keyUp(slider(), { key: "ArrowUp" });
    expect((await saved(1)).text_scale).toBe(120);

    fireEvent.keyDown(slider(), { key: "Home" });
    expect(scale()).toBe("0.8");
    fireEvent.keyUp(slider(), { key: "Home" });
    expect((await saved(2)).text_scale).toBe(80);
  });

  it("goes back to 100% with Reset, at once and saved", async () => {
    backend.settings = { ...SETTINGS, text_scale: 150 };
    await renderSettings("appearance");
    expect(resetButton().disabled).toBe(false);
    fireEvent.click(resetButton());
    expect(scale()).toBe("1");
    expect((await saved()).text_scale).toBe(100);
    expect(slider().value).toBe("100");
    expect(resetButton().disabled).toBe(true);
  });

  it("offers no Reset at 100%", async () => {
    await renderSettings("appearance");
    expect(resetButton().disabled).toBe(true);
  });

  it("offers Reset as soon as a drag leaves 100%", async () => {
    await renderSettings("appearance");
    dragTo(90);
    expect(resetButton().disabled).toBe(false);
  });
});

describe("Appearance · show stats", () => {
  const statsSwitch = () => screen.getByRole("switch", { name: "Show stats" }) as HTMLInputElement;

  it("is off by default, and says what it shows", async () => {
    await renderSettings("appearance");
    expect(statsSwitch().checked).toBe(false);
    const hint = statsSwitch().getAttribute("aria-describedby");
    expect(document.getElementById(hint!)!.textContent).toMatch(/Subscriptions feed.*series/);
  });

  it("shows a stored on", async () => {
    backend.settings = { ...SETTINGS, show_stats: true };
    await renderSettings("appearance");
    expect(statsSwitch().checked).toBe(true);
  });

  it("saves at once when flipped on, and again when flipped back off", async () => {
    await renderSettings("appearance");
    fireEvent.click(statsSwitch());
    expect((await saved(1)).show_stats).toBe(true);
    expect(statsSwitch().checked).toBe(true);
    // The rest of the file rides along untouched.
    expect(lastSaved().text_scale).toBe(SETTINGS.text_scale);
    expect(lastSaved().player_command).toBe(SETTINGS.player_command);

    fireEvent.click(statsSwitch());
    expect((await saved(2)).show_stats).toBe(false);
    expect(statsSwitch().checked).toBe(false);
  });

  it("says so in the rail while they are on", async () => {
    await renderSettings("appearance");
    expect(lineOf("Appearance")).toBe("Text size 100%");
    fireEvent.click(statsSwitch());
    expect(lineOf("Appearance")).toBe("Text size 100% · stats on");
    await saved();
  });

  /* The shell follows the switch through this: Settings is unmounted by the
     time the feed is back on screen. */
  it("hands every save that landed to the shell, and none that failed", async () => {
    const onSaved = vi.fn();
    render(
      <ToastProvider>
        <SettingsView initialSection="appearance" onSaved={onSaved} />
      </ToastProvider>,
    );
    fireEvent.click(await screen.findByRole("switch", { name: "Show stats" }));
    await waitFor(() => expect(onSaved).toHaveBeenCalledTimes(1));
    expect(onSaved.mock.calls[0][0]).toMatchObject({ show_stats: true, text_scale: 100 });

    api.saveSettings.mockImplementationOnce(() => Promise.reject("disk full"));
    fireEvent.click(statsSwitch());
    expect(await screen.findByText("disk full")).toBeTruthy();
    expect(onSaved).toHaveBeenCalledTimes(1);
  });
});

describe("Appearance · keys", () => {
  it("steps by 10, pages by 50, and jumps to either end", () => {
    expect(scaleAfterKey(100, "ArrowRight")).toBe(110);
    expect(scaleAfterKey(100, "ArrowUp")).toBe(110);
    expect(scaleAfterKey(100, "ArrowLeft")).toBe(90);
    expect(scaleAfterKey(100, "ArrowDown")).toBe(90);
    expect(scaleAfterKey(100, "PageUp")).toBe(150);
    expect(scaleAfterKey(100, "PageDown")).toBe(80);
    expect(scaleAfterKey(170, "PageUp")).toBe(200);
    expect(scaleAfterKey(80, "ArrowLeft")).toBe(80);
    expect(scaleAfterKey(200, "ArrowRight")).toBe(200);
    expect(scaleAfterKey(130, "Home")).toBe(80);
    expect(scaleAfterKey(130, "End")).toBe(200);
    expect(scaleAfterKey(130, "Tab")).toBeNull();
  });
});

describe("Appearance · rail", () => {
  const ctx = (text_scale: number) => ({ settings: { ...SETTINGS, text_scale } }) as SettingsCtx;

  it("names the text size", () => {
    expect(summary(ctx(125))).toEqual({ text: "Text size 130%" });
    expect(summary(ctx(100))).toEqual({ text: "Text size 100%" });
    expect(summary(ctx(200))).toEqual({ text: "Text size 200%" });
  });

  it("adds the stats while they are on", () => {
    const on = { settings: { ...SETTINGS, text_scale: 120, show_stats: true } } as SettingsCtx;
    expect(summary(on)).toEqual({ text: "Text size 120% · stats on" });
  });

  it("reads the stored size in the rail", async () => {
    backend.settings = { ...SETTINGS, text_scale: 120 };
    await renderSettings();
    expect(lineOf("Appearance")).toBe("Text size 120%");
  });

  it("comes first under the hood, before Tools", async () => {
    await renderSettings();
    const nav = screen.getByRole("navigation", { name: "Settings sections" });
    const names = within(nav).getAllByRole("button").map((b) => b.getAttribute("aria-label"));
    expect(names.indexOf("Appearance")).toBe(names.indexOf("Tools") - 1);
    expect(names.indexOf("Appearance")).toBe(names.indexOf("Backup & transfer") + 1);
    // The tier's heading sits over its first entry.
    const group = railItem("Appearance").closest(".st-rail-group")!;
    expect(group.querySelector(".st-tier")!.textContent).toBe("Under the hood");
  });

  it("opens the page from the rail", async () => {
    await renderSettings();
    fireEvent.click(railItem("Appearance"));
    expect(screen.getByRole("heading", { level: 2, name: "Appearance" })).toBeTruthy();
    expect(slider()).toBeTruthy();
  });
});
