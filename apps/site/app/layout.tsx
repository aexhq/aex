import type { ReactNode } from "react";
import "./site.css";

export default function Layout({ children }: Readonly<{ children: ReactNode }>) {
  return (
    <html lang="en">
      <body><header><a href="/">AEX</a><nav><a href="/docs">Docs</a></nav></header>{children}<footer>Support: support@aex.dev</footer></body>
    </html>
  );
}
