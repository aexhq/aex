import type { ReactNode } from "react";

import "../../site/design/tokens.css";
import "../../site/design/components.css";
import "../../site/app/site.css";
import "./app.css";

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
      <body>{children}</body>
    </html>
  );
}
