# Key hygiene (offline remediation)

## Status

**Repo hygiene only.** Committed keypairs under `keys/` were treated as
permanently compromised. This change stops tracking them going forward and
points deploy env files at gitignored paths.

**On-chain rotation / redeploy is DEFERRED** until an operator explicitly
green-lights it. Do **not** transfer upgrade authority, redeploy programs,
create markets, or faucet-fund accounts as part of this hygiene step.

**Git history rewrite is also deferred** (separate coordinated step: filter-repo
/ BFG + force-push). Untracking does not remove secrets from past commits.

## Where keys live

| Location | Tracked? | Purpose |
|---|---|---|
| `keys/` | **No** (gitignored) | Legacy path; do not commit. Local copies may remain for emergency ops on the live compromised deployment until rotation. |
| `autara-deploy/.keys-testnet/` | **No** (gitignored) | Fresh testnet keypairs for a future redeploy. |
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
for k in autara-stage autara-pyth-stage autara-admin-stage; do
  git show "$REF:keys/$k.key" > "keys/$k.key" && chmod 600 "keys/$k.key"
done
```

Expected pubkeys (public, safe to share) — use these to confirm a restore:

| Key | Pubkey |
|---|---|
| `autara-stage` | `53def2dc8516302842b10e356914d2a5f6b33425ba42aec684f706aa1cf64192` |
| `autara-pyth-stage` | `eee682c27db375bebbc17ed9a76aaa935c8b72bc7de50d736f03e2dfbed84b15` |
| `autara-admin-stage` | `9fe2d81600314dc3db735bd6924b655b6a515a4de6f084cbbd23139e9da924ec` |

```bash
cargo run -q -p autara-client --example print_pubkey -- --key keys/autara-stage.key
```

CI gets the same three via repo secrets `AUTARA_STAGE_PROGRAM_KEY_B64`,
`AUTARA_STAGE_ORACLE_KEY_B64` and `AUTARA_STAGE_ADMIN_KEY_B64`, which
`autara-readiness` decodes back into `keys/`. These are **distinct** from the
`.keys-testnet` deploy roles (`PROGRAM_KEYPAIR_B64` & co) on the `testnet`
Environment. Once the deferred rotation/redeploy lands this section goes away:
the stage keys are compromised and only still in use because the existing stage
deployment is bound to them.

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
