import type { ComponentType } from "react";
import {
  IconSectionAbout, IconSectionAppearance, IconSectionBackup, IconSectionDownloads,
  IconSectionNewVideos, IconSectionPlayer, IconSectionSignIn, IconSectionSync, IconSectionTools,
} from "../Icons";
import type { RailLine, SectionId, SettingsCtx } from "./context";
import AboutPage, { summary as aboutSummary } from "./AboutPage";
import AppearancePage, { summary as appearanceSummary } from "./AppearancePage";
import BackupPage, { summary as backupSummary } from "./BackupPage";
import DownloadsPage, { summary as downloadsSummary } from "./DownloadsPage";
import NewVideosPage, { summary as newVideosSummary } from "./NewVideosPage";
import PlayerPage, { summary as playerSummary } from "./PlayerPage";
import SignInPage, { summary as signInSummary } from "./SignInPage";
import SyncPage, { summary as syncSummary } from "./SyncPage";
import ToolsPage, { summary as toolsSummary } from "./ToolsPage";

export interface Section {
  id: SectionId;
  label: string;
  tier: string;
  Icon: ComponentType;
  Page: ComponentType;
  summary: (ctx: SettingsCtx) => RailLine;
}

/**
 * The rail, in order of importance: what most visits are for first, then the
 * library, then what spans computers, then what runs underneath. This array is
 * the one place that order is written down.
 */
export const SECTIONS: Section[] = [
  { id: "downloads", label: "Downloads", tier: "Essentials", Icon: IconSectionDownloads, Page: DownloadsPage, summary: downloadsSummary },
  { id: "signin", label: "YouTube sign-in", tier: "Essentials", Icon: IconSectionSignIn, Page: SignInPage, summary: signInSummary },
  { id: "player", label: "Player", tier: "Essentials", Icon: IconSectionPlayer, Page: PlayerPage, summary: playerSummary },
  { id: "new-videos", label: "New videos", tier: "Your library", Icon: IconSectionNewVideos, Page: NewVideosPage, summary: newVideosSummary },
  { id: "sync", label: "Sync", tier: "Across computers", Icon: IconSectionSync, Page: SyncPage, summary: syncSummary },
  { id: "backup", label: "Backup & transfer", tier: "Across computers", Icon: IconSectionBackup, Page: BackupPage, summary: backupSummary },
  { id: "appearance", label: "Appearance", tier: "Under the hood", Icon: IconSectionAppearance, Page: AppearancePage, summary: appearanceSummary },
  { id: "tools", label: "Tools", tier: "Under the hood", Icon: IconSectionTools, Page: ToolsPage, summary: toolsSummary },
  { id: "about", label: "About", tier: "Under the hood", Icon: IconSectionAbout, Page: AboutPage, summary: aboutSummary },
];
