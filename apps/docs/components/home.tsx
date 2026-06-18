import { publicSurface } from "@/lib/public-surface";

const typescriptExample = publicSurface.examples.typescriptLines.join("\n");
const cliExample = publicSurface.examples.cliLines.join("\n");

export function HomePage() {
  return (
    <div className="ant-home">
      <section className="ant-hero">
        <div className="ant-hero-copy">
          <p className="ant-kicker">Agent execution platform</p>
          <h1>{publicSurface.productName}</h1>
          <p className="ant-hero-lede">{publicSurface.oneLine}</p>
          <div className="ant-actions">
            <a className="ant-button ant-button-primary" href="/docs/guides/quickstart/">
              Start quickstart
            </a>
            <a className="ant-button ant-button-secondary" href="/docs/reference/sdk/">
              SDK reference
            </a>
          </div>
        </div>
      </section>

      <section className="ant-sample" aria-label="aex run example">
        <header>
          <h2>First run</h2>
          <a href="/docs/guides/quickstart/">Quickstart</a>
        </header>
        <div className="ant-code-tabs">
          <input className="ant-tab-input" defaultChecked id="ant-home-ts" name="ant-home-code" type="radio" />
          <label className="ant-tab-label" htmlFor="ant-home-ts">
            TypeScript
          </label>
          <input className="ant-tab-input" id="ant-home-cli" name="ant-home-code" type="radio" />
          <label className="ant-tab-label" htmlFor="ant-home-cli">
            CLI
          </label>
          <pre className="ant-code-panel ant-code-panel-ts">
            <code>{typescriptExample}</code>
          </pre>
          <pre className="ant-code-panel ant-code-panel-cli">
            <code>{cliExample}</code>
          </pre>
        </div>
      </section>

      <section className="ant-grid" aria-label="Documentation entry points">
        {publicSurface.featureAreas.map((area) => (
          <a className="ant-card" href={area.href} key={area.title}>
            <h2>{area.title}</h2>
            <p>{area.description}</p>
          </a>
        ))}
      </section>
    </div>
  );
}
