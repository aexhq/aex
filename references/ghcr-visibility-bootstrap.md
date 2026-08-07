---
title: GHCR public namespace bootstrap
description: One-time, fail-closed procedure for creating the five public digest-only OCI package namespaces.
keywords:
  - ghcr
  - oci
  - bootstrap
  - package visibility
audience: release maintainers
status: accepted
related:
  - references/develop.md
  - references/rules.md
  - .github/workflows/main.yml
  - .github/workflows/_build-artifacts.yml
---

# GHCR public namespace bootstrap

Normal publication requires each `ghcr.io/aexhq/aex-units/<unit>` package to
already be public. It checks package visibility before Docker login or push and
refuses a missing or private package. It never changes package visibility.

This one-time procedure exists because GitHub's supported Packages REST API
documents package and version reads/deletes/restores, but no package-visibility
mutation. A new container package may therefore be created private, after which
an organisation owner must make it public in the GitHub UI. The workflow does
not substitute an undocumented API call or claim that `GITHUB_TOKEN` can do
that. See GitHub's official [Packages REST API](https://docs.github.com/en/rest/packages/packages?apiVersion=2022-11-28)
and [package visibility guidance](https://docs.github.com/en/packages/learn-github-packages/configuring-a-packages-access-control-and-visibility).

## Irreversible boundary

Making a package public exposes every version to anonymous readers. GitHub
warns that a public package cannot be made private again. Before proceeding,
the release maintainer must record this acknowledgement from the blocker:

> I understand that public package visibility is a one-time operator action and
> that every published version becomes anonymously readable.

The five namespaces are `central-schema-admin`, `regional-stream`,
`regional-secret-key-admin`, `observation-export-task`, and `brain-mux`.

## Bootstrap

1. Confirm the source is the protected `aexhq/aex` `main` ref and the candidate
   passed both independent OCI reproducibility builds.
2. Create the repository Actions secret `AEX_GHCR_VISIBILITY_BOOTSTRAP` with a
   non-empty, randomly generated value. Its presence is the protected second
   authority; the workflow never prints or persists it.
3. Set the repository Actions variable `AEX_GHCR_VISIBILITY_BOOTSTRAP` to the
   exact value `requested-v1`. The variable is the explicit request authority.
   Neither authority works alone.
4. Rerun the failed protected-main workflow (or push the approved bootstrap
   commit). Each missing/private unit may push only its already-proven digest,
   using `push-by-digest` and `name-canonical`; no mutable tag is created.
5. Expect the run to fail. Download each `oci-artifact-<unit>` evidence bundle
   and inspect `visibility-after-push.json`. It must contain the typed blocker
   `oci-ghcr-bootstrap-awaiting-public` and the exact package settings URL.
6. As an organisation owner, open each blocker URL, acknowledge the irreversible
   boundary above, and change the package visibility to **Public** in GitHub's UI.
7. Rerun the same protected-main workflow. The pre-push visibility check must
   now report `public`; publication, official attestation verification, and a
   fresh anonymous Docker readback must all pass.
8. Delete the bootstrap repository variable and secret. A later normal run must
   still pass without either authority. Retain the typed bootstrap blockers and
   successful `oci-publication.json` records with the release evidence.

If any package is still missing/private without both authorities, or remains
private after the bootstrap push, stop. Do not add a visibility API call, skip
anonymous readback, use a mutable tag, or weaken the package preflight.
