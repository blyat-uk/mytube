/**
 * The app's icon vocabulary, in one place.
 *
 * Icons come from Phosphor, imported one file at a time rather than from the
 * package barrel — the barrel is ~9000 modules and makes the dev server crawl.
 * Components name the *action*, not the glyph, so swapping an icon later is a
 * one-line change here instead of a hunt through the components.
 */
import { ArrowClockwise } from "@phosphor-icons/react/dist/icons/ArrowClockwise";
import { ArrowSquareOut } from "@phosphor-icons/react/dist/icons/ArrowSquareOut";
import { Circle } from "@phosphor-icons/react/dist/icons/Circle";
import { DownloadSimple } from "@phosphor-icons/react/dist/icons/DownloadSimple";
import { EyeSlash } from "@phosphor-icons/react/dist/icons/EyeSlash";
import { GearSix } from "@phosphor-icons/react/dist/icons/GearSix";
import { Play } from "@phosphor-icons/react/dist/icons/Play";
import { SquaresFour } from "@phosphor-icons/react/dist/icons/SquaresFour";
import { Star } from "@phosphor-icons/react/dist/icons/Star";
import { Trash } from "@phosphor-icons/react/dist/icons/Trash";
import { Stack } from "@phosphor-icons/react/dist/icons/Stack";
import { X } from "@phosphor-icons/react/dist/icons/X";

/* Size comes from CSS (`.icon`), which scales with the grid zoom, so the
   components only pin the weight. */

export const IconDownload = () => <DownloadSimple className="icon" weight="bold" />;
export const IconPlay = () => <Play className="icon" weight="fill" />;
export const IconCancel = () => <X className="icon" weight="bold" />;
export const IconRetry = () => <ArrowClockwise className="icon" weight="bold" />;
export const IconExternal = () => <ArrowSquareOut className="icon" weight="bold" />;
export const IconSeries = () => <Stack className="icon" weight="fill" />;
export const IconDelete = () => <Trash className="icon" weight="bold" />;
/** Work in flight: IconRetry's arrow again, named for waiting rather than for
 *  retrying — `.icon-btn.is-spinning` is what turns it. */
export const IconBusy = () => <ArrowClockwise className="icon" weight="bold" />;

/** A channel membership: an outline to offer one, filled once you have joined. */
export const IconMember = ({ joined = false }: { joined?: boolean }) => (
  <Star className="icon" weight={joined ? "fill" : "bold"} />
);

/* The nav wears these only once the window is too narrow for their words, so
   each one lives beside a label it has to stand in for on its own. */
export const IconSubscriptions = () => <SquaresFour className="icon" weight="bold" />;
export const IconSettings = () => <GearSix className="icon" weight="bold" />;
/** The unread dot: what is left once you strike out everything watched. */
export const IconUnwatched = () => <Circle className="icon" weight="fill" />;
/** A struck-through eye stands for hidden — the toggle brings those back. */
export const IconHidden = () => <EyeSlash className="icon" weight="bold" />;
/** Same glyph as IconCancel, named for dismissing rather than stopping. */
export const IconClose = () => <X className="icon" weight="bold" />;
