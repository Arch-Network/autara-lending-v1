# Key hygiene (offline remediation)

## Status

**Repo hygiene done.** Committed keypairs under `keys/` were treated as
permanently compromised. Tracking stopped; deploy env files point at gitignored
paths under `autara-deploy/.keys-testnet/`.

**On-chain redeploy (testnet):** completed 2026-08-14 with the fresh
`.keys-testnet/` keypairs. See `docs/testnet-deployment.md` for the new public
program/oracle/config/market addresses. The legacy compromised ids
(`53def2dc…` / `eee682c2…`) were **not** reused as program ids or as upgrade
authorities for the new deploy.

**Git history rewrite is still deferred** (separate coordinated step: filter-repo
/ BFG + force-push). Untracking does not remove secrets from past commits.

## Where keys live

| Location | Tracked? | Purpose |
|---|---|---|
| `keys/` | **No** (gitignored) | Legacy path; do not commit. Local copies may remain for emergency ops on the live compromised deployment until rotation. |
| `autara-deploy/.keys-testnet/` | **No** (gitignored) | Fresh testnet keypairs used for the 2026-08-14 redeploy. |
| `autara-deploy/.keys-mainnet/` / `.keys-*/` | **No** | Same pattern for other networks. |
| `tokens.json` | Yes | Public mint/authority **pubkeys** + key **paths** only — never secret bytes. |

## Restore stage keys (integration tests / shared_loss_flow)

`autara-client` resolves the stage program, oracle and admin from
`keys/autara-stage.key`, `keys/autara-pyth-stage.key` and
`keys/autara-admin-stage.key`. Those paths are gitignored now, so a fresh clone
cannot build an `AutaraFixture` — every integration test panics identically with
`NotFound` at `autara-client/src/config.rs`.

Untracking did not purge them, so local copies are recoverable from history:

```bash
# Trailing ^ matters: rev-list lands on the commit that DELETED the keys, so we
# want its parent (bb6f488, the last commit where they were still tracked).
REF=$(git rev-list -1 HEAD -- keys/autara-stage.key)^
mkdir -p keys
for k in autara-stage autara-pyth-stage autara-admin-stage autara-cli-signer; do
  git show "$REF:keys/$k.key" > "keys/$k.key" && chmod 600 "keys/$k.key"
done
```

Expected pubkeys (public, safe to share) — use these to confirm a restore:

| Key | Pubkey |
|---|---|
| `autara-stage` | `53def2dc8516302842b10e356914d2a5f6b33425ba42aec684f706aa1cf64192` |
| `autara-pyth-stage` | `eee682c27db375bebbc17ed9a76aaa935c8b72bc7de50d736f03e2dfbed84b15` |
| `autara-admin-stage` | `9fe2d81600314dc3db735bd6924b655b6a515a4de6f084cbbd23139e9da924ec` |
| `autara-cli-signer` | `b5eb801401791f83345cf81bf8d4c04daf34fa203e715467dc73a6995e2d21de` |

`autara-cli-signer` is the **oracle feed authority**. The oracle binds each feed
to whichever signer created it, and the live aUSD/aBTC feeds belong to this key,
so the Pyth pusher must run with `--signer keys/autara-cli-signer.key`. Without
it every push is rejected with `Incorrect authority provided` while the pusher
keeps logging `Sending`, and e2e later fails on a stale price for no visible
reason.

```bash
cargo run -q -p autara-client --example print_pubkey -- --key keys/autara-stage.key
```

CI gets the same four via repo secrets `AUTARA_STAGE_PROGRAM_KEY_B64`,
`AUTARA_STAGE_ORACLE_KEY_B64`, `AUTARA_STAGE_ADMIN_KEY_B64` and
`AUTARA_ORACLE_SIGNER_KEY_B64`, which `autara-readiness` decodes back into
`keys/`. These are **distinct** from the
`.keys-testnet` deploy roles (`PROGRAM_KEYPAIR_B64` & co) on the `testnet`
Environment. Once the deferred rotation/redeploy lands this section goes away:
the stage keys are compromised and only still in use because the existing stage
deployment is bound to them.

## Pre-funded e2e wallet (testnet)

`e2e_flow` used to mint itself aUSD/aBTC, but nobody holds the mint authority for
the live mints — `MintTo` fails with `owner does not match`. It now runs against
a dedicated pre-funded wallet with `E2E_SKIP_MINT=1`, the same path mainnet-safe
already used because mainnet has no mint authority either.

| | |
|---|---|
| Local path | `keys/e2e-testnet-user.key` (gitignored, **not** in git history) |
| Pubkey | `ebc08453e3370b2c121cb42ff644ceb71271390371af71cdfc6931d7a2fefa32` |
| CI secret | `AUTARA_E2E_TESTNET_USER_KEY_B64` |

The flow unwinds every position it opens, so the balance only drifts by the
interest it pays — about 1 atom per run. Gas is topped up from the faucet on each
run, since `E2E_SKIP_MINT=1` also skips faucet funding.

To rebuild or refill it, transfer from a wallet holding the mints (the CLAMM
testnet authority `f5a147302d658b69b1b312c3956bf68c7c8dc9e578615c2ad8343a8ac17cf69f`
holds both):

```bash
openssl rand -hex 32 | tr -d '\n' > keys/e2e-testnet-user.key && chmod 600 keys/e2e-testnet-user.key
cargo run -q -p autara-client --example fund_signer -- \
  --key keys/e2e-testnet-user.key --rpc https://rpc.testnet.arch.network --network testnet

# aUSD (6dp) then aBTC (8dp); the flow needs ~200 aUSD and ~0.02 aBTC per run.
XFER_RPC=https://rpc.testnet.arch.network XFER_NETWORK=testnet4 \
  XFER_FROM_KEY=<holder key> XFER_TO=<user pubkey> \
  XFER_MINT=55c6cee38a31732e2dad821ab1c38f902a7c51efaefb3641d51f3485c4617a45 \
  XFER_AMOUNT=2000000000 cargo run -q -p autara-client --example xfer

base64 -i keys/e2e-testnet-user.key | tr -d '\n' | gh secret set AUTARA_E2E_TESTNET_USER_KEY_B64
```

## Generate / regenerate testnet keys

Four deploy roles (program / oracle / deployer / admin):

```bash
./autara-deploy/scripts/set-github-secrets.sh \
  --env testnet \
  --generate \
  --force \
  --out-dir autara-deploy/.keys-testnet
# Omit --apply unless you intentionally want to update GitHub Environment secrets.
```

Any individual compatible key file (creates the file if missing via `arch_sdk`):

```bash
cargo run -p autara-client --example print_pubkey -- \
  --key autara-deploy/.keys-testnet/<name>
```

Never commit private key bytes. Pubkeys of the new keys are fine to share with
operators when planning the later on-chain cutover.

## Follow-ups (not this PR)

1. Coordinated **history purge** of leaked `keys/*.key` blobs from git history.
2. **On-chain** rotate upgrade authority / admin / mint authority, or prefer a
   full fresh redeploy with `autara.testnet.env` + `sync-program-id.sh`, then
   cut clients over.
