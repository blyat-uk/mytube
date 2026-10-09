import { render, screen } from "@testing-library/react";
import SettingsView from "../SettingsView";
import { ToastProvider } from "../Toast";
import type { SectionId } from "./context";

/**
 * The real Settings view, opened on `section`, once the first settings read has
 * landed and the rail is up. Use with the mocks described in testApi.ts.
 */
export async function renderSettings(section: SectionId = "downloads") {
  const utils = render(
    <ToastProvider>
      <SettingsView initialSection={section} />
    </ToastProvider>,
  );
  await screen.findByRole("navigation", { name: "Settings sections" });
  return utils;
}

/** The rail entry for a section, by its label. */
export const railItem = (label: string) =>
  screen.getByRole("button", { name: label }) as HTMLButtonElement;
