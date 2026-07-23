import { describe, expect, it } from "bun:test";
import { defineAllowedKeys } from "../src/allowed-keys.js";

interface OptionalShape {
  readonly required: string;
  readonly optional?: number;
}

type Branch =
  | { readonly kind: "left"; readonly left?: string }
  | { readonly kind: "right"; readonly right: number };

interface GenericResource {
  readonly kind: "resource";
  readonly id: string;
  readonly version: number;
  readonly name?: string;
}

const optionalKeys = defineAllowedKeys<OptionalShape>()("required", "optional");
const leftKeys = defineAllowedKeys<Extract<Branch, { readonly kind: "left" }>>()(
  "kind",
  "left"
);
const pickedKeys = defineAllowedKeys<Pick<GenericResource, "id" | "version">>()(
  "id",
  "version"
);
const genericKeys = defineAllowedKeys<GenericResource>()("kind", "id", "version", "name");

// @ts-expect-error Optional keys are still part of the exact accepted input shape.
defineAllowedKeys<OptionalShape>()("required");
// @ts-expect-error Extra or misspelled keys are rejected.
defineAllowedKeys<OptionalShape>()("required", "optional", "optionel");
// @ts-expect-error Duplicating one key cannot compensate for omitting another.
defineAllowedKeys<OptionalShape>()("required", "required");
// @ts-expect-error Keys from another discriminated branch are rejected.
defineAllowedKeys<Extract<Branch, { readonly kind: "left" }>>()("kind", "left", "right");

interface WidenedShape {
  readonly first: string;
  readonly newlyAdded?: boolean;
}
// @ts-expect-error Widening an input shape forces its allow-list to be updated.
defineAllowedKeys<WidenedShape>()("first");

describe("defineAllowedKeys types", () => {
  it("preserves exact readonly tuple order at runtime", () => {
    expect(optionalKeys).toEqual(["required", "optional"]);
    expect(leftKeys).toEqual(["kind", "left"]);
    expect(pickedKeys).toEqual(["id", "version"]);
    expect(genericKeys).toEqual(["kind", "id", "version", "name"]);
  });
});
