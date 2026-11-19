import type { RecSort, Recommendation } from "../../types";

/**
 * The Recommendations in the order picked: "Most videos" by the channel's
 * total uploads, "Most popular" by how many of your channels point at it.
 * Most first, ties by title, an unknown count last. A copy: the list it was
 * given is the fetch's, and stays as it came.
 */
export function sortRecommendations(list: readonly Recommendation[], sort: RecSort): Recommendation[] {
  const key = (r: Recommendation) => (sort === "popular" ? r.popularity : r.videoCount) ?? -1;
  return [...list].sort((a, b) => key(b) - key(a) || a.title.localeCompare(b.title));
}
