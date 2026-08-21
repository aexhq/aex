import { readFileSync } from "node:fs";
import path from "node:path";
import process from "node:process";

const cargo = readFileSync(path.resolve(import.meta.dirname, "../Cargo.toml"), "utf8");
const groups = {
  brain: ["brain-protocol", "brain", "brain-aws", "brain-standalone"],
  hands: ["hand-brain-aws"],
};
const selected = groups[process.argv[2]];
if (selected === undefined) throw new Error("usage: pinned-revision.mjs brain|hands");
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
