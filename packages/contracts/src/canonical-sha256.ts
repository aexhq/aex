/**
 * Canonical algorithm-prefixed SHA-256 digest wire grammar.
 *
 * This accepts only the lowercase `sha256:` prefix followed by exactly 64
 * lowercase hexadecimal characters. Changing the algorithm, case, prefix, or
 * length is a wire-contract change.
 */
export const CANONICAL_SHA256_DIGEST_PATTERN = /^sha256:[0-9a-f]{64}$/;
