import { AexConfigError } from "../transport/errors.js";
import type { DownloadGrant as ContractDownloadGrant } from "../generated/models.js";

export interface DownloadRange { readonly start: number; readonly endExclusive: number }

export type DownloadGrant = ContractDownloadGrant;

export const MAX_SINGLE_GET_BYTES = 5_000_000_000_000;
type FetchLike = (input: string | URL | Request, init?: RequestInit) => Promise<Response>;

export function planDownloadRanges(sizeBytes: number, selected?: DownloadRange): readonly DownloadRange[] {
  if (!Number.isSafeInteger(sizeBytes) || sizeBytes < 0) throw new AexConfigError("invalid sizeBytes");
  if (selected) {
    if (!Number.isSafeInteger(selected.start) || !Number.isSafeInteger(selected.endExclusive)
      || selected.start < 0 || selected.endExclusive <= selected.start || selected.endExclusive > sizeBytes) {
      throw new AexConfigError("invalid half-open download range");
    }
    return [selected];
  }
  if (sizeBytes === 0) return [];
  const ranges: DownloadRange[] = [];
  for (let start = 0; start < sizeBytes; start += MAX_SINGLE_GET_BYTES) {
    ranges.push({ start, endExclusive: Math.min(sizeBytes, start + MAX_SINGLE_GET_BYTES) });
  }
  return ranges;
}

export class Download {
  readonly grant: DownloadGrant;
  readonly #fetch: FetchLike;

  constructor(grant: DownloadGrant, fetchLike: FetchLike = globalThis.fetch) {
    this.grant = grant;
    this.#fetch = fetchLike.bind(globalThis);
  }

  async bytes(): Promise<Uint8Array> {
    if (Date.parse(this.grant.expiresAt) <= Date.now()) {
      throw new AexConfigError("download grant expired");
    }
    const sizeBytes = parseCanonicalBytes(this.grant.sizeBytes, "sizeBytes");
    const authorizedBytes = parseCanonicalBytes(this.grant.authorizedBytes, "authorizedBytes");
    if (authorizedBytes > sizeBytes) {
      throw new AexConfigError("download grant authorizes more bytes than the object contains");
    }
    const response = await this.#fetch(this.grant.url, {
      redirect: "error",
    });
    if (!response.ok) throw new AexConfigError(`download failed with status ${response.status}`);
    const bytes = new Uint8Array(await response.arrayBuffer());
    if (bytes.byteLength !== authorizedBytes) {
      throw new AexConfigError(
        `download length mismatch: expected ${authorizedBytes}, received ${bytes.byteLength}`,
      );
    }
    if (authorizedBytes === sizeBytes) {
      const digest = await crypto.subtle.digest("SHA-256", bytes);
      const actual = `sha256:${Array.from(
        new Uint8Array(digest),
        (value) => value.toString(16).padStart(2, "0"),
      ).join("")}`;
      if (actual !== this.grant.sha256) throw new AexConfigError("download digest mismatch");
    }
    return bytes;
  }

  async text(): Promise<string> { return new TextDecoder().decode(await this.bytes()); }
  async json<T>(): Promise<T> { return JSON.parse(await this.text()) as T; }
}

function parseCanonicalBytes(value: string, field: string): number {
  if (!/^(0|[1-9][0-9]*)$/.test(value)) {
    throw new AexConfigError(`download grant ${field} is not a canonical decimal`);
  }
  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed)) {
    throw new AexConfigError(`download grant ${field} exceeds SDK precision`);
  }
  return parsed;
}
