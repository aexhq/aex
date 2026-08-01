import { readFileSync } from "node:fs";
import { resolve } from "node:path";

export default function Docs() {
  const reference = readFileSync(resolve(process.cwd(), ".generated/api-reference.md"), "utf8");
  const operationCount = (reference.match(/^## `/gm) ?? []).length;
  return (
    <section className="aex-section docs" aria-labelledby="docs-heading">
      <div className="aex-container aex-container--prose">
        <h1 id="docs-heading">AEX API reference</h1>
        <p className="docs__lede">{operationCount} generated operations.</p>
        <p className="docs__lede">The full static reference is available in the site artifact.</p>
      </div>
    </section>
  );
}
