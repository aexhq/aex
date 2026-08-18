# Working in this repository (for humans and agents)

* `contracts/` is the source of truth. Never edit `crates/aex-contracts/src/{abi,session}.rs`,
  `packages/contracts/src/{abi,session,paths}.ts`, `packages/contracts/schemas/**`,
  `contracts/examples/**` or `contracts/abi/v1/tools/manifest.digest` by hand — change the schema
  (or `tools/make-examples.py`), run `tools/gen.sh`, commit both.
* Every message type must have at least one example under `contracts/examples/`; CI enforces
  coverage for ABI ops and session events and validates + round-trips every example in Rust and TS.
* Fail fast: request bodies on the public API are strict; the ABI ignores unknown fields; absent
  provider counters stay absent (never 0).
* Plain English in docs and comments; name operations precisely (running / suspended / released /
  sync / persist / checkpoint). Cite spike ids (PD-x, HD-x) when a decision rests on a measurement;
  the decision record is `agentsession/docs/ARCHITECTURE-v1.md`.
* Commit style: `area: imperative summary` (e.g. `contracts: add sync op to ABI v1`).
