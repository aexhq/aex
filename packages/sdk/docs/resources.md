---
title: Registered resources
---

# Registered resources

Workspace files, skills, tools, instructions, and MCP servers are addressed by
name. `set()` is an overwrite of that name; there is no public version-history
resource.

```ts
const instruction = await aex.workspace.instructions.set(
  "review-policy",
  { text: "Check tests before reporting completion." },
  {
    ifRevision: 3,
    idempotencyKey: "review-policy-update"
  }
);
```

Sessions refer to registered names:

```ts
const session = await aex.sessions.create({
  model: "openai/gpt-5",
  registered: {
    instructions: ["review-policy"],
    files: ["repository-context"]
  }
});
```

Blob-backed registrations accept inline bytes or a completed workspace upload.
Secrets have a separate metadata-only registry; secret values are never
returned after creation.
