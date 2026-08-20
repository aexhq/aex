# Working in this repository (for humans and agents)

* Aex owns account, billing, control-plane, and Aex-only public contracts. Neutral session and
  Brain↔Hand contracts are owned by `aexhq/brain`; consume them from an immutable Brain identity and
  do not copy or redefine them here. During the clean pre-GA migration, remove the old generated
  duplicates only after downstream consumers use Brain's types.
* For Aex-owned schemas, never hand-edit generated Rust/TypeScript files or examples: change the
  schema (or generator), regenerate, and commit source plus generated views together. Every Aex
  message type must retain validated examples and Rust/TypeScript round-trip coverage.
* Fail fast: request bodies on the public API are strict; the ABI ignores unknown fields; absent
  provider counters stay absent (never 0).
* Plain English in docs and comments; name operations precisely (running / suspended / released /
  sync / persist / checkpoint). Cite spike ids (PD-x, HD-x) when a decision rests on a measurement;
  the decision record is `aex-research/docs/ARCHITECTURE-v1.md` (private sibling repo).
* Money is integer micro-USD everywhere (top-up amounts are whole cents — the payment surface);
  divisions floor, in the customer's favor. The journal is the billing record: rating is a pure
  fold over the session event log, and the control plane's `usage:` ledger rows are absolute
  overwrites, never increments. Timestamps on the wire carry milliseconds — billing folds on them.
* Commit style: `area: imperative summary` (e.g. `contracts: add sync op to ABI v1`).
