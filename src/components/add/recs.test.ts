import { describe, expect, it } from "vitest";
import { sortRecommendations } from "./recs";
import type { Recommendation } from "../../types";

function rec(title: string, videoCount: number | null, popularity: number): Recommendation {
  return {
    id: `UC${title}`, title, handle: null, thumbPath: null, subscribers: null,
    videoCount, popularity, uploads: [],
  };
}

const titles = (list: Recommendation[]) => list.map((r) => r.title);

describe("sortRecommendations", () => {
  const list = [rec("Small", 3, 4), rec("Big", 900, 1), rec("Unknown", null, 9), rec("Also big", 900, 2)];

  it("puts the most uploads first, ties by title, unknown counts last", () => {
    expect(titles(sortRecommendations(list, "videos"))).toEqual(["Also big", "Big", "Small", "Unknown"]);
  });

  it("puts the channel most of yours point at first", () => {
    expect(titles(sortRecommendations(list, "popular"))).toEqual(["Unknown", "Small", "Also big", "Big"]);
  });

  it("leaves the list it was given alone", () => {
    sortRecommendations(list, "popular");
    expect(titles(list)).toEqual(["Small", "Big", "Unknown", "Also big"]);
  });
});
