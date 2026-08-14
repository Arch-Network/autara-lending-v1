# Autara deploy runbook

`autara-deploy` is an env-driven tool that deploys the Autara lending stack to
Arch Network: the `autara-program` and `autara-oracle` ELFs, the global config,
and the lending markets. It mirrors the CLAMM `clamm-deploy` crate.

`localnet`, `testnet`, and `mainnet` are all wired up. Mainnet defaults to the
public Arch mainnet RPC (`https://rpc.mainnet.arch.network`) and the Bitcoin
mainnet signing network. A **real** mainnet run is additionally gated in CI by
the typed confirmation `DEPLOY MAINNET` (see `_autara-action.yml`) and the
`mainnet` GitHub Environment; this tool never generates mainnet keypairs.

## TL;DR

```bash
# Always dry-run first (prints derived addresses + preflight, sends NOTHING).
NETWORK=testnet ./autara-deploy/scripts/deploy.sh --dry-run

# Real deploy (sends transactions). Builds the SBF ELFs, then deploys.
NETWORK=testnet ./autara-deploy/scripts/deploy.sh

# Read-only balance check.
NETWORK=testnet cargo run -p autara-deploy -- check-balance
```

You can also drive the binary directly (it reads the same env):

```bash
set -a; source autara-deploy/scripts/autara.testnet.env; set +a
DRY_RUN=1 cargo run -p autara-deploy
```

## What it does

1. **Preflight** — prints program/oracle ids, deployer/admin, RPC url, the
   derived global-config PDA, token mints, ELF presence, RPC reachability, and
   on-chain balances. Includes a **program-id guard** (see below).
2. **Deploy programs** — uploads `target/deploy/autara_program.so` and
   `autara_oracle.so` via the deploy crate's ELF uploader (idempotent; chunks
   sized for Arch's 1232-byte transaction limit).
3. **create_global_config** — admin + fee receiver + fee share (idempotent).
4. **Token setup** — ensures every configured `TOKENS` mint exists on-chain
   (idempotent; fails loudly if a mint is missing — create mints out-of-band via
   `autara-cli token setup`, this tool holds no mint authority).
5. **create_market** — creates one lending market per `MARKET_PAIRS` entry
   (curator = admin; idempotent). The economic parameters mirror
   `autara-server`'s defaults but are env-configurable — see
   [Market economic parameters](#market-economic-parameters).
6. **Artifact** — writes `deployments/<network>.json` (addresses + tx ids only),
   including the created/ensured markets.

Each step is gated by `STEP_DEPLOY_PROGRAM`, `STEP_DEPLOY_ORACLE`,
`STEP_INIT_CONFIG`, `STEP_TOKEN_SETUP`, `STEP_CREATE_MARKET` (all default
`true`). In CI these are set explicitly per action: `deploy` (programs),
`initialize` (global config), `upgrade` (program re-upload), and `setup-markets`
(token setup + markets — see `autara-setup-markets.yml`).

## Program-id guard (important)

The on-chain `autara-program` derives the global-config PDA and runs ownership
checks against a **compiled-in id** (`autara_program::id()` =
`53def2dc...1cf64192`), *not* the runtime program id. The client derives PDAs
from the deployed program key. So the deployed program key's pubkey **must
equal** `autara_program::id()`.

- The live/stage program key's pubkey equals `autara_program::id()`
  (`53def2dc…`). Fresh keys under `autara-deploy/.keys-testnet/` will **not**
  match until you run `sync-program-id.sh` + rebuild (part of the deferred
  on-chain redeploy — see `docs/key-hygiene.md`).
- If you deploy with a different key, the tool warns (fatal on a real run). To
  use a new key you must update `id()` in `programs/autara-program/src/lib.rs`
  and rebuild the ELF.

The `autara-oracle` program is position-independent (it uses the runtime
`program_id` only), so it needs no such guard.

## Secret hygiene

- Env files contain **paths only**, never key material.
- `keys/` and `*.key` are gitignored; never commit keypairs.
- Keep rotated/fresh testnet keys in `autara-deploy/.keys-testnet/` (also
  gitignored via `.keys-*/`).
- `autara.testnet.env` points at `.keys-testnet/`. On-chain use of those keys is
  deferred until an operator green-lights rotation/redeploy.

## Per-network differences

Only these should change between networks: `ARCH_RPC_URL`, the `*_KEY_PATH`
files, `TOKENS`, and `MARKET_PAIRS` (plus the optional market params below).
Everything else lives in the env file. See `autara.mainnet.env` for the mainnet
template (paths only, `.keys-mainnet/`).

## Market economic parameters

`create_market` applies these per-market risk parameters. Each has an env knob;
**unset** reproduces the historical testnet behavior byte-for-byte (the defaults
equal the values that were previously hardcoded and mirror `autara-server`'s
`default_market_config`):

| Env var | Default | Meaning |
| --- | --- | --- |
| `MARKET_MAX_LTV` | `0.8` | Max borrow LTV a position may open at. |
| `MARKET_UNHEALTHY_LTV` | `0.9` | LTV at/above which a position is liquidatable. |
| `MARKET_LIQUIDATION_BONUS` | `0.05` | Liquidator bonus (on-chain bounds: `0.001`..=`0.1`). |
| `MARKET_MAX_UTILISATION` | `0.9` | Max supply-vault utilisation after a borrow. |

The interest-rate curve is always **adaptive** (not parameterized). The on-chain
program validates these (e.g. `max_ltv < unhealthy_ltv`, liquidation-bonus
bounds, `unhealthy_ltv * (1 + bonus) <= 0.99`); an invalid combination fails at
`create_market`. The dry-run preflight prints the resolved `market_params:` line
so you can confirm them before a real run. **For mainnet, the team must confirm
the intended risk parameters** and set them explicitly in `autara.mainnet.env`
rather than relying on the testnet-derived defaults.

## Mainnet deploy (ordered runbook)

Mainnet is real value. In addition to the per-network differences above, a
**real** mainnet run is gated by the `mainnet` GitHub Environment (which can
require reviewers) and the typed `DEPLOY MAINNET` confirmation, and the deploy
tool itself runs two **mainnet preflight guards** (fatal on a real run, warnings
in dry-run):

1. **No faucet** — `USE_FAUCET=true` is refused (mainnet has no faucet).
2. **No placeholder mints** — the run is refused while any configured `TOKENS`
   mint still equals the testnet placeholder mints shipped in
   `autara.mainnet.env`. Replace them with the real mainnet APL mints first.

Follow these steps in order. Steps 1–6 are operational prerequisites that
**cannot** be done from code; do them out-of-band first.

1. **Provision the keypairs out-of-band.** Create the four keys in the
   gitignored `autara-deploy/.keys-mainnet/` directory
   (`program.json`, `oracle.json`, `deployer.json`, `admin.json`) — NEVER
   generate or commit them here. The `program.json` pubkey becomes the on-chain
   program id and **must** equal `autara_program::id()` (kept in sync in step 3).
   Decide and document the **upgrade-authority custody plan** for the deployer
   key (it is the program upgrade authority).
2. **Set the real mints + confirmed params.** In `autara.mainnet.env`, replace
   every placeholder mint in `TOKENS=` with the genuine mainnet APL mint, and set
   the confirmed `MARKET_*` risk parameters (or leave them at the documented
   defaults after team sign-off). Keep `USE_FAUCET=false`. This file stays
   **paths-only** — no secrets.
3. **Sync the program id + rebuild the SBF ELFs.** Run
   `autara-deploy/scripts/sync-program-id.sh <PROGRAM_PUBKEY_HEX>` so
   `autara_program::id()` matches the mainnet program key, then rebuild the ELFs
   (`cargo-build-sbf --features entrypoint` for `autara-program` and
   `autara-oracle`). In CI this ordering (derive → sync → rebuild → build-sbf) is
   enforced automatically by `_autara-action.yml`.
4. **Fund the deployer + admin.** Since there is no faucet, transfer enough
   native balance to the deployer (program ELF uploads are large) and the admin
   (signs `create_global_config` + market creation) **out-of-band**.
5. **Create the `mainnet` GitHub Environment.** Repo Settings → Environments →
   `mainnet`; add **required reviewers** so a real run needs human approval.
6. **Set the Environment secrets.** From the provisioned keys:

   ```bash
   # From an already-provisioned key dir:
   ./autara-deploy/scripts/set-github-secrets.sh \
     --env mainnet --from-dir autara-deploy/.keys-mainnet --apply

   # …or generate fresh keys into the gitignored dir and set them:
   ./autara-deploy/scripts/set-github-secrets.sh \
     --env mainnet --generate --rpc-url https://rpc.mainnet.arch.network --apply
   ```

   (Drop `--apply` first to preview — it prints only public keys, never secret
   bytes.)
7. **Dry-run every workflow first.** From the Actions tab, dispatch each workflow
   (`autara-deploy`, `autara-initialize`, `autara-setup-markets`,
   `autara-upgrade`) with `network=mainnet` and `dry_run=true`. Verify the
   printed addresses, the resolved `market_params:`, the program-id guard
   (`program_id_guard: ok`), and the `mainnet_guard: ok` line. A dry-run sends
   **nothing** and skips the gates' teeth.
8. **Pre-upgrade readiness (recommended).** Before any upgrade, run
   `autara-readiness` (or locally `./scripts/readiness.sh testnet|mainnet-safe`).
   Prefer the chained **`autara-release`** workflow: readiness → program+oracle
   upgrade dry-run → optional real upgrade. For mainnet, set
   `readiness_mode=mainnet-safe` (pre-funded user; `E2E_SKIP_MINT=1`) and only
   set `run_real_upgrade=true` after reviewing artifacts; real mainnet still
   requires `mainnet_confirm=DEPLOY MAINNET`.
9. **Real run, in order.** Only after the dry-runs look correct, re-dispatch with
   `dry_run=false` **and** `mainnet_confirm=DEPLOY MAINNET`, approving the
   Environment review when prompted. Order: `autara-deploy` (program + oracle) →
   `autara-initialize` (global config) → `autara-setup-markets` (token setup +
   markets). Use `autara-upgrade` / `autara-release` for in-place **program +
   oracle** upgrades (ids unchanged).

> **Upgrade-authority custody.** The deployer key is the program upgrade
> authority (it signs the on-chain re-upload for `autara-upgrade`). Guard it like
> a production signing key: store it only as the `mainnet` Environment secret and
> keep the mainnet upgrade behind required reviewers + the `DEPLOY MAINNET`
> typed confirm.
