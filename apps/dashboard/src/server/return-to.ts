/** The request header the dashboard proxy uses to preserve a destination. */
export const RETURN_HEADER = "x-aex-return-to";

const MAX_LENGTH = 512;
const REFUSED = ["/signin", "/api"] as const;
const UNSAFE_CHARACTER = /[\u0000-\u001f\u007f\\]/;

/** Narrow an untrusted destination to one normalized same-origin path. */
export function safeReturnPath(raw: string | null | undefined): string {
  if (typeof raw !== "string") return "/";
  if (raw.length === 0 || raw.length > MAX_LENGTH) return "/";
  if (raw[0] !== "/" || raw[1] === "/" || raw[1] === "\\") return "/";
  if (UNSAFE_CHARACTER.test(raw)) return "/";
  const path = raw.split("?")[0]!.split("#")[0]!;
  let resolved: string;
  try {
    resolved = new URL(path, "https://return.invalid").pathname;
  } catch {
    return "/";
  }
  if (resolved !== path) return "/";
  for (const prefix of REFUSED) {
    if (path === prefix || path.startsWith(`${prefix}/`)) return "/";
  }
  return raw;
}

export function signInPath(returnTo: string | null | undefined): string {
  const safe = safeReturnPath(returnTo);
  return safe === "/" ? "/signin" : `/signin?next=${encodeURIComponent(safe)}`;
}
