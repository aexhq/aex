import type { ReactNode } from "react";

import "../design/index.css";
import "./site.css";
import { PublicShell } from "./_components/public-shell";

export const metadata = {
  metadataBase: new URL("https://aex.dev"),
  title: "AEX",
  description: "AEX is a session-centered agent runtime with files, tools, MCP, subagents, telemetry, and prepaid controls.",
  openGraph: {
    title: "AEX",
    description: "AEX is a session-centered agent runtime with files, tools, MCP, subagents, telemetry, and prepaid controls.",
    type: "website",
    images: [{ url: "/og.png", width: 1200, height: 630, alt: "AEX" }],
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
