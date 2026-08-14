import { redirect } from "next/navigation";

import { currentBootstrap, signInDestination } from "../../src/server/context";

export const dynamic = "force-dynamic";

/**
 * There is no overview page.
 *
 * Every number an overview would carry already has one owner — sessions, usage,
 * billing — and a second copy is a second thing to keep true. The root sends you to
 * the last thing that is actually actionable.
 */
export default async function Root() {
  const result = await currentBootstrap();
  if (result.kind !== "ready") redirect(await signInDestination());
  redirect(`/w/${result.bootstrap.workspace.id}/sessions`);
}
