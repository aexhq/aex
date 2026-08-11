/** Internal size authority shared by the Node live-file flows. */
export const MAX_LIVE_FILE_BYTES = 5_368_709_120;

/** Whether an exact file length is representable and within the public limit. */
export function liveFileSizeIsAdmitted(sizeBytes: number): boolean {
  return Number.isSafeInteger(sizeBytes) && sizeBytes >= 0 && sizeBytes <= MAX_LIVE_FILE_BYTES;
}
