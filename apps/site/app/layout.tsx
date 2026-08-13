import type { ReactNode } from "react";

import "../design/index.css";
import "./site.css";
import { PublicShell } from "./_components/public-shell";

export const metadata = {
  metadataBase: new URL("https://aex.dev"),
  title: "AEX — infrastructure for agents that do real work",
  description: "Durable sessions, isolated workspaces, and observable tools for AI agents.",
  openGraph: {
    title: "AEX — infrastructure for agents that do real work",
    description: "Durable sessions, isolated workspaces, and observable tools for AI agents.",
    type: "website",
    images: [{ url: "/og.png", width: 1731, height: 909, alt: "Infrastructure for agents that do real work" }],
  },
  twitter: {
    card: "summary_large_image",
    images: ["/og.png"],
  },
};

export default function Layout({ children }: Readonly<{ children: ReactNode }>) {
  return (
    <html lang="en">
      <body>
        <PublicShell loginHref="https://aex.dev/signin?next=/app">{children}</PublicShell>
      </body>
    </html>
  );
}
