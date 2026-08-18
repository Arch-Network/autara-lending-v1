FROM rust:1.92-bookworm AS builder

RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY . .
# One image serves both roles (see entrypoint.sh / ROLE): the API server and the
# dedicated oracle price pusher. The pusher is what runs per network on Arch
# mainnet + testnet.
RUN cargo build --release --bin autara-server --bin autara-pyth

FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y ca-certificates libssl3 && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/autara-server /usr/local/bin/autara-server
COPY --from=builder /app/target/release/autara-pyth /usr/local/bin/autara-pyth
COPY tokens.json /app/tokens.json
COPY keys/ /app/keys/
COPY entrypoint.sh /app/entrypoint.sh
RUN chmod +x /app/entrypoint.sh

WORKDIR /app
EXPOSE 62776

# ROLE selects the process (default "server" reproduces the previous testnet
# CMD). ROLE=pusher runs the dedicated oracle pusher. Everything is env-driven so
# the same image deploys to Arch mainnet and testnet, differing only by env.
ENTRYPOINT ["/app/entrypoint.sh"]
