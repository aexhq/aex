/** @type {import("next").NextConfig} */
const config = {
  reactStrictMode: true,
  poweredByHeader: false,
  turbopack: {
    resolveAlias: {
      "@aexhq/sdk": "../../packages/sdk/dist/index.js",
      "@aexhq/wire": "../../packages/wire/dist/index.js",
    },
  },
  async headers() {
    return [{ source: "/:path*", headers: [{ key: "Content-Security-Policy", value: "default-src 'self'; connect-src 'self'; frame-ancestors 'none'" }] }];
  },
};
export default config;
