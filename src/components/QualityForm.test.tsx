import { describe, it, expect, vi, afterEach } from "vitest";
import { render, screen, fireEvent, cleanup } from "@testing-library/react";
import { useState } from "react";
import QualityForm from "./QualityForm";
import { DEFAULT_QUALITY, initialPick, resolveExact, type ExactPick } from "../quality";
import { MB, a, rickroll } from "../quality.fixture";
import type { Quality, VideoFormats } from "../types";

afterEach(cleanup);

const select = (label: string) => screen.getByLabelText(label) as HTMLSelectElement;
const shown = (el: HTMLSelectElement) => el.options[el.selectedIndex].text;
const optionTexts = (el: HTMLSelectElement) => Array.from(el.options).map((o) => o.text);
const choose = (el: HTMLSelectElement, text: string) => {
  const i = optionTexts(el).indexOf(text);
  if (i < 0) throw new Error(`no option "${text}" in: ${optionTexts(el).join(", ")}`);
  fireEvent.change(el, { target: { value: String(i) } });
};

describe("the generic form", () => {
  function renderGeneric(start: Quality = DEFAULT_QUALITY) {
    const onPick = vi.fn();
    const onEdit = vi.fn();
    const onBlur = vi.fn();
    function Host() {
      const [q, setQ] = useState(start);
      return (
        <QualityForm
          source="generic" value={q}
          onPick={(n) => { setQ(n); onPick(n); }}
          onEdit={(n) => { setQ(n); onEdit(n); }}
          onBlur={onBlur}
        />
      );
    }
    render(<Host />);
    return { onPick, onEdit, onBlur };
  }

  it("shows the defaults", () => {
    renderGeneric();
    expect(shown(select("Mode"))).toBe("Video");
    expect(shown(select("Resolution"))).toBe("Best available");
    expect(shown(select("Codec"))).toBe("No preference");
    expect(shown(select("Frame rate"))).toBe("No preference");
    expect(shown(select("Container"))).toBe("MKV");
    expect(screen.queryByLabelText("Audio format")).toBeNull();
  });

  it("hands a picked select straight back as a whole Quality", () => {
    const { onPick } = renderGeneric();
    choose(select("Resolution"), "Up to 1080p");
    expect(onPick).toHaveBeenLastCalledWith({ ...DEFAULT_QUALITY, max_height: 1080 });
    choose(select("Codec"), "H.264 (plays everywhere)");
    expect(onPick).toHaveBeenLastCalledWith({ ...DEFAULT_QUALITY, max_height: 1080, vcodec: "h264" });
  });

  it("swaps the video selects for the audio format in audio mode", () => {
    const { onPick } = renderGeneric();
    choose(select("Mode"), "Audio only");
    expect(screen.queryByLabelText("Resolution")).toBeNull();
    choose(select("Audio format"), "MP3");
    expect(onPick).toHaveBeenLastCalledWith({ ...DEFAULT_QUALITY, mode: "audio", audio_format: "mp3" });
  });

  it("holds raw typing until blur, and disables the selects it overrides", () => {
    const { onEdit, onPick, onBlur } = renderGeneric();
    const raw = screen.getByLabelText("Format (-f)");
    fireEvent.change(raw, { target: { value: "137+140" } });
    expect(onEdit).toHaveBeenLastCalledWith({ ...DEFAULT_QUALITY, format: "137+140" });
    expect(onPick).not.toHaveBeenCalled();
    fireEvent.blur(raw);
    expect(onBlur).toHaveBeenCalled();

    expect(select("Resolution").disabled).toBe(true);
    expect(select("Codec").disabled).toBe(true);
    expect(select("Frame rate").disabled).toBe(true);
    // Merging and conversion still apply to whatever the raw format picks.
    expect(select("Container").disabled).toBe(false);
    expect(select("Mode").disabled).toBe(false);
  });

  it("opens Advanced by itself when a raw format is already set", () => {
    renderGeneric({ ...DEFAULT_QUALITY, format: "bv+ba" });
    expect((document.querySelector("details.quality-advanced") as HTMLDetailsElement).open).toBe(true);
  });
});

describe("the exact form", () => {
  function renderExact(f: VideoFormats = rickroll(), base: Quality = DEFAULT_QUALITY) {
    let latest: ExactPick = initialPick(f, base);
    function Host() {
      const [pick, setPick] = useState(latest);
      return (
        <QualityForm
          source="exact" formats={f} value={pick}
          onChange={(n) => { latest = n; setPick(n); }}
        />
      );
    }
    render(<Host />);
    return { quality: () => resolveExact(f, latest) };
  }

  it("starts on the Settings default, sizes on every option", () => {
    renderExact();
    expect(shown(select("Resolution"))).toBe("2160p · ~360 MB");
    expect(optionTexts(select("Codec"))).toEqual(["AV1 · ~360 MB", "VP9 · ~300 MB"]);
    expect(shown(select("Audio track"))).toBe("Opus 257k · webm · ~6.9 MB");
    expect(screen.getByText(/Estimated size ~367 MB/)).toBeTruthy();
  });

  it("offers only the heights the video has", () => {
    renderExact();
    expect(optionTexts(select("Resolution")).map((t) => t.split(" ")[0])).toEqual([
      "2160p", "1440p", "1080p", "720p", "144p",
    ]);
  });

  it("hides frame rate, dynamic range and language when there is one of each", () => {
    renderExact();
    expect(screen.queryByLabelText("Frame rate")).toBeNull();
    expect(screen.queryByLabelText("Dynamic range")).toBeNull();
    expect(screen.queryByLabelText("Language")).toBeNull();
  });

  it("narrows the codecs to the chosen height and resolves to exact ids", () => {
    const { quality } = renderExact();
    choose(select("Resolution"), "1080p · ~30 MB");
    expect(optionTexts(select("Codec"))).toEqual(["AV1 · ~30 MB", "VP9 · ~28 MB", "H.264 · ~80 MB"]);
    choose(select("Codec"), "H.264 · ~80 MB");
    choose(select("Audio track"), "AAC 130k · m4a · ~3.4 MB");
    expect(quality()).toEqual({ ...DEFAULT_QUALITY, format: "137+140" });
  });

  it("names the audio id alone in audio mode", () => {
    const { quality } = renderExact();
    choose(select("Mode"), "Audio only");
    expect(screen.queryByLabelText("Resolution")).toBeNull();
    choose(select("Audio format"), "Opus");
    expect(quality()).toEqual({ ...DEFAULT_QUALITY, mode: "audio", audio_format: "opus", format: "251" });
  });

  it("shows a language row when the audio comes in more than one", () => {
    const f = { ...rickroll(), audio: [a("251", "opus", 129, 3 * MB), a("251-es", "opus", 128, 3 * MB, { language: "es" })] };
    const { quality } = renderExact(f);
    choose(select("Language"), "es");
    expect(quality()?.format).toBe("401+251-es");
  });

  it("lets the raw field win and disables the track selects", () => {
    const { quality } = renderExact();
    fireEvent.change(screen.getByLabelText("Format (-f)"), { target: { value: "18" } });
    expect(select("Resolution").disabled).toBe(true);
    expect(select("Audio track").disabled).toBe(true);
    expect(select("Container").disabled).toBe(false);
    expect(quality()?.format).toBe("18");
  });
});
