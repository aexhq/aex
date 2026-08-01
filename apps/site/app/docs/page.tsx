import { readFileSync } from "node:fs";
import { resolve } from "node:path";

export default function Docs() {
  const reference = readFileSync(resolve(process.cwd(), ".generated/api-reference.md"), "utf8");
  const operationCount = (reference.match(/^## `/gm) ?? []).length;
  return <main><h1>AEX API reference</h1><p>{operationCount} generated operations.</p><p>The full static reference is available in the site artifact.</p></main>;
}
