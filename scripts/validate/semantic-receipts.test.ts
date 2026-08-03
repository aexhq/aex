import { expect, test } from "bun:test";

type UnitRow = {
  id: string;
  package: string;
  required_receipts: string[];
};

type Producer = {
  unit: string;
  package: string;
  class: string;
  layer: string;
  filterset: string;
  features?: string[];
};

const semanticClasses = new Set(["contract", "property", "integration", "conformance"]);

test("semantic receipt producers exactly cover required semantic evidence", async () => {
  const units = Bun.TOML.parse(await Bun.file("release/units.toml").text()) as {
    unit: UnitRow[];
  };
  const registry = await Bun.file("release/semantic-receipts.json").json() as {
    schema: string;
    producers: Producer[];
  };
  expect(registry.schema).toBe("aex.semantic-receipts.v1");

  const rows = new Map(units.unit.map((unit) => [unit.id, unit]));
  const expected = units.unit.flatMap((unit) =>
    unit.required_receipts
      .filter((receipt) => semanticClasses.has(receipt))
      .map((receipt) => `${unit.id}:${receipt}`)
  ).sort();
  const actual = registry.producers.map((producer) => {
    const unit = rows.get(producer.unit);
    expect(unit, `unknown semantic unit ${producer.unit}`).toBeDefined();
    if (!unit) throw new Error(`unknown semantic unit ${producer.unit}`);
    expect(producer.package).toBe(unit.package);
    expect(semanticClasses.has(producer.class)).toBeTrue();
    expect(["unit", "integration"]).toContain(producer.layer);
    expect(producer.filterset.length).toBeGreaterThan(0);
    if (producer.features) {
      expect(producer.features.length).toBeGreaterThan(0);
      expect(producer.features.every((feature) => feature.length > 0)).toBeTrue();
    }
    return `${producer.unit}:${producer.class}`;
  }).sort();
  expect(new Set(actual).size).toBe(actual.length);
  expect(actual).toEqual(expected);
});

test("filtered Rust semantic producers name real test binaries", async () => {
  const registry = await Bun.file("release/semantic-receipts.json").json() as {
    producers: Producer[];
  };
  const tests = await Bun.file("release/test-registry.json").json() as {
    packages: Record<string, { name: string; kind: string }>;
  };
  const packagePaths = new Map(
    Object.entries(tests.packages)
      .filter(([, value]) => value.kind === "cargo")
      .map(([path, value]) => [value.name, path])
  );
  for (const producer of registry.producers.filter((row) => row.filterset !== "all()")) {
    const packagePath = packagePaths.get(producer.package);
    expect(packagePath, `missing registry path for ${producer.package}`).toBeDefined();
    const manifest = await Bun.file(`${packagePath}/Cargo.toml`).text();
    const binaries = [...producer.filterset.matchAll(/binary\(([^)]+)\)/gu)]
      .map((match) => match[1]);
    expect(binaries.length).toBeGreaterThan(0);
    for (const binary of binaries) {
      expect(manifest).toContain(`name = "${binary}"`);
    }
  }
});
