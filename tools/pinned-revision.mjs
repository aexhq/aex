import { readFileSync } from "node:fs";
import path from "node:path";
import process from "node:process";

const cargo = readFileSync(path.resolve(import.meta.dirname, "../Cargo.toml"), "utf8");
// Public npm packages remain on the existing 0.2 package source. A Rust-only runtime repair may
// advance the hosted Brain revision without mutating or republishing those immutable tarballs, so
// package build/smoke jobs intentionally resolve their own exact source identity.
const BRAIN_PACKAGES_REVISION = "af7e68e9ba987564952e51297a27ceeb193fa99e";
if (process.argv[2] === "brain-packages") {
  process.stdout.write(BRAIN_PACKAGES_REVISION);
  process.exit(0);
}
const groups = { brain: ["brain-protocol"] };
const selected = groups[process.argv[2]];
if (selected === undefined) {
  throw new Error("usage: pinned-revision.mjs brain|brain-packages");
}
const revisions = selected.map((name) => {
  const match = cargo.match(
    new RegExp(`^${name}\\s*=\\s*\\{[^\\n]*\\brev\\s*=\\s*"([0-9a-f]{40})"`, "mu"),
  );
  if (match === null) throw new Error(`${name} must pin one exact 40-character Git revision`);
  return match[1];
});
if (new Set(revisions).size !== 1) {
  throw new Error(`${process.argv[2]} dependencies must all pin the same revision`);
}
process.stdout.write(revisions[0]);
