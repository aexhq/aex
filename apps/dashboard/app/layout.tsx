import type { ReactNode } from "react";

import "../../site/design/tokens.css";
import "../../site/design/components.css";
import "../../site/app/site.css";
import "./app.css";

export const metadata = {
  metadataBase: new URL("https://aex.dev"),
  title: "AEX",
  description: "AEX runs agents in durable sessions with an isolated workspace.",
  openGraph: {
    title: "AEX",
    description: "AEX runs agents in durable sessions with an isolated workspace.",
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
      <body>{children}</body>
    </html>
  );
}
