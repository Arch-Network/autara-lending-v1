# Autara Lending V1

Autara Lending V1 is a decentralized lending protocol built on the Arch blockchain. It allows users to lend and borrow permissionlessly assets.
The protocol is designed to be secure, efficient, and user-friendly, providing a seamless experience for both lenders and borrowers.

### Setup

- Install [Rust](https://www.rust-lang.org/tools/install)
- Install [Solana](https://docs.anza.xyz/cli/install) (stable version, not latest)
- Install [Docker](https://docs.docker.com/engine/install/)
- Install [nextest](https://nexte.st/book/installation.html)
- Install [llvm-cov](https://nexte.st/docs/integrations/test-coverage/?h=cove)


### Repository

This repository contains 7 crates:

- `autara-lib`: Main library of the Autara protocol, defining the data structures and logic
- `autara-client`: A client to interact with the Autara protocol
- `autara-program-lib`: Library for Arch smart contracts
- `autara-program`: Autara smart contract
- `autara-integration-tests`: Standalone crate for the Autara smart contracts integration tests
- `autara-pyth`: A naive client to push Pyth price feeds to autara-oracle smart contracts
- `autara-oracle`: A naive oracle smart contract to store Pyth price feeds without any validation

### Build

To build smart contracts, run the following command:

```bash
cargo-build-sbf --features entrypoint
```

To build workspace, run:

```bash
cargo build
```

### Keys

**Do not commit keypairs.** `keys/` and `*.key` are gitignored. Fresh testnet
deploy keys live under the gitignored `autara-deploy/.keys-testnet/` directory
(see [`docs/key-hygiene.md`](docs/key-hygiene.md)).

| Role (file under `.keys-testnet/`) | Purpose |
|---|---|
| `program.json` | Autara program account keypair (pubkey becomes program id) |
| `oracle.json` | Oracle program account keypair |
| `admin.json` | Default admin key for development / global-config |
| `deployer.json` | Program upgrade authority + deploy payer |
| `cli-signer.key` | Default CLI signer (set in `.env` as `AUTARA_SIGNER_KEY`) |
| `token-authority.key` | Mint authority for tokens created via `token setup` |
| `token-btc.key` / `token-usdc.key` / `token-eth.key` | Mint account keypairs (for a future redeploy) |

On-chain rotation with these keys is **deferred** — generate locally first; do
not upgrade/redeploy until explicitly green-lit.

### Local environment

Setup an arch local environment:

```bash
docker pull --platform linux/amd64 baptisteeb/arigato-node:latest
docker run -d --platform linux/amd64 --name arigato-node -p 18443:18443 -p 3030:3030 -p 9002:9002 baptisteeb/arigato-node:latest
```

Then, deploy the smart contracts:

```bash
cargo run --bin deploy
```

### CLI

The CLI (`autara-cli`) is used to interact with the protocol from the command line.

```bash
cargo run --bin autara-cli -- --help
```

Global options:

- `--arch-node <URL>` — Arch node URL (default: `https://rpc.testnet.arch.network`)
- `--signer <PATH>` — Path to signer key file (falls back to `AUTARA_SIGNER_KEY` env var)
- `--network <NETWORK>` — Bitcoin network: `regtest`, `testnet`, or `mainnet` (default: `regtest`)
- `--tokens <PATH>` — Path to `tokens.json` for resolving token names in output (default: `tokens.json`)

#### Token setup

Create BTC, USDC, and ETH token mints on-chain (idempotent) and write a `tokens.json` config file:

```bash
cargo run --bin autara-cli -- token setup --output tokens.json
```

The generated `tokens.json` contains mint addresses, decimals, key file paths, and the token authority info. This file is used by both the CLI (for token name display) and the server.

#### Other CLI commands

```bash
# Read commands
autara-cli read markets                    # List all markets
autara-cli read market --market <PUBKEY>   # Get market details
autara-cli read positions                  # Get user positions
autara-cli read global-config              # Get global config

# Transaction commands
autara-cli tx create-market --config market.json --supply-mint <PUBKEY> --collateral-mint <PUBKEY>
autara-cli tx supply --market <PUBKEY> --amount <ATOMS>
autara-cli tx borrow --market <PUBKEY> --amount <ATOMS>
autara-cli tx repay --market <PUBKEY> --amount <ATOMS>
autara-cli tx deposit-collateral --market <PUBKEY> --amount <ATOMS>
autara-cli tx withdraw-collateral --market <PUBKEY> --amount <ATOMS>

# Token commands
autara-cli token create-token --decimals 8
autara-cli token mint --token <PUBKEY> --amount <ATOMS>
autara-cli token list-accounts

# Oracle commands
autara-cli oracle fetch-price --feed 0x<FEED_ID>
autara-cli oracle push-price --feed 0x<FEED_ID> --price 100000.0
autara-cli oracle push-feeds --feed 0x<BTC_FEED> --feed 0x<USDC_FEED>
autara-cli oracle market-feeds --market <PUBKEY>
```

### Autara Server

The server provides a JSON-RPC API and manages markets, token minting, and oracle price feeds.

```bash
cargo run --bin autara-server -- --tokens tokens.json
```

Options:

- `--tokens <PATH>` — **(required)** Path to `tokens.json` config
- `--program-id <HEX>` — Autara program ID (defaults to the configured program key pubkey)
- `--oracle-program-id <HEX>` — Oracle program ID (defaults to the configured oracle key pubkey)
- `--signer <PATH>` — Signer key file (falls back to `AUTARA_SIGNER_KEY` env var)
- `--arch-node <URL>` — Arch node URL (default: `https://rpc.testnet.arch.network`)
- `--network <NETWORK>` — Bitcoin network (default: `testnet`)
- `--listen <ADDR>` — RPC listen address (default: `0.0.0.0:62776`)
- `--prometheus <ADDR>` — Prometheus exporter address (default: `0.0.0.0:62777`)
- `--market-config <PATH>` — Custom market config JSON (optional, uses defaults otherwise)

On startup the server will:
1. Fund the signer account via faucet
2. Create the global config if it doesn't exist
3. Create markets for all token pair combinations (idempotent)
4. Spawn a Pyth feed pusher in the background
5. Start the JSON-RPC server and Prometheus exporter

### Quickstart (testnet)

```bash
# 1. Deploy programs
cargo run --bin deploy

# 2. Create token mints
cargo run --bin autara-cli -- token setup --output tokens.json

# 3. Start the server
cargo run --bin autara-server -- --tokens tokens.json

# 4. Interact via CLI
cargo run --bin autara-cli -- read markets
```

### Test

```bash
cargo nextest run --no-fail-fast -j 24
```

If you dont have a local arch node running, you can run the tests with the following command:

```bash
cargo nextest run --exclude autara-integration-tests --workspace
```

Run tests with coverage:

```bash
cargo llvm-cov nextest --exclude autara-integration-tests --workspace
```
