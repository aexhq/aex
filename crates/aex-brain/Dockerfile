FROM rust:1.97.1-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --locked --release -p aex-brain

FROM node:22-bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    /usr/local/lib/node_modules/npm \
    /usr/local/lib/node_modules/corepack \
    /usr/local/bin/npm \
    /usr/local/bin/npx \
    /usr/local/bin/corepack \
    && node --version >/dev/null
COPY --from=build /src/target/release/aex-brain /usr/local/bin/aex-brain
ENV BRAIN_DATA_DIR=/data
VOLUME /data
EXPOSE 8700
ENTRYPOINT ["/usr/local/bin/aex-brain"]
