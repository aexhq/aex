import type { Metadata } from "next";
import type { ReactNode } from "react";
import { GeistMono } from "geist/font/mono";
import { GeistSans } from "geist/font/sans";
import { DocsLayout } from "fumadocs-ui/layouts/docs";
import { Provider } from "@/app/provider";
import { baseOptions } from "@/lib/layout.shared";
import { source } from "@/lib/source";
import "./global.css";

export const metadata: Metadata = {
  metadataBase: new URL("https://aex.dev"),
  title: {
    default: "aex",
    template: "%s | aex"
  },
  description: "SDK and CLI docs for durable agent runs across providers."
};

export default function RootLayout({ children }: Readonly<{ children: ReactNode }>) {
  return (
    <html lang="en" className={`${GeistSans.variable} ${GeistMono.variable}`} suppressHydrationWarning>
      <body className="flex min-h-screen flex-col font-sans antialiased">
        <Provider>
          <DocsLayout
            {...baseOptions()}
            tree={source.getPageTree()}
            tabs={false}
            sidebar={{
              defaultOpenLevel: 1,
              prefetch: false
            }}
          >
            {children}
          </DocsLayout>
        </Provider>
      </body>
    </html>
  );
}
