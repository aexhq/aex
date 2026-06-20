#!/usr/bin/env bun

const [packageName, version] = process.argv.slice(2);

if (!packageName || !version) {
  console.error("Usage: assert-npm-version-available.mjs <package> <version>");
  process.exit(2);
}

const registry = (process.env.NPM_REGISTRY_URL ?? "https://registry.npmjs.org").replace(/\/$/, "");
const url = `${registry}/${encodeURIComponent(packageName)}/${encodeURIComponent(version)}`;

const response = await fetch(url, {
  headers: {
    accept: "application/json",
    "cache-control": "no-cache"
  }
});

if (response.status === 404) {
  console.log(`${packageName}@${version} is available on npm.`);
  process.exit(0);
}

if (response.ok) {
  console.error(`${packageName}@${version} already exists on npm.`);
  process.exit(1);
}

console.error(`Could not verify npm availability for ${packageName}@${version}: HTTP ${response.status}`);
process.exit(1);
