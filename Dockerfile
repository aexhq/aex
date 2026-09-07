FROM rust:1.97.1-bookworm AS build
ARG TARGETARCH
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,id=aex-target-${TARGETARCH},target=/src/target \
    cargo build --locked --release -p aex-server && cp /src/target/release/aex-server /aex-server

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10002 aex && install -d -o aex -g aex /var/lib/aex
COPY --from=build /aex-server /usr/local/bin/aex-server
USER 10002:10002
EXPOSE 8081
ENTRYPOINT ["/usr/local/bin/aex-server"]
CMD ["serve", "--config", "/etc/aex/config.json"]
