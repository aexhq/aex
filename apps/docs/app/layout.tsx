import type { Metadata } from "next";
import type { ReactNode } from "react";
import { Inter, JetBrains_Mono, Space_Grotesk } from "next/font/google";
import { DocsLayout } from "fumadocs-ui/layouts/docs";
import { Provider } from "@/app/provider";
import { SupportFooter } from "@/components/support-footer";
import { baseOptions } from "@/lib/layout.shared";
import { source } from "@/lib/source";
import "./global.css";

const spaceGrotesk = Space_Grotesk({
  subsets: ["latin"],
  weight: ["400", "500", "600", "700"],
  variable: "--font-space-grotesk",
  display: "swap"
});

const inter = Inter({
  subsets: ["latin"],
  variable: "--font-inter",
  display: "swap"
});

const jetbrainsMono = JetBrains_Mono({
  subsets: ["latin"],
  variable: "--font-jetbrains-mono",
  display: "swap"
});

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
    <html
      lang="en"
      className={`${spaceGrotesk.variable} ${inter.variable} ${jetbrainsMono.variable}`}
      suppressHydrationWarning
    >
      <body className="flex min-h-screen flex-col font-sans antialiased">
        <Provider>
          <DocsLayout
            {...baseOptions()}
            tree={source.getPageTree()}
            tabs={false}
            themeSwitch={{ enabled: false }}
            sidebar={{
              defaultOpenLevel: 1,
              prefetch: false,
              footer: <SupportFooter />
            }}
          >
            {children}
          </DocsLayout>
        </Provider>
      </body>
    </html>
  );
}
