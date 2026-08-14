import type { Metadata } from "next";

import { DocsPage, docsPage } from "../../../../site/app/_components/docs";

export const metadata: Metadata = {
  title: docsPage.title,
  description: docsPage.description,
};

export default function Docs() {
  return <DocsPage />;
}
