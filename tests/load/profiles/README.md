# Load tier profiles

A tier is a named capacity vector that several workloads share, so two campaigns
at the same tier are comparable. A workload descriptor references one by id
(`tier = "t2"`); a descriptor naming a tier with no profile here fails with
`workload ... references tier ..., which has no profile in tests/load/profiles`.

Two tiers are declared, both grounded in an Area 10 row:

| Tier | Shape | Source |
| --- | --- | --- |
| `t2` | 100 concurrent full-path agents, 30 min | Area 10 T2; `PERF-02` |
| `t3` | 200 active / 500 offered, mixed and adversarial, 30 min | Area 10 T3; `Q-PERFORMANCE` |

Adding a tier requires a `source` naming the Area 10 or `PERF-*` row the vector
comes from. A tier invented for convenience is a number nobody can defend when a
campaign misses it.
