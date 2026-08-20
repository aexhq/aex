# Working in this repository

- Aex owns account, billing, control-plane, and Aex-specific public contracts. Neutral session and
  Brain-to-Hand contracts belong to `aexhq/brain`; consume them from one immutable Brain revision
  and do not copy or redefine them here.
- Change an Aex schema or its generator before generated Rust, TypeScript, or examples. Regenerate
  and commit the source and generated views together. Keep validated examples and round-trip tests
  for every message type.
- Public request bodies are strict. The Hand ABI ignores unknown fields. An absent provider counter
  stays absent; it is never reported as zero.
- Money is integer micro-USD. Top-ups use whole cents, division floors in the customer's favour,
  and the journal is the billing record. Usage ledger rows are absolute folds, never increments.
  Wire timestamps carry milliseconds.
- Keep docs and comments self-contained and use precise lifecycle terms: running, suspended,
  released, sync, persist, and checkpoint.
- Commit style: `area: imperative summary`.
