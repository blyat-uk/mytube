import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, cleanup, waitFor } from "@testing-library/react";

vi.mock("@tauri-apps/api/event", async () => (await import("./testApi")).eventModule);
vi.mock("@tauri-apps/plugin-dialog", async () => (await import("./testApi")).dialogModule);
vi.mock("../../api", async () => (await import("./testApi")).apiModule);

import { api, backend, lastSaved, PLAYERS, resetHarness, SETTINGS } from "./testApi";
import { railItem, renderSettings } from "./testRender";
import PlayerPage, { summary } from "./PlayerPage";
import { SettingsContext, type SettingsCtx } from "./context";
import type { PlayerOption, Settings } from "../../types";

beforeEach(resetHarness);
afterEach(cleanup);

/** The Player page, once detection has filled the list. */
async function renderPlayer() {
  await renderSettings("player");
  await screen.findByRole("radiogroup", { name: "Play downloads with" });
}

const radio = (name: RegExp) => screen.getByRole("radio", { name }) as HTMLInputElement;
const field = () => screen.queryByLabelText("Player command") as HTMLInputElement | null;
const titles = () =>
  [...document.querySelectorAll(".st-pane .st-choice-title")].map((t) => t.textContent);
const checked = () =>
  (document.querySelector(".st-pane .st-choice.is-on .st-choice-title") as HTMLElement).textContent;

describe("Player", () => {
  it("offers every detected player, and Custom for a command none of them is", async () => {
    await renderPlayer();
    expect(screen.getByRole("heading", { level: 2, name: "Player" })).toBeTruthy();
    expect(titles()).toEqual(["System default", "mpv", "VLC", "Custom command…"]);
    expect(radio(/^Custom command/).checked).toBe(true);
    expect(field()!.value).toBe("smplayer");
  });

  it("shows a detected player as itself, with no text field", async () => {
    backend.settings = { ...SETTINGS, player_command: "vlc" };
    await renderPlayer();
    expect(radio(/^VLC/).checked).toBe(true);
    expect(radio(/^Custom command/).checked).toBe(false);
    expect(field()).toBeNull();
  });

  it("reads an empty command as the system default", async () => {
    backend.settings = { ...SETTINGS, player_command: "" };
    await renderPlayer();
    expect(radio(/^System default/).checked).toBe(true);
    expect(field()).toBeNull();
  });

  it("commits a detected player the moment it is chosen", async () => {
    await renderPlayer();
    fireEvent.click(radio(/^mpv/));
    await waitFor(() => expect(api.saveSettings).toHaveBeenCalledTimes(1));
    expect(lastSaved().player_command).toBe("mpv");
    expect(field()).toBeNull();
  });

  it("keeps a custom command on blur, like every other text field", async () => {
    await renderPlayer();
    expect(radio(/^Custom command/).checked).toBe(true);
    const custom = field()!;
    fireEvent.change(custom, { target: { value: "mpv --fullscreen" } });
    expect(api.saveSettings).not.toHaveBeenCalled();
    fireEvent.blur(custom);
    await waitFor(() => expect(api.saveSettings).toHaveBeenCalledTimes(1));
    expect(lastSaved().player_command).toBe("mpv --fullscreen");
  });

  /** Choosing Custom over a detected player must open the field without
   *  snapping straight back to the player whose command it still holds. */
  it("opens the text field when Custom is chosen over a detected player, focused", async () => {
    backend.settings = { ...SETTINGS, player_command: "vlc" };
    await renderPlayer();
    fireEvent.click(radio(/^Custom command/));
    expect(radio(/^Custom command/).checked).toBe(true);
    expect(radio(/^VLC/).checked).toBe(false);
    expect(field()!.value).toBe("vlc");
    expect(document.activeElement).toBe(field());
    expect(api.saveSettings).not.toHaveBeenCalled();
  });

  /** Typing that happens to spell a detected command must not swap the field
   *  for that player: the input would unmount mid-edit, never fire its blur,
   *  and the list would show a choice settings.json does not hold. */
  it.each([
    ["backspaced to a detected player", "mpv"],
    ["cleared to the system default", ""],
  ])("keeps the text field when the command is %s, and saves it on blur", async (_, typed) => {
    backend.settings = { ...SETTINGS, player_command: "mpv --fullscreen" };
    await renderPlayer();
    const custom = field()!;
    fireEvent.focus(custom);
    fireEvent.change(custom, { target: { value: typed } });

    const still = field()!;
    expect(still).toBe(custom);
    expect(still.value).toBe(typed);
    expect(radio(/^Custom command/).checked).toBe(true);
    expect(checked()).toBe("Custom command…");

    fireEvent.blur(still);
    await waitFor(() => expect(api.saveSettings).toHaveBeenCalledTimes(1));
    expect(lastSaved().player_command).toBe(typed);
  });

  it("closes the text field again once a player is picked from the list", async () => {
    await renderPlayer();
    fireEvent.click(radio(/^VLC/));
    await waitFor(() => expect(api.saveSettings).toHaveBeenCalledTimes(1));
    expect(radio(/^VLC/).checked).toBe(true);
    expect(field()).toBeNull();
  });

  it("links each player's command, and the field's hint, for a screen reader", async () => {
    backend.settings = { ...SETTINGS, player_command: "vlc" };
    await renderPlayer();
    const described = (el: HTMLElement) =>
      document.getElementById(el.getAttribute("aria-describedby")!)?.textContent;
    expect(described(radio(/^VLC/))).toBe("vlc");
    expect(described(radio(/^System default/)))
      .toBe("Opens each file in the app your system uses for it");
    fireEvent.click(radio(/^Custom command/));
    expect(described(field()!))
      .toBe("Runs to open a downloaded file. Arguments are allowed, e.g. mpv --fullscreen.");
    expect(field()!.placeholder).toBe("mpv --fullscreen");
  });

  it("says it is looking while detection runs, and offers nothing to pick yet", async () => {
    api.detectPlayers.mockImplementation(() => new Promise(() => {}));
    await renderSettings("player");
    expect(screen.getByText("Looking for players…")).toBeTruthy();
    expect(screen.queryAllByRole("radio")).toHaveLength(0);
    expect(field()).toBeNull();
  });

  it("still offers the system default and Custom when detection fails", async () => {
    api.detectPlayers.mockImplementation(() => Promise.reject("no"));
    await renderPlayer();
    expect(titles()).toEqual(["System default", "Custom command…"]);
    expect(field()!.value).toBe("smplayer");
  });

  it("names the player in the rail, and follows a pick", async () => {
    await renderPlayer();
    const line = () =>
      document.getElementById(railItem("Player").getAttribute("aria-describedby")!)!.textContent;
    expect(line()).toBe("smplayer");
    fireEvent.click(radio(/^VLC/));
    expect(line()).toBe("VLC");
    fireEvent.click(radio(/^System default/));
    expect(line()).toBe("System default");
    await waitFor(() => expect(lastSaved().player_command).toBe(""));
  });
});

describe("Player, between a pick and its value", () => {
  /** A pick's own state change can render before the new command arrives
   *  through settings. The page must not read that in-between render -- old
   *  custom value, nothing focused -- as a reason to keep the field open.
   *  The real settings document hands the value back in the same render, so
   *  this drives the page through a context of its own that does not. */
  it("closes the text field after a pick even when the new value arrives a render later", () => {
    const save = vi.fn();
    const ctx = (player_command: string) => ({
      settings: { ...SETTINGS, player_command } as Settings,
      players: PLAYERS as PlayerOption[] | null,
      set: vi.fn(), save, flush: vi.fn(), patch: vi.fn(),
    }) as unknown as SettingsCtx;
    const page = (value: string) => (
      <SettingsContext.Provider value={ctx(value)}><PlayerPage /></SettingsContext.Provider>
    );

    const { rerender } = render(page("smplayer"));
    expect(field()).toBeTruthy();

    fireEvent.click(radio(/^mpv/));
    expect(save).toHaveBeenCalledWith("player_command", "mpv");
    rerender(page("mpv"));

    expect(radio(/^mpv/).checked).toBe(true);
    expect(field()).toBeNull();
  });
});

describe("the player rail line", () => {
  const ctx = (player_command: string, players: PlayerOption[] | null = PLAYERS) =>
    ({ settings: { ...SETTINGS, player_command }, players }) as unknown as SettingsCtx;

  it("names a detected player, the system default, or the command itself", () => {
    expect(summary(ctx("vlc"))).toEqual({ text: "VLC" });
    expect(summary(ctx(""))).toEqual({ text: "System default" });
    expect(summary(ctx("mpv --fullscreen"))).toEqual({ text: "mpv --fullscreen" });
  });

  it("says the command while detection runs", () => {
    expect(summary(ctx("vlc", null))).toEqual({ text: "vlc" });
    expect(summary(ctx("", null))).toEqual({ text: "System default" });
  });
});
