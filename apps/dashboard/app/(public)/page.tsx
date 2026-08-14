import type { Metadata } from "next";

import { LandingPage, marketingPage } from "../../../site/app/_components/landing";

export const metadata: Metadata = {
  title: marketingPage.title,
  description: marketingPage.description,
};

export default function Home() {
  return <LandingPage />;
}
