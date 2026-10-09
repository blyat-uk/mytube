import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import { IconCheck } from "../Icons";
import type { SectionId, SettingsCtx } from "./context";
import { SECTIONS } from "./sections";

/**
 * The Settings sidebar: every section in order of importance, each with a live
 * second line, so the rail doubles as an overview of the whole configuration.
 * Something that needs the user marks its own entry -- the attention rule --
 * instead of waiting at the bottom of a long page.
 *
 * At narrow widths CSS folds it to the glyphs; every entry keeps its name as
 * `aria-label` and its line in `title`, and markers move onto the glyph.
 */
export default function SettingsRail({ ctx, active, onSelect, savedAt }: {
  ctx: SettingsCtx;
  active: SectionId;
  onSelect: (id: SectionId) => void;
  /** When the last save landed; a change flashes "Saved" in the foot. */
  savedAt: number;
}) {
  const listRef = useRef<HTMLDivElement | null>(null);
  // "Saved" for a moment after each save, then the standing line again.
  const [flash, setFlash] = useState(0);
  useEffect(() => {
    if (!savedAt) return;
    setFlash(savedAt);
    const t = window.setTimeout(() => setFlash(0), 2400);
    return () => window.clearTimeout(t);
  }, [savedAt]);

  // ↑/↓ walk the entries, Home/End jump to either end: the rail is one list.
  function onKeyDown(e: KeyboardEvent<HTMLDivElement>) {
    const keys = ["ArrowDown", "ArrowUp", "Home", "End"];
    if (!keys.includes(e.key)) return;
    const items = [...(listRef.current?.querySelectorAll<HTMLButtonElement>(".st-item") ?? [])];
    const at = items.indexOf(document.activeElement as HTMLButtonElement);
    if (at < 0) return;
    e.preventDefault();
    const next = e.key === "Home" ? 0
      : e.key === "End" ? items.length - 1
        : (at + (e.key === "ArrowDown" ? 1 : -1) + items.length) % items.length;
    items[next]?.focus();
  }

  let tier = "";
  return (
    <nav className="st-rail" aria-label="Settings sections">
      <div className="st-rail-list" ref={listRef} onKeyDown={onKeyDown}>
        {SECTIONS.map((s) => {
          const line = s.summary(ctx);
          const heading = s.tier !== tier ? s.tier : null;
          tier = s.tier;
          const on = s.id === active;
          const marker = line.flag?.tone === "news" ? "news"
            : line.tone === "error" || line.flag?.tone === "error" ? "error" : null;
          return (
            <div key={s.id} className="st-rail-group">
              {heading && <div className="st-tier" aria-hidden="true">{heading}</div>}
              <button
                type="button"
                className={`st-item${on ? " is-on" : ""}${line.tone === "error" ? " is-error" : ""}`}
                aria-current={on ? "page" : undefined}
                aria-label={s.label}
                aria-describedby={`st-line-${s.id}`}
                title={line.text ? `${s.label} — ${line.text}` : s.label}
                onClick={() => onSelect(s.id)}
              >
                <span className="st-item-glyph" aria-hidden="true">
                  <s.Icon />
                  {marker && <i className={`st-marker is-${marker}`} />}
                </span>
                <span className="st-item-text">
                  <span className="st-item-name">{s.label}</span>
                  <span className="st-item-line" id={`st-line-${s.id}`}>
                    {line.tone === "ok" && <span className="st-item-ok" aria-hidden="true">● </span>}
                    {line.text}
                    {line.flag?.label && <span className="sr-only">. {line.flag.label}</span>}
                  </span>
                </span>
                {line.flag && (
                  <span className={`st-flag is-${line.flag.tone}`} aria-hidden="true">{line.flag.text}</span>
                )}
              </button>
            </div>
          );
        })}
      </div>
      <div className="st-rail-foot" aria-live="polite">
        <IconCheck />
        {flash > 0
          ? <span key={flash} className="st-saved">Saved</span>
          : <span>Changes save as you make them</span>}
      </div>
    </nav>
  );
}
