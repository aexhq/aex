FROM rust:1.97.1-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --locked --release -p aex-brain
RUN set -eux; \
    guest_script="$(find /usr/local/cargo/git/checkouts -type f -path '*/crates/brain-loophost/guest/componentize-one.mjs')"; \
    test -n "$guest_script"; \
    test "$(printf '%s\n' "$guest_script" | wc -l)" -eq 1; \
    guest_dir="$(dirname "$guest_script")"; \
    mkdir -p /opt/brain-loophost; \
    cp -a "$guest_dir" /opt/brain-loophost/guest; \
    cp -a "$guest_dir/../wit" /opt/brain-loophost/wit

FROM node:22-bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /opt/brain-loophost /opt/brain-loophost
RUN npm ci --ignore-scripts --prefix /opt/brain-loophost/guest \
    && cd /opt/brain-loophost/guest \
    && node --input-type=module -e "import('@bytecodealliance/componentize-js')" \
    && rm -rf \
    /usr/local/lib/node_modules/npm \
    /usr/local/lib/node_modules/corepack \
    /usr/local/bin/npm \
    /usr/local/bin/npx \
    /usr/local/bin/corepack \
    && node --version >/dev/null
COPY --from=build /src/target/release/aex-brain /usr/local/bin/aex-brain
ENV BRAIN_DATA_DIR=/data
ENV BRAIN_LOOPHOST_TOOLCHAIN_DIR=/opt/brain-loophost/guest
VOLUME /data
EXPOSE 8700
ENTRYPOINT ["/usr/local/bin/aex-brain"]
