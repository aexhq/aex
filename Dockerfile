FROM rust:1.97.1-bookworm AS build
ARG TARGETARCH
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,id=aex-target-${TARGETARCH},target=/src/target \
    cargo build --locked --release -p aex-server && cp /src/target/release/aex-server /aex-server

FROM golang:1.26-bookworm@sha256:d9c68c2c51161e12fd77e4c6320687c9cd86e1af1e3ad6e6cd63ff970641453c AS runtime
WORKDIR /src
COPY runtime/go.mod runtime/go.sum ./
RUN go mod download
COPY runtime/*.go ./
RUN CGO_ENABLED=0 go build -trimpath -o /aex-runtime . \
    && cp -a "$(go list -m -f '{{.Dir}}' github.com/aexhq/brain)" /brain

FROM node:24.17.0-bookworm-slim AS dependencies
WORKDIR /opt/aex
COPY package.json package-lock.json ./
COPY packages/sdk/package.json packages/sdk/package.json
COPY packages/cli/package.json packages/cli/package.json
RUN npm ci --omit=dev --ignore-scripts
COPY --from=runtime /brain/runners ./runners
RUN npm --prefix runners/pi ci --ignore-scripts && npm --prefix runners/pi run build && npm --prefix runners/pi prune --omit=dev --ignore-scripts \
    && npm --prefix runners/codex ci --ignore-scripts && npm --prefix runners/opencode ci --ignore-scripts
COPY --from=runtime /brain/services/http.mjs ./brain-services/http.mjs
COPY --from=runtime /brain/services/browser ./brain-services/browser
ENV PLAYWRIGHT_BROWSERS_PATH=/ms-playwright
RUN npm --prefix brain-services/browser ci --ignore-scripts && node brain-services/browser/node_modules/playwright/cli.js install chromium

FROM node:24.17.0-bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates perl-base poppler-utils procps util-linux && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10002 aex && install -d -o 10002 -g 10002 /var/lib/aex \
    && useradd --system --uid 10003 aex-sandbox && useradd --system --uid 10004 aex-application && useradd --system --uid 10006 aex-browser \
    && install -d -o 10003 -g 10003 -m 700 /var/lib/aex-tools-sandbox \
    && install -d -o 10004 -g 10004 -m 700 /var/lib/aex-tools-application
COPY --from=build /aex-server /usr/local/bin/aex-server
COPY --from=runtime /aex-runtime /usr/local/bin/aex-runtime
COPY --from=dependencies /opt/aex /opt/aex
COPY --from=dependencies /ms-playwright /ms-playwright
COPY services /opt/aex/services
ENV PLAYWRIGHT_BROWSERS_PATH=/ms-playwright
RUN node /opt/aex/brain-services/browser/node_modules/playwright/cli.js install-deps chromium \
    && node --test /opt/aex/services/media/server.test.mjs /opt/aex/services/media/render.test.mjs /opt/aex/services/tools/services.test.mjs
USER 10006:10006
RUN npm --prefix /opt/aex/brain-services/browser test
USER 10002:10002
EXPOSE 8081
ENTRYPOINT ["/usr/local/bin/aex-server"]
CMD ["serve", "--config", "/etc/aex/config.json"]
