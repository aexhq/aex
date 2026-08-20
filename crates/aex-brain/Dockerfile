FROM rust:1.97.1-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --locked --release -p aex-brain

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/aex-brain /usr/local/bin/aex-brain
EXPOSE 8700
ENTRYPOINT ["/usr/local/bin/aex-brain"]
