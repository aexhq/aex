/**
 * Runner-agnostic env stubbing with explicit save/restore.
 *
 * No test-runner coupling: callers wire their own lifecycle, either through the
 * returned restore fn or by registering `afterEach(restoreEnv)` themselves.
 * Stubs are tracked in a LIFO registry so stacked stubs on the same key unwind
 * to the original value.
 */

interface EnvStubRecord {
  readonly key: string;
  readonly hadValue: boolean;
  readonly previousValue: string | undefined;
  restored: boolean;
}

const activeStubs: EnvStubRecord[] = [];

/**
 * Set (or, with `undefined`, delete) `process.env[key]`, remembering the prior
 * state. Returns an idempotent restore fn; `restoreEnv()` also restores it.
 */
export function stubEnv(key: string, value: string | undefined): () => void {
  if (typeof key !== "string" || key.length === 0) {
    throw new TypeError("stubEnv requires a non-empty string key");
  }
  const record: EnvStubRecord = {
    key,
    hadValue: key in process.env,
    previousValue: process.env[key],
    restored: false
  };
  activeStubs.push(record);
  if (value === undefined) {
    delete process.env[key];
  } else {
    process.env[key] = value;
  }
  return () => restoreRecord(record);
}

/** Restore every outstanding stub in reverse (LIFO) order. Safe to call with none active. */
export function restoreEnv(): void {
  while (activeStubs.length > 0) {
    restoreRecord(activeStubs[activeStubs.length - 1]!);
  }
}

function restoreRecord(record: EnvStubRecord): void {
  if (record.restored) return;
  record.restored = true;
  const index = activeStubs.indexOf(record);
  if (index !== -1) activeStubs.splice(index, 1);
  if (record.hadValue) {
    process.env[record.key] = record.previousValue;
  } else {
    delete process.env[record.key];
  }
}
