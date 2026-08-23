# @aexhq/environment

Provider-neutral profiles and authoring helpers for Aex environment extensions.

```ts
import { computer, defineEnvironment, linux } from "@aexhq/environment";

export const runtime = defineEnvironment({
  identity: "@vendor/env-runtime",
  protocol: "environment/v1",
  profile: computer({
    platform: linux.amd64,
    network: "allowlist",
    recovery: "retained",
  }),
  serialize: (options) => options,
  handle: (context) => ({
    status: () => context.request("GET", "/status"),
  }),
});
```

Profiles declare the execution contract. They intentionally do not advertise language runtimes:
prepared tool artifacts carry their own runtime and immutable dependencies.
