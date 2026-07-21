/**
 * Return the last non-trailing segment of either a POSIX- or Windows-shaped
 * path without applying host-specific path semantics.
 */
export function crossPlatformBasename(path: string): string {
  const normalised = path.replace(/\\/g, "/").replace(/\/+$/, "");
  return normalised.split("/").at(-1) ?? "";
}
