import type { ReactNode } from "react";

import "../../site/design/tokens.css";
import "../../site/design/components.css";
import "../../site/app/site.css";
import "./app.css";

const description =
  "Aex is the agent cloud platform. Give your agent a session it remembers, a machine to work on, and tools that touch real files.";

export const metadata = {
  metadataBase: new URL("https://aex.dev"),
  title: "Aex",
  description,
  icons: {
    icon: [
      { url: "/favicon.ico" },
      { url: "/icon-dark.png", media: "(prefers-color-scheme: dark)" },
    ],
    apple: [{ url: "/apple-touch-icon.png", sizes: "180x180", type: "image/png" }],
  },
  openGraph: {
    title: "Aex",
    description,
    type: "website",
    images: [{ url: "/og.png", width: 1200, height: 630, alt: "Aex" }],
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
