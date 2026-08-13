import { Notice } from "../../../src/ui/components";
import { safeReturnPath } from "../../../src/server/return-to";
import {
  PROVIDER_LABEL,
  configuredProviders,
  isSignInFailure,
  type ProviderId,
  type SignInFailure,
} from "../../../src/server/signin";

export const dynamic = "force-dynamic";
export const metadata = { title: "Sign in — AEX" };

const FAILURE: Readonly<Record<SignInFailure, { readonly title: string; readonly body: string }>> = {
  binding_failed: {
    title: "That sign-in link expired",
    body: "Start again here in the browser that will finish the sign-in.",
  },
  provider_denied: {
    title: "The provider did not grant access",
    body: "Nothing was created. You can try again.",
  },
  exchange_refused: {
    title: "AEX declined that sign-in",
    body: "The code was stale, already used, or did not identify a verified account.",
  },
  exchange_unavailable: {
    title: "The sign-in service did not answer",
    body: "Nothing was created. Try again in a moment.",
  },
};

function startHref(provider: ProviderId, returnTo: string): string {
  const path = `/api/auth/${provider}/start`;
  return returnTo === "/" ? path : `${path}?next=${encodeURIComponent(returnTo)}`;
}

export default async function SignIn({
  searchParams,
}: {
  searchParams: Promise<{ error?: string; next?: string }>;
}) {
  const { error, next } = await searchParams;
  const providers = configuredProviders();
  const failure = typeof error === "string" && isSignInFailure(error) ? FAILURE[error] : null;
  const returnTo = safeReturnPath(next);

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

        {failure ? (
          <Notice status="warning" title={failure.title} live>
            <p className="small">{failure.body}</p>
          </Notice>
        ) : null}

        {providers.length === 0 ? (
          <Notice status="serious" title="No sign-in provider is configured">
            <p className="small">
              This deployment cannot begin a browser sign-in. Use the CLI device flow — <code
              className="mono">aex auth login</code> — until Google sign-in is configured.
            </p>
          </Notice>
        ) : (
          <div className="card">
            <div className="card-body stack">
              {providers.map((provider) => (
                <a key={provider} className="button" href={startHref(provider, returnTo)}>
                  Continue with {PROVIDER_LABEL[provider]}
                </a>
              ))}
            </div>
          </div>
        )}

        {returnTo === "/" ? null : (
          <p className="small muted">
            After signing in you will return to <code className="mono">{returnTo}</code>.
          </p>
        )}

        <p className="small muted">
          <a href="/docs">Documentation</a>
        </p>
      </div>
    </main>
  );
}
