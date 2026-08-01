# Workload descriptors

One file per workload, at `tests/load/workloads/<owner>/<id>.toml`, validated
against `tests/load/schema/workload.schema.json` and parsed by
`aex_load_harness::WorkloadDescriptor`.

The owning stream authors its own rows. `release/policy/workload-registry.toml`
already names every blocking prelaunch gate, who owes it, and which live package
carries its `load` target; a gate with no descriptor here appears in
`release/unearned-evidence.json` as a pending row, never as a pass.

Rules a descriptor must satisfy, all enforced at parse time:

1. `kind = "slo"` is rejected. A prelaunch gate is a `budget` (an engineering
   threshold on a pinned shape, blocking, with a `source` naming its Area 10 or
   `PERF-*` row) or a `diagnostic` (recorded, never blocking).
2. `report.metrics` lists all eighteen mandatory metrics. A campaign that omits
   one is not comparable with any other campaign.
3. `arrival.mix` weights sum to 1.0; closed arrival declares `concurrency`, open
   arrival declares `rate_per_s`.
4. `tier` names a profile in `tests/load/profiles`.
5. `budget_micro_usd` may be unset while nothing is deployed; once a release
   candidate exists an unset ceiling is a failure, because a campaign with no
   ceiling cannot be stopped.

The executor itself is a `[[test]] name = "load"` target inside the live
companion named by `target`, driven by `aex_load_harness::Driver`. No package is
added under `tests/load/`.
