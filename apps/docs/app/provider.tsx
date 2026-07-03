"use client";

import type { ReactNode } from "react";
import { RootProvider } from "fumadocs-ui/provider/next";
import SearchDialog from "@/components/search";

export function Provider({ children }: { children: ReactNode }) {
  return (
    <RootProvider
      theme={{ forcedTheme: "light", defaultTheme: "light", enableSystem: false }}
      search={{
        SearchDialog
      }}
    >
      {children}
    </RootProvider>
  );
}
