export function workerCountFromEnv(name: string, fallback: number): number {
  const raw = process.env[name];
  if (!raw) return fallback;

  const parsed = Number(raw);
  if (!Number.isInteger(parsed) || parsed < 1) {
    throw new Error(`${name} must be a positive integer, got ${JSON.stringify(raw)}`);
  }

  return parsed;
}
