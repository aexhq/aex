/**
 * Shared "did you mean?" suggester — the SSoT for near-miss enum hints across
 * the SDK and the CLI. Previously this lived only in the CLI, so the SDK could
 * not emit a fuzzy model/provider hint even though the docs promised one.
 */

/**
 * The closest candidate to `input` by a unique case-insensitive prefix match,
 * else by Levenshtein distance (≤ 2), else `undefined`. Ties break by
 * declaration order. Used on an invalid `--model` / `--provider` /
 * `--runtime-size` (and the SDK's unknown-model throw) to turn a flat rejection
 * into a fix hint.
 */
export function suggest(input: string, candidates: readonly string[]): string | undefined {
  const needle = input.trim();
  if (!needle) return undefined;
  // Unique case-insensitive prefix match first (cheap + intuitive).
  const lower = needle.toLowerCase();
  const prefixHits = candidates.filter((c) => c.toLowerCase().startsWith(lower));
  if (prefixHits.length === 1) return prefixHits[0];
  // Else nearest by edit distance, ties broken by declaration order.
  let best: string | undefined;
  let bestDist = Number.POSITIVE_INFINITY;
  for (const candidate of candidates) {
    const dist = levenshtein(needle, candidate);
    if (dist < bestDist) {
      bestDist = dist;
      best = candidate;
    }
  }
  return bestDist <= 2 ? best : undefined;
}

/** Levenshtein edit distance between two strings (rolling two-row DP). */
export function levenshtein(a: string, b: string): number {
  const m = a.length;
  const n = b.length;
  if (m === 0) return n;
  if (n === 0) return m;
  let prev = Array.from({ length: n + 1 }, (_, j) => j);
  let curr = new Array<number>(n + 1);
  for (let i = 1; i <= m; i++) {
    curr[0] = i;
    for (let j = 1; j <= n; j++) {
      const cost = a[i - 1] === b[j - 1] ? 0 : 1;
      curr[j] = Math.min(prev[j]! + 1, curr[j - 1]! + 1, prev[j - 1]! + cost);
    }
    [prev, curr] = [curr, prev];
  }
  return prev[n]!;
}
