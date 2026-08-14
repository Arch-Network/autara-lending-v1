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
