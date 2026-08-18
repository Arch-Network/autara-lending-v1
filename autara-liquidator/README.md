# Autara Liquidator

The liquidator uses one executable for mainnet and testnet. RPC URLs, lending
programs, token mints, PropAMM endpoints/programs, and CLAMM programs/configs/
pools come from the JSON config; none are selected by compiled network IDs.

PropAMM is an external RFQ venue. The bot calls `GET /health`, `GET /markets`,
`POST /rfq/quote`, validates the unsigned transaction, adds only the liquidator
BIP322 signature, and submits it through `POST /rfq/swap`. The PropAMM quote
signer secret is never configured or loaded by this process.

CLAMM remains the atomic route: its exact-input SwapV2 instruction is embedded
as the lending liquidation callback. Ties prefer this atomic path.

## Configuration

- Mainnet example: `liquidator-config.example.json`
- Testnet example: `liquidator-config.testnet.example.json`

Both examples point to the same server key path:
`/home/ubuntu/autara/liquidator/keys/liquidator.key`. Select a network by passing
the corresponding config to the same binary. Do not copy deployment IDs into
Rust code.

The liquidator Arch pubkey derived from that server key is:

```text
be6fec0e8983f218a5af6ed2f7a95bba8e83ad26d5183fee3231f5e837182b46
```

At the 2026-08-13 testnet preflight it held `2,000,000` lamports, or
`0.002 ARCH`.

## Read-only venue preflight

The checker reads the configured CLAMM program/config/pool/vaults and requests
an unsigned PropAMM RFQ. It never loads a key or broadcasts a transaction.

```bash
cargo run -p autara-liquidator --bin check_venues -- \
  --config autara-liquidator/liquidator-config.testnet.example.json \
  --user be6fec0e8983f218a5af6ed2f7a95bba8e83ad26d5183fee3231f5e837182b46 \
  --input-mint 1d46e0dd87393236e4e01252439f46dcbaec7c2255d1fd734e61771a00e8f4e9 \
  --output-mint 55c6cee38a31732e2dad821ab1c38f902a7c51efaefb3641d51f3485c4617a45 \
  --amount 10000
```

Expected: `user_native_balance_lamports: 2000000`, `clamm_ready_pools: 1`, a
positive CLAMM quote, `propamm_ready: true`, and a positive RFQ quote.

## Testnet inventory funding

The bot needs debt-token inventory for a non-atomic PropAMM liquidation. CLAMM
does not require standing collateral inventory because collateral is received
earlier in the same atomic liquidation transaction.

Size test inventory from current restricted-market exposure with the read-only
checker:

```bash
cargo run -p autara-liquidator --bin check_exposure -- \
  --config autara-liquidator/liquidator-config.testnet.example.json
```

The testnet mint-authority key paths are operator-only inputs and must never be
placed in the liquidator config or copied to the server:

```text
/Users/ashutoshvarma/Projects/arch/autara-lending-v1/autara-deploy/.keys-testnet/ausd.mint.authority.key
/Users/ashutoshvarma/Projects/arch/autara-lending-v1/autara-deploy/.keys-testnet/abtc.mint.authority.key
```

The aUSD and aBTC authority pubkeys are respectively
`d0208cfc6086e663140b134b852d48564c22b57949ae4ceea9245006fd90b804`
and `2a533fae6d2aab9ca336f2bf5f07fc8048f5f4300337852a1d2a816d6ce25bda`.
Successful mint transactions prove they match the deployed mints' authorities.
The following commands also create the recipient ATA. Replace the amount
placeholders with an explicitly approved inventory size in atoms; do not infer
production inventory from these examples.

```bash
cargo run -p autara-client --bin autara-cli -- \
  --arch-node https://rpc.testnet.arch.network \
  --network testnet \
  --program-id 53def2dc8516302842b10e356914d2a5f6b33425ba42aec684f706aa1cf64192 \
  --signer /Users/ashutoshvarma/Projects/arch/autara-lending-v1/autara-deploy/.keys-testnet/ausd.mint.authority.key \
  token mint \
  --token 55c6cee38a31732e2dad821ab1c38f902a7c51efaefb3641d51f3485c4617a45 \
  --to be6fec0e8983f218a5af6ed2f7a95bba8e83ad26d5183fee3231f5e837182b46 \
  --amount <AUSD_ATOMS>

cargo run -p autara-client --bin autara-cli -- \
  --arch-node https://rpc.testnet.arch.network \
  --network testnet \
  --program-id 53def2dc8516302842b10e356914d2a5f6b33425ba42aec684f706aa1cf64192 \
  --signer /Users/ashutoshvarma/Projects/arch/autara-lending-v1/autara-deploy/.keys-testnet/abtc.mint.authority.key \
  token mint \
  --token 1d46e0dd87393236e4e01252439f46dcbaec7c2255d1fd734e61771a00e8f4e9 \
  --to be6fec0e8983f218a5af6ed2f7a95bba8e83ad26d5183fee3231f5e837182b46 \
  --amount <ABTC_ATOMS>
```

The authority accounts must hold enough native lamports to pay transaction
fees. Faucet-fund the `*.mint.authority.key` files when needed; never pass the
`*.mint.key` files as transaction signers because those pubkeys are the
token-program-owned mint accounts and cannot be fee payers. Recheck the
recipient balances with:

```bash
cargo run -p autara-client --bin autara-cli -- \
  --arch-node https://rpc.testnet.arch.network \
  --network testnet \
  --program-id 53def2dc8516302842b10e356914d2a5f6b33425ba42aec684f706aa1cf64192 \
  --signer /Users/ashutoshvarma/Projects/arch/autara-lending-v1/autara-deploy/.keys-testnet/ausd.mint.authority.key \
  token list-accounts \
  --owner be6fec0e8983f218a5af6ed2f7a95bba8e83ad26d5183fee3231f5e837182b46
```

## Running

Keep testnet in dry-run mode until the state reload and both venue readiness
checks pass:

```bash
cargo run -p autara-liquidator --bin autara-liquidator -- \
  --config autara-liquidator/liquidator-config.testnet.example.json
```

For a live testnet acceptance run, copy the example to a deployment-specific
config, change `dry_run` to `false`, and use the same executable. A PropAMM
failure after liquidation emits `INVENTORY ALERT` and deliberately does not
retry the already-liquidated position through CLAMM.

Rollback is config-only: stop the process and remove the failing venue's
`propamm` or `clamm` section. Startup succeeds when at least one configured
venue passes readiness.
