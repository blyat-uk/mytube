import { memo, useEffect, useLayoutEffect, useMemo, useRef, useState, type CSSProperties } from "react";
import { thumbSrc } from "../../api";
import type { Video } from "../../types";
import { bandStates, cssUrl, placeAround, seriesModel, type Band, type Figure } from "./seriesBands";

export interface SeriesBandsProps {
  /** Every loaded part of the open series, in any order: the bands put them in
   *  upload order themselves. */
  parts: Video[];
}

/** What two bracketed figures keep between them before they give up centring
 *  on their brackets and spread to the block's ends instead. */
const FIGURE_GAP = 24;
/** Between a hovered band and its tooltip. */
const TIP_GAP = 10;
/** The tooltip flips under the bands rather than come this close to the
 *  window's top edge. */
const TIP_EDGE = 8;

/**
 * Every source of a part's thumbnail as background layers, best first. A layer
 * that fails to load draws nothing and the one under it shows through, which
 * is the same cached-then-remote fallback a card's <img> gets from onError.
 */
function thumbLayers(v: Video): string | undefined {
  const list: string[] = [];
  const primary = thumbSrc(v);
  if (primary) list.push(primary);
  if (v.thumb_url && v.thumb_url !== primary) list.push(v.thumb_url);
  return list.length ? list.map(cssUrl).join(", ") : undefined;
}

function FigureText({ f, mark }: { f: Figure; mark?: "watched" | "left" }) {
  return (
    <div className="sb-fig">
      <b>{f.runtime}</b>
      {mark && <i className={`sb-mk is-${mark}`} />}
      <span>
        {f.caption}
        {f.downloaded > 0 && (
          <>
            , <em>{f.downloaded} downloaded</em>
          </>
        )}
      </span>
    </div>
  );
}

function bandClass(b: Band, count: number, tip: boolean): string {
  let c = "sb-band";
  if (b.position === 1) c += " is-first";
  if (b.position === count) c += " is-last";
  if (b.video.watched) c += " is-watched";
  if (b.downloaded) c += " is-disk";
  if (b.video.hidden) c += " is-hidden";
  if (b.next) c += " is-next";
  if (tip) c += " is-tip";
  return c;
}

/**
 * Inside a series: one band per part in upload order, each as wide as the part
 * is long and filled with that part's own thumbnail blurred down to its
 * colours. A red rail runs under what you watched and a green one under what
 * is on disk, the way YouTube underlines a thumbnail; hidden parts are hatched.
 * The next part wears the amber of a series in progress, and two figures below
 * say how much is behind you and how much is left.
 *
 * The bands and the brackets under them share one grid whose columns are the
 * parts' runtimes in `fr`, so a bracket over parts 1 to 7 ends exactly where
 * band 7 does, at any width. Only three things need measuring, and a layout
 * effect does them before the first paint: the next label's place (centred on
 * its band, held inside the block near an end), whether the two figures fit
 * side by side, and the hover tooltip's place.
 *
 * Memoised: the view hands over the same `parts` array until a part changes,
 * and everything drawn is derived from it once.
 */
function SeriesBands({ parts }: SeriesBandsProps) {
  const model = useMemo(() => (parts.length < 2 ? null : seriesModel(parts)), [parts]);
  const layers = useMemo(() => model?.bands.map((b) => thumbLayers(b.video)) ?? [], [model]);
  /** The hovered part, by id: an index would point at another part once a
   *  date moved the order under the pointer. */
  const [tip, setTip] = useState<string | null>(null);
  const rootRef = useRef<HTMLElement>(null);
  const gridRef = useRef<HTMLDivElement>(null);
  const labelRef = useRef<HTMLDivElement>(null);
  const tipRef = useRef<HTMLDivElement>(null);

  // A part that went away under the pointer takes its tooltip with it.
  const tipBand = (tip !== null && model?.bands.find((b) => b.video.id === tip)) || null;

  useLayoutEffect(() => {
    const root = rootRef.current;
    const grid = gridRef.current;
    if (!model || !root || !grid) return;
    const layout = () => {
      const label = labelRef.current;
      const band = grid.querySelector<HTMLElement>(".sb-band.is-next");
      if (label && band) {
        const x = placeAround(band.offsetLeft + band.offsetWidth / 2, label.offsetWidth, 0, grid.clientWidth);
        label.style.left = `${x}px`;
      }
      // Each bracketed figure centres on its bracket and, when its bracket is
      // too short for it, leans out over the other one (CSS does that). Two
      // short-on-room figures would then collide, and only a measurement can
      // tell: so measure them centred, and if they come within FIGURE_GAP of
      // each other, push them to the block's two ends and let them wrap. The
      // attribute is React's to leave alone, since it never renders one.
      delete root.dataset.spread;
      if (model.figures.kind !== "brackets") return;
      const [a, b] = Array.from(root.querySelectorAll<HTMLElement>(".sb-brk .sb-fig"));
      if (a && b && a.getBoundingClientRect().right + FIGURE_GAP > b.getBoundingClientRect().left) {
        root.dataset.spread = "";
      }
    };
    layout();
    if (typeof ResizeObserver === "undefined") return;
    // The grid for the window's width and the figures' height (the text size),
    // the label for its own width, which the text size moves without moving
    // the grid's.
    const ro = new ResizeObserver(layout);
    ro.observe(grid);
    if (labelRef.current) ro.observe(labelRef.current);
    return () => ro.disconnect();
  }, [model]);

  useLayoutEffect(() => {
    const grid = gridRef.current;
    const el = tipRef.current;
    if (!tipBand || !grid || !el) return;
    const band = grid.querySelector<HTMLElement>(`.sb-band[data-part="${tipBand.position}"]`);
    if (!band) return;
    // Fixed, in window coordinates: the bands sit at the top of the scroll
    // area, which would clip a tooltip above them. Held inside the block's
    // width, above the band unless that leaves the window, then below it.
    const g = grid.getBoundingClientRect();
    const b = band.getBoundingClientRect();
    const centre = b.left + b.width / 2;
    const x = placeAround(centre, el.offsetWidth, g.left, g.right);
    let y = b.top - TIP_GAP - el.offsetHeight;
    const above = y >= TIP_EDGE;
    // Below clears the rails, which hang 12px under the band.
    if (!above) y = b.bottom + TIP_GAP + 12;
    el.style.left = `${x}px`;
    el.style.top = `${y}px`;
    el.style.setProperty("--caret", `${Math.max(12, Math.min(el.offsetWidth - 12, centre - x))}px`);
    el.dataset.side = above ? "above" : "below";
  }, [tipBand]);

  // A fixed tooltip would stay put while its band scrolled away under it.
  const tipOpen = tipBand !== null;
  useEffect(() => {
    if (!tipOpen) return;
    const hide = () => setTip(null);
    window.addEventListener("scroll", hide, true);
    window.addEventListener("resize", hide);
    return () => {
      window.removeEventListener("scroll", hide, true);
      window.removeEventListener("resize", hide);
    };
  }, [tipOpen]);

  if (!model) return null;
  const { bands, next, figures } = model;
  const count = bands.length;
  const gridStyle = { gridTemplateColumns: model.columns, "--sb-gap": `${model.gap}px` } as CSSProperties;

  // Leaving a band for its neighbour keeps the tooltip up (it just moves);
  // only leaving the row of bands takes it down.
  const leave = (e: React.MouseEvent) => {
    const to = e.relatedTarget;
    if (!(to instanceof Element) || !to.closest(".sb-band")) setTip(null);
  };

  return (
    <section ref={rootRef} className="series-bands">
      <div
        ref={gridRef}
        className={`sb-grid${model.gap === 1 ? " is-dense" : ""}`}
        style={gridStyle}
        role="img"
        aria-label={model.label}
      >
        {bands.map((b, i) => (
          <div
            key={b.video.id}
            className={bandClass(b, count, tip === b.video.id)}
            style={{ gridColumn: i + 1 }}
            data-part={b.position}
            onMouseEnter={() => setTip(b.video.id)}
            onMouseLeave={leave}
          >
            <div className="sb-fill" style={{ "--img": layers[i] } as CSSProperties} />
            <i className="sb-hit" />
          </div>
        ))}

        {next && (
          <div ref={labelRef} className="sb-next">
            Part {next.position} is next
            {next.runtime && <span>{next.runtime}</span>}
          </div>
        )}

        {figures.kind === "brackets" && (
          <>
            <div className="sb-brk is-watched" style={{ gridColumn: `1 / ${figures.split + 1}` }}>
              <FigureText f={figures.watched} />
            </div>
            <div className="sb-brk is-left" style={{ gridColumn: `${figures.split + 1} / -1` }}>
              <FigureText f={figures.left} />
            </div>
          </>
        )}
        {figures.kind === "plain" && (
          <div className="sb-plain">
            {figures.watched && <FigureText f={figures.watched} mark="watched" />}
            {figures.left && <FigureText f={figures.left} mark="left" />}
          </div>
        )}
      </div>

      {tipBand && (
        <div ref={tipRef} className="sb-tip" aria-hidden="true">
          {/* One line of facts over the title, so the box stays short enough
              to fit above the bands under the top row at 100% text. */}
          <div className="sb-tip-head">
            <span className="sb-tip-part">
              Part {tipBand.position} of {count}
              {tipBand.runtime && <span>{tipBand.runtime}</span>}
            </span>
            <span className="sb-tip-states">
              {bandStates(tipBand).map((s) => (
                <span key={s} className={`sb-state is-${s.replace(" ", "-")}`}>
                  <i />
                  {s}
                </span>
              ))}
            </span>
          </div>
          <div className="sb-tip-title">{tipBand.video.title}</div>
        </div>
      )}
    </section>
  );
}

export default memo(SeriesBands);
