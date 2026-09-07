FROM rust:1.97.1-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --locked --release -p aex-server

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10002 aex && install -d -o aex -g aex /var/lib/aex
COPY --from=build /src/target/release/aex-server /usr/local/bin/aex-server
USER 10002:10002
EXPOSE 8081
ENTRYPOINT ["/usr/local/bin/aex-server"]
CMD ["serve", "--config", "/etc/aex/config.json"]
