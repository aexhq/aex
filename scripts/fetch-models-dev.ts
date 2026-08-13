//! Fetches the current models.dev snapshot and regenerates the admit table.
//!
//! Manual by design (no cron, no PR bot): run
//! `bun scripts/fetch-models-dev.ts`, review the diff, and commit. The change
//! then rides the normal release train; CI verifies the digest and the
//! generated table against the committed snapshot.
//!
//! Usage: `bun scripts/fetch-models-dev.ts [--keep]`
//!   --keep  leaves the fetched snapshot in place (default is no-op when
//!           unchanged; it always rewrites when it changed).

import { createHash } from "node:crypto";

const URL = "https://models.dev/api.json";
const SNAPSHOT_PATH = "release/models-dev/api.json";

const keep = process.argv.includes("--keep");

const response = await fetch(URL, {
  headers: { "user-agent": "aex-models-dev-fetch/1" },
  redirect: "follow",
});
if (!response.ok) {
  throw new Error(`models.dev fetch failed: HTTP ${response.status}`);
}
const body = Buffer.from(await response.arrayBuffer());

const existing = await Bun.file(SNAPSHOT_PATH)
  .arrayBuffer()
  .then((buffer) => Buffer.from(buffer))
  .catch(() => null);

if (existing && Buffer.compare(existing, body) === 0) {
  console.log("unchanged: the vendored snapshot is current");
  process.exit(0);
}

const digest = createHash("sha256").update(body).digest("hex");
await Bun.write(SNAPSHOT_PATH, body);
await Bun.write(
  "scripts/models.digest",
  `${digest}  ${SNAPSHOT_PATH}\n`,
);

if (!keep) {
  console.log(`updated: snapshot now ${digest}`);
  console.log("regenerate with `bun scripts/gen-models.ts` and review the diff");
}
