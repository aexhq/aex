import { build } from "esbuild";
await build({ entryPoints: ["tests/journeys/browser.mjs"], outfile: "artifacts/browser-journey.js", bundle: true,
  platform: "browser", format: "esm", target: "es2022" });
