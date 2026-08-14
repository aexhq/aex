import { redirect } from "next/navigation";

import { currentBootstrap, signInDestination } from "../../../src/server/context";

export const dynamic = "force-dynamic";

/** Send an authenticated person to the first workspace they can act in. */
export default async function AppHome() {
  const result = await currentBootstrap();
  if (result.kind !== "ready") redirect(await signInDestination());
  redirect(`/w/${result.bootstrap.workspace.id}/sessions`);
}
