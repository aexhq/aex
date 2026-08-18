# The control-plane image: identity, prepaid billing, session authority, rated usage — one
# process in front of one brain. Payments default to the loud fake; AEX_PAYMENTS=stripe +
# STRIPE_SECRET_KEY is real billing. The SQLite ledger lives on /data: MOUNT IT DURABLY —
# accounts and money do not belong on ephemeral container storage.
FROM rust:1.97-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release -p aex-control --bin aex-control

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/aex-control /usr/local/bin/aex-control
ENV AEX_CONTROL_DB=/data/control.db
VOLUME /data
EXPOSE 8600
ENTRYPOINT ["/usr/local/bin/aex-control"]
