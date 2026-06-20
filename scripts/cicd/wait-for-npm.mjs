#!/usr/bin/env bun

const [packageName, version] = process.argv.slice(2);

if (!packageName || !version) {
  console.error("Usage: wait-for-npm.mjs <package> <version>");
  process.exit(2);
}

const registry = (process.env.NPM_REGISTRY_URL ?? "https://registry.npmjs.org").replace(/\/$/, "");
const timeoutMs = Number(process.env.NPM_WAIT_TIMEOUT_MS ?? 10 * 60_000);
const intervalMs = Number(process.env.NPM_WAIT_INTERVAL_MS ?? 10_000);
const metadataUrl = `${registry}/${encodeURIComponent(packageName)}/${encodeURIComponent(version)}`;
const deadline = Date.now() + timeoutMs;
let lastStatus = "not checked";

while (Date.now() < deadline) {
  try {
    const metadata = await fetch(metadataUrl, {
      headers: {
        accept: "application/json",
        "cache-control": "no-cache"
      }
    });

    lastStatus = `metadata HTTP ${metadata.status}`;

    if (metadata.ok) {
      const body = await metadata.json();
      const tarballUrl = body?.dist?.tarball;

      if (typeof tarballUrl === "string" && tarballUrl.length > 0) {
        const tarballStatus = await checkTarball(tarballUrl);
        lastStatus = `metadata visible; tarball ${tarballStatus}`;

        if (tarballStatus === "visible") {
          console.log(`${packageName}@${version} is visible on npm.`);
          process.exit(0);
        }
      } else {
        lastStatus = "metadata visible without dist.tarball";
      }
    }
  } catch (error) {
    lastStatus = error instanceof Error ? error.message : String(error);
  }

  await sleep(intervalMs);
}

console.error(`Timed out waiting for ${packageName}@${version} on npm. Last status: ${lastStatus}`);
process.exit(1);

async function checkTarball(url) {
  const head = await fetch(url, {
    method: "HEAD",
    headers: { "cache-control": "no-cache" }
  });

  if (head.ok) return "visible";
  if (head.status !== 405 && head.status !== 403) return `HTTP ${head.status}`;

  const get = await fetch(url, {
    headers: {
      "cache-control": "no-cache",
      range: "bytes=0-0"
    }
  });

  return get.ok || get.status === 206 ? "visible" : `HTTP ${get.status}`;
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
