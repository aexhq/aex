/**
 * The TypeDoc JSON -> Markdown projection used by the generated SDK reference.
 *
 * Split out of `generate-all.mjs`, which is a pipeline: read sources, run
 * external tools, write files. This module is the one part of it that is a pure
 * transform — TypeDoc's reflection JSON in, Markdown out, no filesystem and no
 * subprocess — so it can be read and reasoned about without the pipeline around
 * it.
 *
 * The four exported entry points are the pipeline's whole use of it:
 * `flattenTypedoc` + `isPublicDocItem` select what is public, `groupByKind`
 * buckets it, and `renderKindSection` renders one bucket. Everything else here
 * is module-private, exactly as it was when it lived next to the pipeline.
 */
export function flattenTypedoc(node, acc = []) {
  if (!node || typeof node !== "object") return acc;
  if (node.kind && node.name) acc.push(node);
  for (const child of node.children ?? []) flattenTypedoc(child, acc);
  return acc;
}

export function isPublicDocItem(item) {
  if (!item.name || item.name.startsWith("_") || item.name === "default") return false;
  if (item.flags?.isPrivate || item.flags?.isProtected) return false;
  return ["Class", "Interface", "Function", "Type alias", "Variable", "Enumeration"].includes(kindName(item));
}

export function groupByKind(items) {
  const groups = new Map();
  for (const item of items) {
    const key = kindName(item).replace(/\s+/g, "");
    const arr = groups.get(key) ?? [];
    arr.push(item);
    groups.set(key, arr);
  }
  for (const arr of groups.values()) arr.sort((a, b) => a.name.localeCompare(b.name));
  return groups;
}

export function renderKindSection(kind, items = []) {
  const labels = new Map([
    ["Class", "Classes"],
    ["Interface", "Interfaces"],
    ["Function", "Functions"],
    ["TypeAlias", "Type Aliases"],
    ["Variable", "Variables"],
    ["Enumeration", "Enumerations"]
  ]);
  const label = labels.get(kind) ?? `${kind}s`;
  const lines = [`## ${label}`, ""];
  for (const item of items) {
    lines.push(`### \`${item.name}\``, "");
    const summary = commentText(item.comment) || signatureComment(item) || "";
    if (summary) lines.push(summary, "");
    const signatures = item.signatures ?? [];
    for (const signature of signatures) {
      const params = (signature.parameters ?? []).map((param) => param.name).join(", ");
      lines.push(`\`${item.name}(${params})\``, "");
    }
    const members = (item.children ?? []).filter((child) => isPublicMember(child));
    if (members.length > 0) {
      lines.push("Members:", "");
      for (const member of members) {
        lines.push(`- \`${member.name}\`${kindName(member) ? ` (${kindName(member)})` : ""}`);
      }
      lines.push("");
    }
  }
  return lines.join("\n");
}

function isPublicMember(member) {
  if (!member.name || member.name.startsWith("_") || member.name.startsWith("#")) return false;
  if (member.flags?.isPrivate || member.flags?.isProtected) return false;
  if (member.inheritedFrom) return false;
  return ["Method", "Property", "Accessor", "Constructor", "Function"].includes(kindName(member));
}

function kindName(item) {
  if (item.kindString) return item.kindString;
  const names = new Map([
    [4, "Namespace"],
    [8, "Enumeration"],
    [16, "Enumeration member"],
    [32, "Variable"],
    [64, "Function"],
    [128, "Class"],
    [256, "Interface"],
    [512, "Constructor"],
    [1024, "Property"],
    [2048, "Method"],
    [262144, "Accessor"],
    [4194304, "Type alias"]
  ]);
  return names.get(item.kind) ?? "";
}

function commentText(comment) {
  const parts = comment?.summary ?? [];
  return parts.map((part) => part.text ?? "").join("").trim();
}

function signatureComment(item) {
  return commentText(item.signatures?.[0]?.comment);
}
