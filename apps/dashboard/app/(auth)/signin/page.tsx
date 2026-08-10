import { Notice } from "../../../src/ui/components";

export const metadata = { title: "Sign in — AEX" };

/**
 * TODO(dashboard): wire the two provider buttons to the published exchange.
 *
 * The server half now exists and its shape has changed. `POST /api/auth/sessions`
 * performs the OAuth authorization-code exchange itself, in `central-identity-api`,
 * and takes `{ provider, code, state, codeVerifier }`. It no longer accepts a
 * provider profile this page established, and the shared `exchangeSecret` that used
 * to prove the caller was first-party is deleted — with the code redeemed
 * server-side there is no caller asserting an identity for a secret to prove.
 *
 * What is still missing is entirely on this side, which is why the buttons stay
 * disabled rather than being wired to something half-built:
 *
 *  1. a handler per button that mints a PKCE `code_verifier`, stores it in a
 *     `__Host-` cookie (`HttpOnly`, `Secure`, `SameSite=Lax` — the redirect is a
 *     top-level GET), and redirects to the provider's authorize endpoint with
 *     `state` and `code_challenge` both set to its S256 challenge;
 *  2. a `/auth/callback` route that reads the cookie back, posts the four fields,
 *     and sets the returned `aex_ds_` credential with `dashboardSessionCookie`;
 *  3. `dashboard_session_create` added to `DASHBOARD_ROUTES` in
 *     `src/server/routes.ts`, which currently does not list it;
 *  4. the two public client ids and the one registered redirect URI as build
 *     configuration. The client *secrets* stay in `central-identity-api`'s bound
 *     secrets and must never reach this app.
 *
 * The verifier cookie is the whole CSRF story: the server refuses any callback
 * whose `state` is not the S256 challenge of the verifier presented with it, so a
 * cross-site forgery carrying an attacker's code and state cannot produce the
 * matching verifier from a victim's browser.
 */
export default function SignIn() {
  return (
    <main id="main" className="frame" style={{ maxWidth: "26rem", paddingBlock: "var(--aex-space-9)" }}>
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
