/**
 * Response schemas for the `assets.*` family — the content-addressed byte store
 * every workspace resource is built on.
 *
 * `POST /assets/presign` has THREE distinct 200 bodies and they are genuinely
 * different objects, not one object with optional halves: a dedup hit carries
 * `contentType` and no upload grant, a single-PUT mint carries `uploadUrl` and
 * `requiredHeaders` and no `contentType`, and a multipart mint carries a
 * `multipart` block and no top-level upload fields. A union of three strict
 * objects says that; one object with everything optional would accept a
 * nonsensical mixture and assert almost nothing.
 */
import * as z from "zod/mini";
import {
  describeResponse,
  responseObject,
  wireLiteral,
  wireNonEmptyString,
  wireNonNegativeInteger,
  wirePositiveInteger,
  wireString
} from "./response-common.js";

const PresignedPartSchema = responseObject({
  partNumber: wirePositiveInteger,
  url: wireNonEmptyString
});

/** Dedup hit: the bytes are already stored, so no upload grant is minted. */
const AssetPresignExistingSchema = responseObject({
  ok: wireLiteral(true),
  exists: wireLiteral(true),
  assetId: wireNonEmptyString,
  contentHash: wireNonEmptyString,
  sizeBytes: wireNonNegativeInteger,
  contentType: wireString,
  storagePath: wireNonEmptyString
});

/** Single-PUT mint. Note the ABSENCE of `contentType` — the server sends none here. */
const AssetPresignSinglePutSchema = responseObject({
  ok: wireLiteral(true),
  exists: wireLiteral(false),
  assetId: wireNonEmptyString,
  contentHash: wireNonEmptyString,
  sizeBytes: wireNonNegativeInteger,
  storagePath: wireNonEmptyString,
  uploadUrl: wireNonEmptyString,
  requiredHeaders: z.record(wireString, wireString),
  expiresInSeconds: wirePositiveInteger
});

/** Multipart mint. No top-level `storagePath` — the key rides inside `multipart`. */
const AssetPresignMultipartSchema = responseObject({
  ok: wireLiteral(true),
  exists: wireLiteral(false),
  assetId: wireNonEmptyString,
  contentHash: wireNonEmptyString,
  sizeBytes: wireNonNegativeInteger,
  multipart: responseObject({
    uploadId: wireNonEmptyString,
    key: wireNonEmptyString,
    partSize: wirePositiveInteger,
    partCount: wirePositiveInteger,
    partUrls: z.array(PresignedPartSchema),
    expiresInSeconds: wirePositiveInteger
  })
});

export const AssetPresignResponseSchema = describeResponse(
  "AssetPresignResponse",
  "One of three: a dedup hit, a single-PUT upload grant, or a multipart upload plan.",
  z.union([AssetPresignExistingSchema, AssetPresignSinglePutSchema, AssetPresignMultipartSchema], {
    error:
      "asset presign response must be a dedup hit (exists:true), a single-PUT grant " +
      "(uploadUrl + requiredHeaders) or a multipart plan (multipart)"
  })
);

export const AssetFinalizeResponseSchema = describeResponse(
  "AssetFinalizeResponse",
  "The committed asset identity. Identical on the single-PUT and multipart-complete paths.",
  responseObject({
    ok: wireLiteral(true),
    exists: wireLiteral(true),
    assetId: wireNonEmptyString,
    contentHash: wireNonEmptyString,
    sizeBytes: wireNonNegativeInteger,
    contentType: wireString
  })
);

export const AssetMpuPresignPartsResponseSchema = describeResponse(
  "AssetMpuPresignPartsResponse",
  "Fresh presigned URLs for the requested multipart part numbers, in request order.",
  responseObject({
    ok: wireLiteral(true),
    partUrls: z.array(PresignedPartSchema),
    expiresInSeconds: wirePositiveInteger
  })
);

/**
 * `POST /assets/mpu/abort`.
 *
 * Worth knowing while reading a green C4: the handler swallows a failing S3
 * `AbortMultipartUpload` and still answers `{ ok: true }`, so this schema
 * passing does not mean the abort happened.
 */
export const AssetMpuAbortResponseSchema = describeResponse(
  "AssetMpuAbortResponse",
  "Acknowledgement of a multipart abort. `ok: true` even when the underlying S3 abort failed.",
  responseObject({ ok: wireLiteral(true) })
);
