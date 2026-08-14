"use client";

import { useState } from "react";

export function AccountMenu({ email, userId }: { email: string; userId: string }) {
  const [failed, setFailed] = useState(false);

  async function signOut(): Promise<void> {
    const match = /(?:^|;\s*)__Host-aex_csrf=([^;]*)/.exec(document.cookie);
    const response = await fetch("/api/session", {
      method: "DELETE",
      headers: { "x-aex-csrf": match?.[1] ? decodeURIComponent(match[1]) : "" },
      credentials: "same-origin",
    }).catch(() => null);
    if (!response?.ok) {
      setFailed(true);
      return;
    }
    globalThis.location.assign("/signin");
  }

  return (
    <details className="menu">
      <summary aria-label="Account">{email}</summary>
      <div className="menu-panel end">
        <p className="menu-group mono">{userId}</p>
        <button type="button" onClick={() => void signOut()}>Sign out</button>
        {failed ? <p className="menu-group small" role="alert">You are still signed in — try again.</p> : null}
      </div>
    </details>
  );
}
