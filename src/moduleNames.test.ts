import { describe, expect, it } from "vitest";

/**
 * macOS and Windows resolve file names without regard to case, Linux does not.
 * `./stats/SeriesBands` resolved to the component on this machine and to its
 * helpers (`seriesBands.ts`) on both of the others, so the 5.1.0 release check
 * failed there to build and to test while every check here passed. Nothing
 * that runs on Linux can see that, except a rule against the names themselves.
 */
describe("module names", () => {
  it("never differ from one another only in case", () => {
    const paths = Object.keys(import.meta.glob("./**/*.{ts,tsx}"));
    const byFolded = new Map<string, string>();
    const clashes: string[] = [];
    for (const path of paths) {
      // A module is imported without its extension, so that is what must be
      // unique; `X.tsx` beside `X.test.tsx` is two names, not one.
      const module = path.replace(/\.tsx?$/, "");
      const seen = byFolded.get(module.toLowerCase());
      if (seen !== undefined && seen !== module) clashes.push(`${seen} / ${module}`);
      else byFolded.set(module.toLowerCase(), module);
    }
    expect(clashes).toEqual([]);
  });
});
