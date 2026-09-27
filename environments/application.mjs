import { timingSafeEqual } from "node:crypto";

export function authenticateApplication(headers, token) {
  const bearer = headers.authorization?.replace(/^Bearer /u, "");
  const equal = value => typeof value === "string" && Buffer.byteLength(value) === Buffer.byteLength(token)
    && timingSafeEqual(Buffer.from(value), Buffer.from(token));
  if (equal(bearer)) return undefined;
  if (!bearer?.startsWith("application.") || bearer.length > 8192) throw new Error("denied");
  const envelope = JSON.parse(Buffer.from(bearer.slice(12), "base64url").toString("utf8"));
  if (envelope.version !== 1 || !equal(envelope.token)
    || typeof envelope.authorization !== "string" || !/^http_[a-f0-9]{64}$/u.test(envelope.authorization)
    || typeof envelope.credential !== "string" || envelope.credential.length < 32 || envelope.credential.length > 4096) throw new Error("denied");
  return { authorization: envelope.authorization, credential: envelope.credential };
}
