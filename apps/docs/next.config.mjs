import { createMDX } from "fumadocs-mdx/next";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const docsDir = dirname(fileURLToPath(import.meta.url));

/** @type {import("next").NextConfig} */
const nextConfig = {
  basePath: "/docs",
  images: {
    unoptimized: true
  },
  output: "export",
  outputFileTracingRoot: join(docsDir, "../.."),
  reactStrictMode: true,
  trailingSlash: true,
  turbopack: {
    root: join(docsDir, "../..")
  },
  typedRoutes: false
};

const withMDX = createMDX();

export default withMDX(nextConfig);
