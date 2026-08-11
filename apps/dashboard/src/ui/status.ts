import type { Status } from "./components";

/**
 * Lifecycle value to status role, in one table.
 *
 * `undefined` is a deliberate answer: an idle or suspended session is neither
 * good nor bad, and painting it green or amber would add a judgement the product
 * does not make.
 */
export function sessionStatus(value: string): Status | undefined {
  switch (value) {
    case "running": return "good";
    case "awaiting_approval": return "warning";
    case "terminating": return "warning";
    case "terminated": return "serious";
    case "deleting": return "serious";
    default: return undefined;
  }
}

export function label(value: string): string {
  return value.replaceAll("_", " ");
}

export function shortId(value: string): string {
  return value.length > 14 ? `${value.slice(0, 10)}…${value.slice(-4)}` : value;
}

export function instant(value: string | undefined): string {
  if (!value) return "—";
  const parsed = Date.parse(value);
  return Number.isNaN(parsed) ? value : new Date(parsed).toISOString().replace("T", " ").replace(".000Z", "Z");
}

export function bytes(value: string | undefined): string {
  if (!value) return "—";
  const size = Number(value);
  if (!Number.isFinite(size)) return value;
  const units = ["B", "KiB", "MiB", "GiB", "TiB"];
  let scaled = size;
  let unit = 0;
  while (scaled >= 1024 && unit < units.length - 1) {
    scaled /= 1024;
    unit += 1;
  }
  return `${unit === 0 ? scaled : scaled.toFixed(1)} ${units[unit]}`;
}
