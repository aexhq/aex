import type { ReactNode } from "react";

import "./tokens.css";
import "./app.css";

export const metadata = {
  title: "AEX",
  description: "Sessions, observability, usage and workspace resources for AEX.",
};

export default function Layout({ children }: Readonly<{ children: ReactNode }>) {
  return (
    <html lang="en">
      <body>{children}</body>
    </html>
  );
}
