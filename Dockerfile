FROM rust:1.97.1-bookworm AS build
ARG TARGETARCH
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,id=aex-target-${TARGETARCH},target=/src/target \
    cargo build --locked --release -p aex-server && cp /src/target/release/aex-server /aex-server

FROM node:24.17.0-bookworm-slim AS environment
WORKDIR /opt/aex
COPY package.json package-lock.json ./
COPY packages/sdk/package.json packages/sdk/package.json
COPY packages/cli/package.json packages/cli/package.json
RUN npm ci --omit=dev --ignore-scripts

FROM node:24.17.0-bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10002 aex && install -d -o 10002 -g 10002 /var/lib/aex \
    && install -d -o 10003 -g 10003 -m 700 /var/lib/aex-environment
COPY --from=build /aex-server /usr/local/bin/aex-server
COPY --from=environment /opt/aex/node_modules /opt/aex/node_modules
COPY environments /opt/aex/environments
USER 10002:10002
EXPOSE 8081
ENTRYPOINT ["/usr/local/bin/aex-server"]
CMD ["serve", "--config", "/etc/aex/config.json"]
