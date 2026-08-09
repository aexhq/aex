---
title: npm trusted publisher bootstrap
description: One-time, fail-closed registry-side procedure that lets the SDK publish with OIDC provenance and no stored token.
keywords:
  - npm
  - trusted publishing
  - oidc
  - bootstrap
  - provenance
audience: release maintainers
status: accepted
related:
  - references/develop.md
  - references/rules.md
  - references/ghcr-visibility-bootstrap.md
  - .github/workflows/main.yml
  - .github/workflows/_build-artifacts.yml
---

# npm trusted publisher bootstrap

The `sdk` unit publishes `@aexhq/sdk` with `npm publish --provenance` through npm
trusted publishing. There is no npm token in this repository, and the workflow
policy refuses to reintroduce one: `policy::lint_workflow_text` fails any
workflow line naming a long-lived npm credential, and the publish step asserts
that no stored credential exists in `~/.npmrc` before it reaches the registry.
A stored token would publish without provenance *and still succeed*, which is
the silent fallback trusted publishing exists to remove.

The credential is therefore minted per run: npm exchanges the job's GitHub OIDC
identity token for a short-lived publish credential. That exchange only works
once the package on npmjs.com carries a trusted publisher matching this run.
Until then publication fails closed, and the release cannot be certified.

## The caller-workflow exception

Configure the workflow filename as **`main.yml`**, not `_build-artifacts.yml`.

This is the one identity in this repository that does *not* name
`_build-artifacts.yml`, and it is the only way to get it wrong. npm validates
the OIDC `workflow_ref` claim, which for a `workflow_call` reusable workflow is
the **calling** workflow. The file that actually contains `npm publish` appears
only in `job_workflow_ref`, which npm does not read. npm documents this
directly: when a workflow uses `workflow_call` to invoke another workflow that
runs `npm publish`, "validation checks the calling workflow's name instead of
the workflow that actually contains the publish command, which can cause
configuration mismatches". See npm's
[trusted publishing documentation](https://docs.npmjs.com/trusted-publishers/).

Every *other* published identity — the build provenance subject, the certified
envelope, the npm and OCI readbacks, the cosign identity — is minted inside the
reusable workflow and so names `.github/workflows/_build-artifacts.yml`. Both
facts are correct at once. Do not "fix" the readback validators to agree with
the registry configuration, or the registry configuration to agree with them.

`id-token: write` must be granted in both the caller (`main.yml`'s `build` job)
and this reusable workflow's `build` job. Both already carry it; a reusable
workflow cannot hold more scope than its caller.

## Bootstrap

1. Confirm the package exists and the repository is ahead of it. The exchange
   is a publisher check, not a package check: a package that already serves an
   older version still fails until a publisher is configured.
2. As an npm owner of the `@aexhq` scope, open the package's **Settings** on
   npmjs.com and add a **GitHub Actions** trusted publisher:

   | Field | Value |
   | --- | --- |
   | Organization or user | `aexhq` |
   | Repository | `aex` |
   | Workflow filename | `main.yml` |
   | Environment | *(leave blank)* |
   | Allowed actions | `npm publish` |

   Leave **Environment** empty. This field is the trap that actually cost a
   day. The `build` job declares no `environment:`, so GitHub puts no
   `environment` claim in the token; an environment named here is a claim the
   run cannot satisfy, and the exchange is rejected exactly as if no publisher
   existed at all. A `npm-release` GitHub environment does exist in the
   repository, left over from an earlier plan, but no workflow declares it and
   nothing references it — its presence is not permission to name it here.

   If npm publishing should one day be gated on review, do not resolve it by
   adding `environment:` to the `build` job: that job is the whole artifact
   matrix, so a reviewer rule on `npm-release` would gate every OCI image and
   blob too. Scoping it correctly means carrying the unit kind in
   `MatrixEntry` and selecting the environment per kind.
3. Re-run the failed protected-main workflow. The `Build and publish / sdk` job
   must exchange the token, publish with provenance, and pass its own registry
   readback: the version npm serves back must match the packed `package.json`
   and the SHA-512 the registry reports must cover the same bytes as the
   SHA-256 in the envelope.
4. Confirm no credential was introduced. The publish step's `~/.npmrc`
   assertion must still pass, and `npm view @aexhq/sdk` must report the
   published version as provenance-attested.

## Reading a failure

A rejected exchange reaches the log as `npm error code ENEEDAUTH` — the same
code npm uses for "you never logged in", which is why the publish step runs at
`--loglevel verbose` and dumps the npm debug log rather than leaving it on a
runner about to be destroyed. Read the exchange line above the error, not the
error:

```
npm http fetch POST 404 https://registry.npmjs.org/-/npm/v1/oidc/token/exchange/package/@aexhq%2fsdk
npm verbose oidc Failed token exchange request with body message: OIDC token exchange error - package not found
```

`package not found` here does **not** mean the package is missing, and it does
not mean no publisher is configured. The 404 is the exchange endpoint reporting
that no configured publisher *matched this run's claims* — one response for
both "nothing is configured" and "something is configured that disagrees with
this run". `@aexhq/sdk` was serving 0.43.0 publicly, with a publisher naming
the right repository and the right workflow, and still answered this way
because that publisher carried an environment the job does not declare. Read
the 404 as a claim comparison, never as a statement about the package. The
publish step detects this exact response and prints the required configuration;
treat that as the authority over the raw npm wording.

Do not respond to a failed exchange by adding a stored npm token, dropping
`--provenance`, granting the job broader scope, or pointing the trusted
publisher at `_build-artifacts.yml`. If the exchange still fails with the table
above configured exactly, stop and confirm the npm client is at least 11.5.1 —
the workflow pins it because Node 22 ships npm 10.x, which performs no exchange
at all and falls back to asking for a credential.
