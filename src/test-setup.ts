import { afterEach, vi } from "vitest";

/**
 * A React update outside `act` is only a console warning, so a race that lands
 * one on a slow CI runner scrolled past as noise for as long as nothing failed.
 * Turning it into a failure names the test and keeps the log readable. Still
 * printed as well, since React's own message carries the component's name.
 */
const actWarnings: string[] = [];
const consoleError = console.error.bind(console);
vi.spyOn(console, "error").mockImplementation((...args: unknown[]) => {
  const [format, ...rest] = args.map(String);
  if (format?.includes("not wrapped in act(")) {
    let i = 0;
    actWarnings.push(format.split("\n")[0].replace(/%s/g, () => rest[i++] ?? "%s"));
  }
  consoleError(...args);
});

afterEach(() => {
  if (actWarnings.length === 0) return;
  const seen = actWarnings.splice(0);
  throw new Error(`React state update outside act():\n  ${seen.join("\n  ")}`);
});
