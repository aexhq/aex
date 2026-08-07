-- aex-migration: tx=yes destructive=no phase=baseline
-- 20260801000900_baseline_seed_pricing_context.sql — the one public pricing
-- context, so that a reservation and a usage fact have a rate book to name.
--
-- `finance.reservation.pricing_version` and `finance.usage_inbox.pricing_version`
-- are both foreign keys into `finance.pricing_context`, and `grants.toml` grants
-- no role `INSERT` on that table — `aex_finance_settlement` holds `SELECT` and
-- nothing else does. A migration is therefore the only writer the table has, and
-- until one runs no reservation and no usage fact can be stored at all.
--
-- What this seeds is the **synthetic zero book**, `synthetic-zero-v1`: the exact
-- value `AEX_FINANCE_API_DEFAULT_PRICING_VERSION`, `AEX_PRICING_VERSION` and
-- every usage producer already name. It prices all four meters of
-- `aex_internal_contracts::usage::Meter` at exactly zero and declares
-- `billingActive = false`.
--
-- It is deliberately **not** the priced launch card, and this file is not the
-- place one can be installed. OD-09 keeps commercial values out of public
-- source; `aex_usage_rating::RateContext::open` refuses a zero or inactive book
-- on an active plane; and a central migration applies byte-identically to dev
-- and prd, so a plane-specific card cannot be expressed here at all. Installing
-- a priced context needs three things that do not exist yet: the accepted rate
-- values, a rate-book signing key, and a production `BookVerifier` bound to it.
--
-- `signature` is the text `unsigned:synthetic-zero-v1`. That is a statement that
-- the artifact is unsigned, not a signature: no verifier can accept it, and the
-- column is `NOT NULL` so the absence has to be spelled rather than omitted. It
-- is safe only because the book is zero and inactive, and it is not a template
-- for a priced context.
--
-- The book is written once, in canonical JCS form, and both the digest and the
-- stored document derive from that single literal: `content_sha256` is SHA-256
-- over exactly the bytes a signature would cover — the document without its
-- `signature` member — and `rate_book` is those bytes plus the member. Neither
-- can drift from the other, and `crates/aex-usage-rating` asserts offline that
-- the literal is canonical, loads under the production rate-book reader, prices
-- every meter to zero, and is refused on an active plane.
--
-- `effective_from` is the epoch and `effective_to` is unbounded: a shadow
-- context that expired would fail the replay of an older fact, and a zero book
-- has no window worth defending.

INSERT INTO finance.pricing_context (
  pricing_version, content_sha256, signature, rate_book, rounding_rule,
  currency, billing_active, effective_from, effective_to
)
SELECT
  'synthetic-zero-v1'::text,
  sha256(convert_to(signed_content, 'UTF8')),
  convert_to('unsigned:synthetic-zero-v1', 'UTF8'),
  signed_content::jsonb
    || jsonb_build_object('signature', 'unsigned:synthetic-zero-v1'::text),
  'half_even'::text,
  'USD'::finance.currency,
  false,
  TIMESTAMPTZ '1970-01-01 00:00:00+00',
  NULL::timestamptz
FROM (
  SELECT $rate_book${"billingActive":false,"currency":"USD","effectiveFrom":"1970-01-01T00:00:00Z","effectiveTo":null,"pricingVersion":"synthetic-zero-v1","rateCardHash":"sha256:7d2738abcb93d7a75e76975ada330bfa1cb762f8372d5e92bcbe4eebfa746d8b","rates":{"compute.millicpu_ms.v1":{"denominatorUnits":"1","numeratorMicrousd":"0"},"data_transfer.egress_byte.v1":{"denominatorUnits":"1","numeratorMicrousd":"0"},"memory.byte_ms.v1":{"denominatorUnits":"1","numeratorMicrousd":"0"},"storage.byte_min.v1":{"denominatorUnits":"1","numeratorMicrousd":"0"}},"roundingRule":"half_even","schemaVersion":1}$rate_book$::text AS signed_content
) AS artifact;
