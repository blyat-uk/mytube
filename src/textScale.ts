/*
 * Settings → Appearance's text size. A percentage of the stylesheet's own
 * sizes, handed to CSS as the unitless `--text-scale` on the root element:
 * App.css sets `html { font-size: calc(10px * var(--text-scale, 1)) }` and
 * sizes every piece of text in rem, so this one property moves all the text
 * and nothing else. Icons, thumbnails and spacing are in px and stay put --
 * the user asked for bigger words, not a zoomed page.
 */

/** The slider's range, the one Rust clamps `text_scale` to. */
export const TEXT_SCALE = { min: 80, max: 200, step: 10, default: 100 } as const;

const PROPERTY = "--text-scale";

/**
 * Any number onto one of the slider's stops, rounding a half step up exactly
 * as `Settings::from_json_str` does. Not a number at all is 100%: an older
 * backend, or a fixture, may simply not carry the key.
 */
export function normalizeTextScale(pct: number): number {
  if (!Number.isFinite(pct)) return TEXT_SCALE.default;
  const stepped = Math.round(pct / TEXT_SCALE.step) * TEXT_SCALE.step;
  return Math.min(TEXT_SCALE.max, Math.max(TEXT_SCALE.min, stepped));
}

/** Sets every text size in the app to `pct` percent of its own, at once. */
export function applyTextScale(pct: number): void {
  document.documentElement.style.setProperty(PROPERTY, String(normalizeTextScale(pct) / 100));
}

/** How long the first render waits for the stored size; see `bootTextScale`. */
export const BOOT_WAIT_MS = 1000;

/**
 * Applies the stored text size before React first renders, so a window kept at
 * 150% never paints a frame at 100% on the way there. Resolves once the size is
 * applied, once the read has failed, or after `waitMs` -- never rejects.
 *
 * A failed read is the ordinary case under `bun run dev` in a browser, where
 * there is no IPC bridge and every invoke rejects at once; CSS's fallback is
 * 100%. The timeout is not for that. `get_settings` is one small file read,
 * answered in milliseconds, but this module must never be the reason nothing
 * renders: if the backend is wedged, App.tsx's own settings read (which the
 * shell already waits on) decides what the window shows, not a promise in here.
 * A read that lands after the wait still applies, unless something -- the
 * Appearance page's slider -- has set a size since: that one is newer.
 */
export function bootTextScale(
  read: () => Promise<{ text_scale: number }>,
  waitMs = BOOT_WAIT_MS,
): Promise<void> {
  const landed = Promise.resolve()
    .then(read)
    .then(
      (s) => {
        if (!document.documentElement.style.getPropertyValue(PROPERTY)) applyTextScale(s.text_scale);
      },
      () => {},
    );
  let timer: number | undefined;
  const waited = new Promise<void>((resolve) => { timer = window.setTimeout(resolve, waitMs); });
  return Promise.race([landed, waited]).finally(() => window.clearTimeout(timer));
}
