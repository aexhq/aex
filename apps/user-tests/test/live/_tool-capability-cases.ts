/**
 * The seeded case corpus for `live-sdk-tool-capability-fuzz.test.ts`.
 *
 * Every array below is `fc.sample` of a fast-check arbitrary at a FIXED seed,
 * so a failing cell is reproducible from its name alone. Extracted from the
 * suite because the corpus is a data declaration, not a test: it says what the
 * gate covers, and it should be readable — and re-seedable — without reading
 * the live driving that consumes it.
 *
 * `BASE_SEED` is offset per family so two families never draw the same words.
 * Changing a seed or `FUZZ_CASES` changes what the paid gate exercises; both
 * are declared here exactly once.
 */
import fc from "fast-check";

const FUZZ_CASES = 2;
const BASE_SEED = 0x0ae2026;

const wordArb = fc
  .array(fc.constantFrom(..."abcdefghijkmnpqrstuvwxyz"), { minLength: 4, maxLength: 9 })
  .map((chars) => chars.join(""));
const wordsArb = fc.array(wordArb, { minLength: 7, maxLength: 10 });

export interface FileCase {
  readonly id: number;
  readonly lines: readonly string[];
  readonly needle: string;
  readonly replacement: string;
  readonly headLines: number;
  readonly tailLines: number;
}

export const FILE_CASES: readonly FileCase[] = fc.sample(
  fc.record({
    lines: wordsArb,
    needle: wordArb.map((word) => `needle_${word}`),
    replacement: wordArb.map((word) => `replacement_${word}`),
    headLines: fc.integer({ min: 1, max: 3 }),
    tailLines: fc.integer({ min: 1, max: 3 })
  }),
  { seed: BASE_SEED + 1, numRuns: FUZZ_CASES }
).map((value, id) => ({ ...value, id }));

export interface ProcessCase {
  readonly id: number;
  readonly left: number;
  readonly right: number;
  readonly todoWords: readonly string[];
  readonly commitWord: string;
}

export const PROCESS_CASES: readonly ProcessCase[] = fc.sample(
  fc.record({
    left: fc.integer({ min: 2, max: 40 }),
    right: fc.integer({ min: 2, max: 40 }),
    todoWords: fc.array(wordArb, { minLength: 2, maxLength: 4 }),
    commitWord: wordArb
  }),
  { seed: BASE_SEED + 2, numRuns: FUZZ_CASES }
).map((value, id) => ({ ...value, id }));

export interface BackgroundCase {
  readonly id: number;
  readonly marker: string;
}

export const BACKGROUND_CASES: readonly BackgroundCase[] = fc.sample(
  wordArb.map((word) => ({ marker: `bg_${word}` })),
  { seed: BASE_SEED + 3, numRuns: FUZZ_CASES }
).map((value, id) => ({ ...value, id }));

export interface WebCase {
  readonly id: number;
  readonly query: string;
  readonly terms: readonly string[];
  readonly maxResults: number;
  readonly maxBytes: number;
}

export const WEB_CASES: readonly WebCase[] = fc.sample(
  fc.record({
    query: fc.constantFrom("Wikipedia free encyclopedia", "IANA example domains"),
    maxResults: fc.integer({ min: 1, max: 3 }),
    maxBytes: fc.integer({ min: 3500, max: 6000 })
  }),
  { seed: BASE_SEED + 4, numRuns: FUZZ_CASES }
).map((value, id) => ({
  ...value,
  id,
  terms: value.query.startsWith("Wikipedia")
    ? ["wikipedia", "encyclopedia"]
    : ["iana", "domain"]
}));

export interface SubagentCase {
  readonly id: number;
  readonly marker: string;
}

export const SUBAGENT_CASES: readonly SubagentCase[] = fc.sample(
  wordArb.map((word) => ({ marker: `child_${word}` })),
  { seed: BASE_SEED + 5, numRuns: FUZZ_CASES }
).map((value, id) => ({ ...value, id }));

export interface CustomCase {
  readonly id: number;
  readonly text: string;
  readonly count: number;
  readonly enabled: boolean;
  readonly tags: readonly string[];
  readonly appMode: string;
}

export const CUSTOM_CASES: readonly CustomCase[] = fc.sample(
  fc.record({
    text: wordArb,
    count: fc.integer({ min: 1, max: 4 }),
    enabled: fc.boolean(),
    tags: fc.array(wordArb, { minLength: 1, maxLength: 3 }),
    appMode: wordArb.map((word) => `mode_${word}`)
  }),
  { seed: BASE_SEED + 6, numRuns: FUZZ_CASES }
).map((value, id) => ({ ...value, id }));
