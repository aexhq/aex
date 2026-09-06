# Aex

Aex is the official host of [Brain](https://github.com/aexhq/brain).
Brain owns agent session execution and durable history. Aex adds customer identity,
authorization, hosted admission, and access to those sessions.

This is the planning baseline for a complete rewrite. There is no server implementation
in this repository yet. The [roadmap](ROADMAP.md) and [MVP ADRs](docs/adr/README.md)
are proposals for discussion, except where they restate the owner's explicit requirements.

The intended public repository contains the reusable hosting service, its public contracts,
examples, tests, and documentation. Private infrastructure and business details live in
the separate Platform repository. Public builds must not require access to Platform.

Start with the roadmap; use the ADRs for the reasons and trade-offs behind the proposal.
