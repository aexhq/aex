#!/usr/bin/env bash
set -euo pipefail

# Gitleaks' CLI is MIT licensed and does not require the organization licence
# used by gitleaks-action. The digest is the upstream v8.30.1 linux_x64 release
# checksum; a changed or replaced archive fails before any downloaded code runs.
readonly GITLEAKS_VERSION="8.30.1"
readonly GITLEAKS_LINUX_X64_SHA256="551f6fc83ea457d62a0d98237cbad105af8d557003051f41f3e7ca7b3f2470eb"
readonly runner_temp="${RUNNER_TEMP:?RUNNER_TEMP must name the runner scratch directory}"
readonly workspace="${GITHUB_WORKSPACE:?GITHUB_WORKSPACE must name the checkout}"
readonly scratch="$(mktemp -d "${runner_temp%/}/gitleaks.XXXXXXXXXX")"

cleanup() {
  rm -rf -- "${scratch}"
}
trap cleanup EXIT

readonly archive="gitleaks_${GITLEAKS_VERSION}_linux_x64.tar.gz"
readonly archive_path="${scratch}/${archive}"
readonly release_url="https://github.com/gitleaks/gitleaks/releases/download/v${GITLEAKS_VERSION}/${archive}"

curl \
  --fail \
  --silent \
  --show-error \
  --location \
  --retry 3 \
  --retry-all-errors \
  --connect-timeout 15 \
  --max-time 120 \
  --proto '=https' \
  --tlsv1.2 \
  --output "${archive_path}" \
  "${release_url}"

printf '%s  %s\n' "${GITLEAKS_LINUX_X64_SHA256}" "${archive_path}" | sha256sum --check --strict
tar --extract --gzip --file "${archive_path}" --directory "${scratch}" gitleaks

"${scratch}/gitleaks" git \
  --redact=100 \
  --no-banner \
  --no-color \
  --exit-code=1 \
  --log-opts=--all \
  --timeout=300 \
  "${workspace}"
