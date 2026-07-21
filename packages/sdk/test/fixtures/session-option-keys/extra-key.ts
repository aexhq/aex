import type { ExactKeySet } from "../../../src/session-validate.js";

type Assert<T extends true> = T;

interface ExampleOptions {
  readonly existing: string;
}

// @ts-expect-error Listed keys must all belong to the public option shape.
const EXTRA_KEY_OPTIONS = ["existing", "notPublic"] as const satisfies readonly (keyof ExampleOptions)[];

// @ts-expect-error The reverse exact-set comparison must reject extra runtime vocabulary too.
export type ExtraKeyRejected = Assert<ExactKeySet<ExampleOptions, typeof EXTRA_KEY_OPTIONS>>;
