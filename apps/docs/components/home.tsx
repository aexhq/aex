const example = `import { RunModels } from "@aexhq/sdk";

const runId = await aex.submit({
  provider: "anthropic",
  model: RunModels.CLAUDE_HAIKU_4_5,
  prompt: "Write the report and save outputs.",
  secrets: {
    apiKey: process.env.ANTHROPIC_API_KEY!
  }
});

for await (const event of aex.stream(runId)) {
  console.log(event.type);
}`;

export function HomePage() {
  return (
    <div className="ant-home">
      <section className="ant-hero">
        <div className="ant-hero-copy">
          <p className="ant-kicker">Durable agent runs</p>
          <h1>Run autonomous agents across providers with one SDK.</h1>
          <p className="ant-hero-lede">
            aex is the serverless control plane for autonomous agent sessions: submit the task,
            stream a unified event log, capture outputs, and archive the run record through the managed runtime.
          </p>
          <div className="ant-actions">
            <a className="ant-button ant-button-primary" href="/docs/guides/quickstart/">
              Start quickstart
            </a>
            <a className="ant-button ant-button-secondary" href="/docs/reference/sdk/">
              SDK reference
            </a>
          </div>
          <div className="ant-chip-row" aria-label="Supported providers">
            <span className="ant-chip">anthropic</span>
            <span className="ant-chip">deepseek</span>
            <span className="ant-chip">openai</span>
            <span className="ant-chip">gemini</span>
            <span className="ant-chip">mistral</span>
          </div>
        </div>
        <aside className="ant-command-panel" aria-label="aex run example">
          <header>
            <span>submit a run</span>
            <span>TypeScript</span>
          </header>
          <pre>
            <code>{example}</code>
          </pre>
        </aside>
      </section>

      <section className="ant-grid" aria-label="Documentation entry points">
        <a className="ant-card" href="/docs/concepts/runs/">
          <h2>Runs</h2>
          <p>The immutable submission unit, lifecycle, idempotency model, event stream, and archived run record.</p>
        </a>
        <a className="ant-card" href="/docs/guides/product-boundaries/">
          <h2>Capabilities & boundaries</h2>
          <p>What aex owns, what providers and infrastructure own, and which claims are out of scope.</p>
        </a>
        <a className="ant-card" href="/docs/concepts/providers-and-runtimes/">
          <h2>Providers & runtimes</h2>
          <p>
            How provider selection maps to the managed runtime without changing the user-facing surface.
          </p>
        </a>
        <a className="ant-card" href="/docs/concepts/secrets-byok/">
          <h2>Secrets & BYOK</h2>
          <p>
            Per-run provider keys, MCP credentials, and proxy endpoint auth with secrets kept out of the runtime archive.
          </p>
        </a>
      </section>
    </div>
  );
}
