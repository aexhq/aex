import type { ExactKeySet } from "../../../src/session-validate.js";

type Assert<T extends true> = T;

interface ExampleOptions {
  readonly existing: string;
  readonly newlyAdded?: string;
}

const MISSING_KEY_OPTIONS = ["existing"] as const satisfies readonly (keyof ExampleOptions)[];

// @ts-expect-error The exact-set proof must reject an omitted public option key.
export type MissingKeyRejected = Assert<ExactKeySet<ExampleOptions, typeof MISSING_KEY_OPTIONS>>;
