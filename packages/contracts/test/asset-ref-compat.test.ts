import { describe, expect, it } from "vitest";
import {
  isAssetRef,
  isFileAssetRef,
  type AssetRef,
  type FileRef
} from "../src/index.js";

type IsExactly<Left, Right> =
  (<Value>() => Value extends Left ? 1 : 2) extends
  (<Value>() => Value extends Right ? 1 : 2)
    ? (<Value>() => Value extends Right ? 1 : 2) extends
      (<Value>() => Value extends Left ? 1 : 2)
      ? true
      : false
    : false;

type Assert<Condition extends true> = Condition;

type _FileRefIsExactlyAssetRef = Assert<IsExactly<FileRef, AssetRef>>;
type _FileRefHasNoNonAssetVariant = Assert<IsExactly<Exclude<FileRef, AssetRef>, never>>;
type _PredicatesHaveExactlyOneType = Assert<IsExactly<typeof isFileAssetRef, typeof isAssetRef>>;

describe("asset ref public compatibility", () => {
  it("keeps FileRef and AssetRef as the same public type", () => {
    const asset: AssetRef = {
      kind: "asset",
      assetId: "asset_01234567",
      name: "input.txt"
    };
    const file: FileRef = asset;
    const roundTrip: AssetRef = file;

    expect(roundTrip).toBe(asset);

    // @ts-expect-error - FileRef has no legacy `file` discriminator variant.
    const legacyFile: FileRef = { ...asset, kind: "file" };
    // @ts-expect-error - FileRef has no skill discriminator variant.
    const skill: FileRef = { ...asset, kind: "skill" };
    expect([legacyFile.kind, skill.kind]).toEqual(["file", "skill"]);
  });

  it("exports the deprecated name as the canonical function object", () => {
    expect(isAssetRef).toBeTypeOf("function");
    expect(isFileAssetRef).toBe(isAssetRef);
  });

  it.each([
    {
      label: "complete asset ref",
      ref: {
        kind: "asset",
        assetId: "asset_01234567",
        name: "input.txt"
      } as FileRef,
      expected: true
    },
    { label: "forged discriminator-only asset", ref: { kind: "asset" } as FileRef, expected: true },
    {
      label: "null-prototype asset",
      ref: Object.assign(Object.create(null) as object, { kind: "asset" }) as FileRef,
      expected: true
    },
    { label: "forged file discriminator", ref: { kind: "file" } as unknown as FileRef, expected: false },
    { label: "forged skill discriminator", ref: { kind: "skill" } as unknown as FileRef, expected: false },
    { label: "missing discriminator", ref: {} as FileRef, expected: false }
  ])("preserves open-world discriminator behavior for $label", ({ ref, expected }) => {
    expect(isAssetRef(ref)).toBe(expected);
    expect(isFileAssetRef(ref)).toBe(expected);
  });
});
