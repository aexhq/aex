// @ts-nocheck
import * as __fd_glob_28 from "../content/docs/reference/sdk/index.md?collection=docs"
import * as __fd_glob_27 from "../content/docs/guides/testing.md?collection=docs"
import * as __fd_glob_26 from "../content/docs/guides/telemetry.md?collection=docs"
import * as __fd_glob_25 from "../content/docs/guides/retries.md?collection=docs"
import * as __fd_glob_24 from "../content/docs/guides/resources.md?collection=docs"
import * as __fd_glob_23 from "../content/docs/guides/release.md?collection=docs"
import * as __fd_glob_22 from "../content/docs/guides/quickstart.md?collection=docs"
import * as __fd_glob_21 from "../content/docs/guides/networking.md?collection=docs"
import * as __fd_glob_20 from "../content/docs/guides/limits.md?collection=docs"
import * as __fd_glob_19 from "../content/docs/guides/files.md?collection=docs"
import * as __fd_glob_18 from "../content/docs/guides/errors.md?collection=docs"
import * as __fd_glob_17 from "../content/docs/guides/billing.md?collection=docs"
import * as __fd_glob_16 from "../content/docs/guides/authentication.md?collection=docs"
import * as __fd_glob_15 from "../content/docs/reference/index.md?collection=docs"
import * as __fd_glob_14 from "../content/docs/reference/cli.md?collection=docs"
import * as __fd_glob_13 from "../content/docs/reference/api.md?collection=docs"
import * as __fd_glob_12 from "../content/docs/concepts/sessions.md?collection=docs"
import * as __fd_glob_11 from "../content/docs/concepts/composition.md?collection=docs"
import * as __fd_glob_10 from "../content/docs/concepts/agent-tools.md?collection=docs"
import * as __fd_glob_9 from "../content/docs/support.md?collection=docs"
import * as __fd_glob_8 from "../content/docs/integrations.md?collection=docs"
import * as __fd_glob_7 from "../content/docs/index.md?collection=docs"
import * as __fd_glob_6 from "../content/docs/features.md?collection=docs"
import * as __fd_glob_5 from "../content/docs/examples.md?collection=docs"
import * as __fd_glob_4 from "../content/docs/changelog.md?collection=docs"
import { default as __fd_glob_3 } from "../content/docs/reference/meta.json?collection=docs"
import { default as __fd_glob_2 } from "../content/docs/guides/meta.json?collection=docs"
import { default as __fd_glob_1 } from "../content/docs/concepts/meta.json?collection=docs"
import { default as __fd_glob_0 } from "../content/docs/meta.json?collection=docs"
import { server } from 'fumadocs-mdx/runtime/server';
import type * as Config from '../source.config';

const create = server<typeof Config, import("fumadocs-mdx/runtime/types").InternalTypeConfig & {
  DocData: {
  }
}>({"doc":{"passthroughs":["extractedReferences"]}});

export const docs = await create.docs("docs", "content/docs", {"meta.json": __fd_glob_0, "concepts/meta.json": __fd_glob_1, "guides/meta.json": __fd_glob_2, "reference/meta.json": __fd_glob_3, }, {"changelog.md": __fd_glob_4, "examples.md": __fd_glob_5, "features.md": __fd_glob_6, "index.md": __fd_glob_7, "integrations.md": __fd_glob_8, "support.md": __fd_glob_9, "concepts/agent-tools.md": __fd_glob_10, "concepts/composition.md": __fd_glob_11, "concepts/sessions.md": __fd_glob_12, "reference/api.md": __fd_glob_13, "reference/cli.md": __fd_glob_14, "reference/index.md": __fd_glob_15, "guides/authentication.md": __fd_glob_16, "guides/billing.md": __fd_glob_17, "guides/errors.md": __fd_glob_18, "guides/files.md": __fd_glob_19, "guides/limits.md": __fd_glob_20, "guides/networking.md": __fd_glob_21, "guides/quickstart.md": __fd_glob_22, "guides/release.md": __fd_glob_23, "guides/resources.md": __fd_glob_24, "guides/retries.md": __fd_glob_25, "guides/telemetry.md": __fd_glob_26, "guides/testing.md": __fd_glob_27, "reference/sdk/index.md": __fd_glob_28, });