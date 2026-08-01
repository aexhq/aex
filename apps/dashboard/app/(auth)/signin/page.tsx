import { Notice } from "../../../src/ui/components";

export const metadata = { title: "Sign in — AEX" };

/**
 * TODO(cross-stream): the browser sign-in ceremony is owned by central identity.
 *
 * The exchange this page needs — `POST /internal/v1/identity/users/resolutions`,
 * which turns a validated provider callback into an `aex_ds_` browser session — is
 * not published in this branch, and neither is the email-challenge pair. The
 * provider buttons are therefore rendered disabled with the reason stated. They are
 * not wired to a handler that would 404, and no local credential path is invented
 * to stand in for the missing one.
 */
export default function SignIn() {
  return (
    <main id="main" className="frame" style={{ maxWidth: "26rem", paddingBlock: "var(--space-8)" }}>
      <div className="stack">
        <div className="stack-tight">
          <h1>Sign in to AEX</h1>
          <p className="small muted">
            Browser sessions are issued by the AEX identity service. The dashboard never holds a
            password, a provider token, or a database credential.
          </p>
        </div>

        <div className="card">
          <div className="card-body stack">
            <button type="button" className="button" disabled aria-describedby="signin-state">
              Continue with GitHub
            </button>
            <button type="button" className="button" disabled aria-describedby="signin-state">
              Continue with Google
            </button>
          </div>
        </div>

        <div id="signin-state">
          <Notice status="warning" title="Browser sign-in is not available in this build">
            <p className="small">
              The session exchange that turns a provider callback into an AEX browser session has
              not shipped yet. Until it does, use the CLI device flow — <code className="mono">aex
              auth login</code> — which authenticates against the same account.
            </p>
          </Notice>
        </div>

        <p className="small muted">
          <a href="https://aex.dev/docs">Documentation</a>
        </p>
      </div>
    </main>
  );
}
