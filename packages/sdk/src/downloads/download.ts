import { AexConfigError } from "../transport/errors.js";

export interface DownloadRange { readonly start: number; readonly endExclusive: number }

export interface DownloadGrant {
  readonly url: string;
  readonly headers?: Readonly<Record<string, string>>;
  readonly expiresAt: string;
  readonly sizeBytes: number;
  readonly authorizedBytes: number;
  readonly measurementId: string;
  readonly sha256: string;
}

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
    const response = await this.#fetch(this.grant.url, {
      ...(this.grant.headers ? { headers: this.grant.headers } : {}),
      redirect: "error",
    });
    if (!response.ok) throw new AexConfigError(`download failed with status ${response.status}`);
    const bytes = new Uint8Array(await response.arrayBuffer());
    if (bytes.byteLength !== this.grant.authorizedBytes) {
      throw new AexConfigError(
        `download length mismatch: expected ${this.grant.authorizedBytes}, received ${bytes.byteLength}`,
      );
    }
    if (this.grant.authorizedBytes === this.grant.sizeBytes) {
      const digest = await crypto.subtle.digest("SHA-256", bytes);
      const actual = Array.from(new Uint8Array(digest), (value) => value.toString(16).padStart(2, "0")).join("");
      if (actual !== this.grant.sha256) throw new AexConfigError("download digest mismatch");
    }
    return bytes;
  }

  async text(): Promise<string> { return new TextDecoder().decode(await this.bytes()); }
  async json<T>(): Promise<T> { return JSON.parse(await this.text()) as T; }
}
