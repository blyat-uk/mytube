import {
  memo, useEffect, useId, useLayoutEffect, useMemo, useRef, useState,
  type CSSProperties, type PointerEvent, type Ref,
} from "react";
import type { FeedStats } from "../../types";
import {
  WAVE_HEIGHT, axisTicks, barLabel, barX, breakLabel, chooseLayout, halfHeight, itemCentre,
  itemIndexAt, runtimeText, segments, seriesCaption, tipRows, todayOf, videosText, waveCap,
  waveGeometry, waveSummary, type Unit, type WaveBar, type WaveGeometry, type WaveItem,
} from "./waveform";

export interface FeedWaveformProps {
  stats: FeedStats;
}

const MID = WAVE_HEIGHT / 2;
/** Each half stands this far off the axis, so the two read as a pair. */
const AXIS_GAP = 1;
/** How long the entrance sweep takes to cross the wave, and each bar's own grow. */
const SWEEP_MS = 360;
const GROW_MS = 460;
/** The 10rem floor of the wave's column at 100% text: what a first render
 *  draws for, before the column has been measured (and all jsdom ever has). */
const JSDOM_MIN = 100;

/**
 * Bars grow from the axis once, the first time the wave appears in a session --
 * not on the refetches a channel pick, a search or a poll bring, nor when the
 * caller mounts it afresh. Module state, because that is the one thing a
 * remount does not reset.
 */
let enteredThisSession = false;

const fmt = (n: number) => n.toLocaleString("en");

/**
 * The feed's stats: the library as a sound wave, one bar per day, week, month
 * or longer of uploads -- whichever fits what the library holds (see
 * `waveform.ts`) -- mirrored about its axis and filled from the middle out:
 * red watched, green on disk, grey still to watch, hatched tips hidden. Long
 * quiet stretches are cut out and drawn as a break. The totals stand either
 * side like a player's timestamps, and four figures under it name the colours.
 *
 * Presentation only: the counts arrive as `stats`, already narrowed to the
 * channel pick and the search. Memoised, so the feed's own re-renders -- a
 * card's state, a selection -- pass it by while `stats` is unchanged.
 *
 * Its height depends on the width it is given and never on the numbers (see
 * `.wv` in App.css): the caller swaps fresh stats in over old ones, and the
 * scroll position restored beneath must not move when it does.
 */
function FeedWaveform({ stats }: FeedWaveformProps) {
  if (stats.videos === 0) return null;
  return <Wave stats={stats} />;
}

export default memo(FeedWaveform);

function Wave({ stats }: FeedWaveformProps) {
  const uid = useId().replace(/[^a-zA-Z0-9_-]/g, "");
  const rootRef = useRef<HTMLDivElement>(null);
  const midRef = useRef<HTMLDivElement>(null);
  const waveRef = useRef<HTMLDivElement>(null);
  const tipRef = useRef<HTMLDivElement>(null);

  // The column the wave may take: a short history gets a short wave rather
  // than a few bars strewn across 720px. Never narrower than the pill above it
  // (10rem holds "+99 this week"); the few bars then gather in the middle.
  const today = todayOf(stats);
  const cap = useMemo(() => waveCap(stats.days, today), [stats.days, today]);

  // The wave's real width, and the root font size the year labels are spaced
  // by. Read before the first paint, then kept by a ResizeObserver on the
  // column, whose axis row is in rem: a text-size change resizes it too. jsdom
  // has neither layout nor ResizeObserver, so tests draw at the cap's width.
  const [box, setBox] = useState(() => ({ width: Math.max(cap, JSDOM_MIN), rem: 10 }));
  useLayoutEffect(() => {
    const el = midRef.current;
    if (!el) return;
    const read = () => {
      const width = Math.floor(el.getBoundingClientRect().width) || Math.max(cap, JSDOM_MIN);
      const rem = parseFloat(getComputedStyle(document.documentElement).fontSize) || 10;
      setBox((b) => (b.width === width && b.rem === rem ? b : { width, rem }));
    };
    read();
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(read);
    ro.observe(el);
    return () => ro.disconnect();
  }, [cap]);

  const layout = useMemo(() => chooseLayout(stats.days, today, box.width), [stats.days, today, box.width]);
  const items = layout.items;
  const g = useMemo(() => waveGeometry(layout.slots, box.width), [layout.slots, box.width]);
  const busiest = useMemo(
    () => items.reduce((m, it) => (it.kind === "bar" ? Math.max(m, it.bar.total) : m), 0),
    [items],
  );
  const ticks = useMemo(() => axisTicks(layout, g, box.rem), [layout, g, box.rem]);

  const [enter] = useState(() => !enteredThisSession);
  const [entering, setEntering] = useState(enter);
  useEffect(() => {
    if (!enter) return;
    enteredThisSession = true;
    const t = window.setTimeout(() => setEntering(false), SWEEP_MS + GROW_MS + 100);
    return () => window.clearTimeout(t);
  }, [enter]);

  // The hovered bar or break. Picked by distance, not by hitting a 5px bar,
  // so the whole height of the wave answers.
  const [hover, setHover] = useState<number | null>(null);
  const hovered = hover !== null && hover < items.length ? items[hover] : null;
  const onMove = (e: PointerEvent<HTMLDivElement>) => {
    const x = e.clientX - e.currentTarget.getBoundingClientRect().left;
    const i = itemIndexAt(x, items, g);
    setHover(i < 0 ? null : i);
  };

  // The tooltip stands beside its bar, its top level with the wave's, on
  // whichever side has room, and never leaves the component. Beside rather
  // than above: the busiest bars reach the top of the wave, and above them is
  // the edge of the scroll container, which would cut it off. Level with the
  // wave's top, so it never covers the "+N this week" pill over the newest bars.
  useLayoutEffect(() => {
    const tip = tipRef.current, root = rootRef.current, waveEl = waveRef.current;
    if (!tip || !root || !waveEl || hovered === null) return;
    const r = root.getBoundingClientRect();
    const wave = waveEl.getBoundingClientRect();
    const cx = wave.left - r.left + itemCentre(hovered, g);
    const w = tip.offsetWidth, h = tip.offsetHeight;
    let left = cx + 14;
    if (left + w > r.width) left = cx - 14 - w;
    left = Math.max(0, Math.min(left, r.width - w));
    const top = Math.max(0, Math.min(wave.top - r.top, r.height - h));
    tip.style.transform = `translate(${Math.round(left)}px, ${Math.round(top)}px)`;
  }, [hovered, g]);

  const style = { "--wv-w": `max(${cap}px, 10rem)` } as CSSProperties;
  const hatch = `${uid}-hatch`;

  return (
    <section className="wv-box" aria-label="Feed stats">
      <div
        ref={rootRef}
        className={`wv${hovered !== null ? " is-hovering" : ""}`}
        style={style}
      >
        <div className="wv-flank is-l">
          <b>{fmt(stats.videos)}</b>
          <span>{seriesCaption(stats.videos, stats.series)}</span>
        </div>

        <div className="wv-mid" ref={midRef}>
          <div className="wv-top">
            {stats.newThisWeek > 0 && <span className="wv-new">+{fmt(stats.newThisWeek)} this week</span>}
          </div>
          <div className="wv-wave" ref={waveRef} onPointerMove={onMove} onPointerLeave={() => setHover(null)}>
            <svg
              className="wv-svg"
              width={box.width}
              height={WAVE_HEIGHT}
              viewBox={`0 0 ${box.width} ${WAVE_HEIGHT}`}
              role="img"
              aria-label={waveSummary(stats)}
            >
              <defs>
                {/* --hatch, drawn: #5a5a5a stripes 2px wide every 5px on #2c2c2c. */}
                <pattern id={hatch} width="5" height="5" patternUnits="userSpaceOnUse" patternTransform="rotate(45)">
                  <rect width="5" height="5" fill="#2c2c2c" />
                  <rect width="2" height="5" fill="#5a5a5a" />
                </pattern>
              </defs>
              <WaveBars items={items} slots={layout.slots} g={g} busiest={busiest} uid={uid} hatch={hatch} entering={entering} />
              {hovered !== null && (
                // The hovered bar again, over its dimmed self: it comes out
                // brighter than any bar at rest, which is the point.
                <g className="wv-hot">
                  {hovered.kind === "bar"
                    ? <Bar bar={hovered.bar} at={hovered.at} g={g} busiest={busiest} id={`${uid}-hot`} hatch={hatch} />
                    : <Break item={hovered} g={g} />}
                </g>
              )}
            </svg>
          </div>
          <div className="wv-axis" aria-hidden="true">
            {ticks.map((t) => (
              <span key={`${t.label}@${t.x}`} className={`wv-year is-${t.align}`} style={{ left: `${t.x}px` }}>
                {t.label}
              </span>
            ))}
          </div>
        </div>

        <div className="wv-flank is-r">
          <b>{runtimeText(stats.runtimeSecs, stats.runtimeUnknown)}</b>
          <span>of runtime</span>
        </div>

        <div className="wv-figs">
          <Figure kind="left" n={stats.toWatch} label="to watch" />
          <Figure kind="watched" n={stats.watched} label="watched" />
          <Figure kind="disk" n={stats.downloaded} label="downloaded" />
          <Figure kind="hidden" n={stats.hidden} label="hidden" />
        </div>

        {hovered !== null && <Tip ref={tipRef} item={hovered} unit={layout.unit} />}
      </div>
    </section>
  );
}

interface BarsProps {
  items: WaveItem[];
  slots: number;
  g: WaveGeometry;
  busiest: number;
  uid: string;
  hatch: string;
  entering: boolean;
}

/**
 * Every bar, memoised apart from the hover: a few hundred clipped shapes are
 * not redrawn each time the pointer crosses from one bar to the next.
 */
const WaveBars = memo(function WaveBars({ items, slots, g, busiest, uid, hatch, entering }: BarsProps) {
  const last = Math.max(1, slots - 1);
  return (
    <g className={`wv-bars${entering ? " is-entering" : ""}`}>
      {items.map((item, i) => item.kind === "bar" ? (
        <Bar
          key={i}
          bar={item.bar}
          at={item.at}
          g={g}
          busiest={busiest}
          id={`${uid}-b${i}`}
          hatch={hatch}
          delay={entering ? Math.round((item.at / last) * SWEEP_MS) : undefined}
        />
      ) : <Break key={i} item={item} g={g} />)}
    </g>
  );
});

/**
 * A quiet stretch cut out of the wave: two slashes across the axis, the
 * broken-axis mark a chart uses for the same thing, so the stretches either
 * side do not read as neighbours.
 */
function Break({ item, g }: { item: WaveItem; g: WaveGeometry }) {
  const cx = itemCentre(item, g);
  const d = [-2.5, 2.5]
    .map((dx) => `M${cx + dx - 2.5} ${MID + 7}L${cx + dx + 2.5} ${MID - 7}`)
    .join("");
  return <path className="wv-break" d={d} />;
}

interface BarProps {
  bar: WaveBar;
  /** The slot it stands in. */
  at: number;
  g: WaveGeometry;
  busiest: number;
  /** Unique per drawn bar: its two clip paths are named from it. */
  id: string;
  hatch: string;
  delay?: number;
}

/**
 * One bar: two halves either side of the axis, each a pill the colours are
 * clipped to, so a segment shorter than the pill's round end still takes the
 * curve rather than poking out square.
 */
function Bar({ bar, at, g, busiest, id, hatch, delay }: BarProps) {
  const half = halfHeight(bar.total, busiest);
  if (half <= 0) return null;
  const x = barX(at, g), w = g.barWidth, r = w / 2;
  const segs = segments(bar, half);
  const top = MID - AXIS_GAP - half, bottom = MID + AXIS_GAP;
  const fill = (kind: string) => (kind === "hidden" ? `url(#${hatch})` : undefined);
  return (
    <g className="wv-bar" style={delay !== undefined ? ({ animationDelay: `${delay}ms` } as CSSProperties) : undefined}>
      <clipPath id={`${id}-t`}><rect x={x} y={top} width={w} height={half} rx={r} /></clipPath>
      <clipPath id={`${id}-b`}><rect x={x} y={bottom} width={w} height={half} rx={r} /></clipPath>
      <g clipPath={`url(#${id}-t)`}>
        {segs.map((s) => (
          <rect key={s.kind} className={`wv-s is-${s.kind}`} x={x} y={MID - AXIS_GAP - s.start - s.length}
            width={w} height={s.length} fill={fill(s.kind)} />
        ))}
      </g>
      <g clipPath={`url(#${id}-b)`}>
        {segs.map((s) => (
          <rect key={s.kind} className={`wv-s is-${s.kind}`} x={x} y={MID + AXIS_GAP + s.start}
            width={w} height={s.length} fill={fill(s.kind)} />
        ))}
      </g>
    </g>
  );
}

function Figure({ kind, n, label }: { kind: string; n: number; label: string }) {
  return (
    <div className="wv-fig">
      <i className={`wv-mk is-${kind}`} aria-hidden="true" />
      <b>{fmt(n)}</b>
      <span>{label}</span>
    </div>
  );
}

/** A bar's span and counts, or a break's span. Pointer only: the SVG's label carries the totals for everyone else. */
function Tip({ item, unit, ref }: { item: WaveItem; unit: Unit; ref: Ref<HTMLDivElement> }) {
  if (item.kind === "break") {
    return (
      <div ref={ref} className="wv-tip" role="tooltip">
        <div className="wv-tip-month">{breakLabel(item.gap)}</div>
        <div className="wv-tip-count">Nothing uploaded</div>
      </div>
    );
  }
  const bar = item.bar;
  const rows = tipRows(bar);
  return (
    <div ref={ref} className="wv-tip" role="tooltip">
      <div className="wv-tip-month">{barLabel(bar, unit)}</div>
      <div className="wv-tip-count">{videosText(bar.total)}</div>
      {rows.length > 0 && (
        <ul className="wv-tip-rows">
          {rows.map((row) => (
            <li key={row.kind}>
              <i className={`wv-mk is-${row.kind}`} aria-hidden="true" />
              <span><b>{row.count}</b> {row.label}{row.downloaded && <>, <em>{row.downloaded}</em></>}</span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
