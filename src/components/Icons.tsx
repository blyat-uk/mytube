/**
 * The app's icon vocabulary, in one place.
 *
 * Icons come from Phosphor, imported one file at a time rather than from the
 * package barrel — the barrel is ~9000 modules and makes the dev server crawl.
 * Components name the *action*, not the glyph, so swapping an icon later is a
 * one-line change here instead of a hunt through the components.
 */
import { ArrowCircleDown } from "@phosphor-icons/react/dist/icons/ArrowCircleDown";
import { ArrowClockwise } from "@phosphor-icons/react/dist/icons/ArrowClockwise";
import { ArrowSquareOut } from "@phosphor-icons/react/dist/icons/ArrowSquareOut";
import { Check } from "@phosphor-icons/react/dist/icons/Check";
import { CardsThree } from "@phosphor-icons/react/dist/icons/CardsThree";
import { CheckCircle } from "@phosphor-icons/react/dist/icons/CheckCircle";
import { ClockCounterClockwise } from "@phosphor-icons/react/dist/icons/ClockCounterClockwise";
import { Circle } from "@phosphor-icons/react/dist/icons/Circle";
import { DownloadSimple } from "@phosphor-icons/react/dist/icons/DownloadSimple";
import { EyeSlash } from "@phosphor-icons/react/dist/icons/EyeSlash";
import { Funnel } from "@phosphor-icons/react/dist/icons/Funnel";
import { CaretDown } from "@phosphor-icons/react/dist/icons/CaretDown";
import { MagnifyingGlass } from "@phosphor-icons/react/dist/icons/MagnifyingGlass";
import { GearSix } from "@phosphor-icons/react/dist/icons/GearSix";
import { Play } from "@phosphor-icons/react/dist/icons/Play";
import { SquaresFour } from "@phosphor-icons/react/dist/icons/SquaresFour";
import { Star } from "@phosphor-icons/react/dist/icons/Star";
import { Trash } from "@phosphor-icons/react/dist/icons/Trash";
import { Stack } from "@phosphor-icons/react/dist/icons/Stack";
import { X } from "@phosphor-icons/react/dist/icons/X";
import { User } from "@phosphor-icons/react/dist/icons/User";
import { Clock } from "@phosphor-icons/react/dist/icons/Clock";
import { ArrowsClockwise } from "@phosphor-icons/react/dist/icons/ArrowsClockwise";
import { Archive } from "@phosphor-icons/react/dist/icons/Archive";
import { Wrench } from "@phosphor-icons/react/dist/icons/Wrench";
import { Info } from "@phosphor-icons/react/dist/icons/Info";
import { Folder } from "@phosphor-icons/react/dist/icons/Folder";
import { Minus } from "@phosphor-icons/react/dist/icons/Minus";
import { Plus } from "@phosphor-icons/react/dist/icons/Plus";
import { FilmStrip } from "@phosphor-icons/react/dist/icons/FilmStrip";
import { MusicNotes } from "@phosphor-icons/react/dist/icons/MusicNotes";
import { CaretRight } from "@phosphor-icons/react/dist/icons/CaretRight";
import { Monitor } from "@phosphor-icons/react/dist/icons/Monitor";
import { Laptop } from "@phosphor-icons/react/dist/icons/Laptop";
import { PencilSimple } from "@phosphor-icons/react/dist/icons/PencilSimple";
import { Globe } from "@phosphor-icons/react/dist/icons/Globe";
import { FileText } from "@phosphor-icons/react/dist/icons/FileText";
import { Prohibit } from "@phosphor-icons/react/dist/icons/Prohibit";
import { Sparkle } from "@phosphor-icons/react/dist/icons/Sparkle";
import { ShieldCheck } from "@phosphor-icons/react/dist/icons/ShieldCheck";
import { Eye } from "@phosphor-icons/react/dist/icons/Eye";
import { Copy } from "@phosphor-icons/react/dist/icons/Copy";
import { ArrowUp } from "@phosphor-icons/react/dist/icons/ArrowUp";
import { Warning } from "@phosphor-icons/react/dist/icons/Warning";
import { TerminalWindow } from "@phosphor-icons/react/dist/icons/TerminalWindow";
import { UploadSimple } from "@phosphor-icons/react/dist/icons/UploadSimple";

/* Size comes from CSS (`.icon`), which scales with the grid zoom, so the
   components only pin the weight. */

export const IconDownload = () => <DownloadSimple className="icon" weight="bold" />;
export const IconPlay = () => <Play className="icon" weight="fill" />;
export const IconCancel = () => <X className="icon" weight="bold" />;
export const IconRetry = () => <ArrowClockwise className="icon" weight="bold" />;
export const IconExternal = () => <ArrowSquareOut className="icon" weight="bold" />;
export const IconSeries = () => <Stack className="icon" weight="fill" />;
/** "Only groups": a fanned hand of cards, distinct from Grouped's stack. */
export const IconGroupsOnly = () => <CardsThree className="icon" weight="bold" />;
/** "Continue watching": the clock turned back, to pick a series up where you left it. */
export const IconContinue = () => <ClockCounterClockwise className="icon" weight="bold" />;
export const IconDelete = () => <Trash className="icon" weight="bold" />;
/** Ticked off: an outline while it is still to watch, filled once it is done —
 *  the counterpart of IconUnwatched's dot. */
export const IconWatched = ({ done = false }: { done?: boolean }) => (
  <CheckCircle className="icon" weight={done ? "fill" : "bold"} />
);

/** Work in flight: IconRetry's arrow again, named for waiting rather than for
 *  retrying — `.icon-btn.is-spinning` is what turns it. */
export const IconBusy = () => <ArrowClockwise className="icon" weight="bold" />;

/** A channel membership: an outline to offer one, filled once you have joined. */
export const IconMember = ({ joined = false }: { joined?: boolean }) => (
  <Star className="icon" weight={joined ? "fill" : "bold"} />
);

/** Auto-download for a channel: an outline while off, filled once its new
 *  uploads download on their own. Distinct from IconDownload's bare arrow,
 *  which fetches one video now. */
export const IconAutoDownload = ({ on = false }: { on?: boolean }) => (
  <ArrowCircleDown className="icon" weight={on ? "fill" : "bold"} />
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
export const IconCheck = () => <Check className="icon" weight="bold" />;
export const IconClose = () => <X className="icon" weight="bold" />;
/** The folded filter row's button, and what it opens. */
export const IconFilters = () => <Funnel className="icon" weight="bold" />;
export const IconSearch = () => <MagnifyingGlass className="icon" weight="bold" />;
export const IconCaret = () => <CaretDown className="icon icon-caret" weight="bold" />;

/* Settings. Each section's glyph names the section, and its rail entry, its page
   header and its folded tile all wear the same one. */
export const IconSectionDownloads = () => <DownloadSimple className="icon" weight="bold" />;
export const IconSectionSignIn = () => <User className="icon" weight="bold" />;
export const IconSectionPlayer = () => <Play className="icon" weight="fill" />;
export const IconSectionNewVideos = () => <Clock className="icon" weight="bold" />;
export const IconSectionSync = () => <ArrowsClockwise className="icon" weight="bold" />;
export const IconSectionBackup = () => <Archive className="icon" weight="bold" />;
export const IconSectionTools = () => <Wrench className="icon" weight="bold" />;
export const IconSectionAbout = () => <Info className="icon" weight="bold" />;

export const IconFolder = () => <Folder className="icon" weight="bold" />;
export const IconMinus = () => <Minus className="icon" weight="bold" />;
export const IconPlus = () => <Plus className="icon" weight="bold" />;
export const IconVideoMode = () => <FilmStrip className="icon" weight="bold" />;
export const IconAudioMode = () => <MusicNotes className="icon" weight="bold" />;
export const IconChevron = () => <CaretRight className="icon" weight="bold" />;
/** A computer in Sync's list: this one and desktops alike — a peer's kind is unknown. */
export const IconComputer = () => <Monitor className="icon" weight="bold" />;
export const IconLaptop = () => <Laptop className="icon" weight="bold" />;
export const IconEdit = () => <PencilSimple className="icon" weight="bold" />;
export const IconBrowser = () => <Globe className="icon" weight="bold" />;
export const IconFile = () => <FileText className="icon" weight="bold" />;
export const IconNone = () => <Prohibit className="icon" weight="bold" />;
/** Automatic: picked for you. */
export const IconAutomatic = () => <Sparkle className="icon" weight="fill" />;
export const IconShield = () => <ShieldCheck className="icon" weight="bold" />;
export const IconShow = () => <Eye className="icon" weight="bold" />;
export const IconCopy = () => <Copy className="icon" weight="bold" />;
export const IconNewRelease = () => <ArrowUp className="icon" weight="bold" />;
export const IconWarning = () => <Warning className="icon" weight="bold" />;
export const IconInfo = () => <Info className="icon" weight="bold" />;
export const IconTerminal = () => <TerminalWindow className="icon" weight="bold" />;
export const IconExport = () => <UploadSimple className="icon" weight="bold" />;
export const IconImport = () => <DownloadSimple className="icon" weight="bold" />;
