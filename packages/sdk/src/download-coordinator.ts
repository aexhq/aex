import type { DownloadGrant } from "@aexhq/contracts";
import {
  planDownloadRanges,
  type DownloadRange
} from "./client-v1.js";

export interface CoordinateDownloadGrantsInput {
  readonly sizeBytes: number;
  readonly sha256: string;
  readonly range?: DownloadRange;
  readonly mint: (
    range: DownloadRange,
    position: { readonly index: number; readonly count: number }
  ) => Promise<DownloadGrant>;
}

export interface CoordinatedDownloadGrant {
  readonly range: DownloadRange;
  readonly grant: DownloadGrant;
  readonly index: number;
  readonly count: number;
}

/**
 * Mints one grant at a time for a provider-bounded full or partial download.
 * It validates object identity and authorization metadata before yielding the
 * bearer URL, so callers can fetch and discard each grant before minting the
 * next one.
 */
export async function* coordinateDownloadGrants(
  input: CoordinateDownloadGrantsInput
): AsyncGenerator<CoordinatedDownloadGrant> {
  const ranges = planDownloadRanges(input.sizeBytes, input.range);
  for (const [index, range] of ranges.entries()) {
    const grant = await input.mint(range, { index, count: ranges.length });
    const authorizedBytes = range.endExclusive - range.start;
    if (
      grant.sizeBytes !== input.sizeBytes ||
      grant.authorizedBytes !== authorizedBytes ||
      grant.sha256 !== input.sha256
    ) {
      throw new Error(
        "download grant does not match the planned range and immutable object"
      );
    }
    yield {
      range,
      grant,
      index,
      count: ranges.length
    };
  }
}
