import type { ReactNode } from "react";

import { PublicShell } from "../../../site/app/_components/public-shell";

export default function PublicLayout({ children }: Readonly<{ children: ReactNode }>) {
  return <PublicShell loginHref="/signin?next=/app">{children}</PublicShell>;
}
