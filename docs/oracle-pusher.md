# Oracle price pusher (Arch mainnet + testnet)

The lending program rejects any supply/borrow/liquidate whose oracle is older
than `max_age` (60s) with `OracleRateTooOld` (`0x1b70`). The oracle account's
`publish_time` is stamped with the **on-chain clock at push time**
(`programs/autara-oracle/src/lib.rs`) and compared against the on-chain clock at
read time — same clock, so a feed is fresh only while something keeps writing to
its PDA. Each network therefore needs its own pusher writing to that network's
oracle program.

## Design

One container image, one `entrypoint.sh`, selected by `ROLE`:

- `ROLE=server` (default) — the API/indexer. Unchanged behavior. Set
  `DISABLE_PRICE_PUSHER=1` to hand pushing to a dedicated pusher.
- `ROLE=pusher` — the dedicated `autara-pyth` price pusher, one per network.

Mainnet and testnet run the **same image**; only env differs. Example env in
`autara-deploy/scripts/autara.pusher.{testnet,mainnet}.env`.

## Deploy (two pusher services)

Point a service at this image and set env from the matching file:

| Env | testnet | mainnet |
|-----|---------|---------|
| `ROLE` | `pusher` | `pusher` |
| `NETWORK` | `testnet` | `mainnet` |
| `ARCH_RPC_URL` | `https://rpc.testnet.arch.network` | `https://rpc.mainnet.arch.network` |
| `ORACLE_PROGRAM_ID` | `180ec4dd…` (`deployments/testnet.json` → `oracle_id`) | `a2b2fe9e…` (deploy artifact `oracle_id`) |
| `FEEDS` | BTC,USDC (add ETH if used) | BTC,USDC |
| `SIGNER_KEY_B64` | optional (faucet funds a throwaway key) | **required** — pre-funded key, no faucet |
| `SLACK_WEBHOOK_URL` | optional | recommended — `#arch-prime-alerts` incoming webhook |
| `EXPLORER_API_URL` | optional (defaults to `https://explorer.arch.network`) | optional (same default) |

`NETWORK=mainnet` is translated to `--network bitcoin` for `autara-pyth`
(it parses the raw `bitcoin::Network`).

## Mainnet operational notes

- **Fund + monitor the signer.** There is no faucet and no airdrop loop on the
  standalone pusher. If the signer runs dry, pushes stop and markets fail with
  `0x1b70`. Any key works (the oracle program only needs a signature) — use a
  dedicated low-value key and alert on its balance. When `SLACK_WEBHOOK_URL` is
  set, the pusher sends transition alerts at 172,800,000 lamports (48-hour
  warning) and 21,600,000 lamports (6-hour critical), plus a recovery message.
  Failed Slack deliveries retry after five minutes.
- **Balance is read from the explorer API**, not the push RPC: a single RPC node
  can serve stale account state, which flapped the alerts between critical and
  recovered while the signer was actually funded. Reads go to
  `EXPLORER_API_URL` (default `https://explorer.arch.network`) at
  `/api/v1/{mainnet,testnet}/accounts/<pubkey-hex>`. Transactions still go to
  `ARCH_RPC_URL` — the explorer API is read-only. Local networks the explorer
  does not index fall back to RPC.
- **Feeds push atomically.** All feeds go in one transaction; a malformed feed
  drops the whole push. Keep the mainnet feed list to what the markets need.
- If you also run `ROLE=server` on mainnet, set `DISABLE_PRICE_PUSHER=1` there so
  it doesn't double-push (and note the server auto-creates markets under its own
  signer — keep that off mainnet unless that's intended).

## Keep Railway from stopping

The dedicated pusher is a long-running loop. It dies in practice when:

1. **No stable `SIGNER_KEY_B64`** — throwaway faucet keys get `IncorrectAuthority`
   after migrate/restarts.
2. **Signer runs dry** — pushes stop; markets fail with `OracleRateTooOld`.
3. **Busy-loop on fetch errors** (fixed) — CPU spin can get the container killed.
4. **No healthcheck** — Railway cannot restart a silent dead loop.

Hardening in this image:

- Container **requires** `SIGNER_KEY_B64`.
- Pusher exposes **`/health`** and **`/metrics`** on `PORT` (default 9090).
- `/health` is `200` only if a push succeeded within ~90s.
- Metrics: `autara_pusher_pushes_succeeded_total`,
  `autara_pusher_pushes_failed_total`, `autara_pusher_fetch_failures_total`,
  `autara_pusher_consecutive_failures`, `autara_pusher_last_success_unixtime`,
  `autara_pusher_signer_balance_lamports`.

Railway service settings (testnet + mainnet pusher):

| Setting | Value |
|---------|--------|
| Restart policy | Always / on failure |
| Healthcheck path | `/health` |
| Healthcheck timeout | ≥ 30s (allow first push) |
| Public networking | off (private scrape only) |
| Env | from `autara.pusher.{testnet,mainnet}.env` + `SIGNER_KEY_B64` |

For the mainnet service, also set `SLACK_WEBHOOK_URL` to the incoming webhook
secret for `#arch-prime-alerts`. The webhook URL must never be committed.

After deploy: `curl -fsS http://<private-host>/health` and scrape `/metrics`.
Alert rules live in `prometheus-alerts.yml` (`AutaraPusherNotPushing`,
`AutaraPusherConsecutiveFailures`, `AutaraPusherOutOfFunds`).

## CI: publish image + optional Railway redeploy

From GitHub Actions → **autara-deploy-pusher** → Run workflow → choose
`testnet` or `mainnet`.

1. Builds the repo `Dockerfile` and pushes
   `ghcr.io/Arch-Network/autara-lending-v1/autara:<network>-<sha>` and
   `:<network>-latest` (uses `GITHUB_TOKEN` / `packages: write`).
2. If the target GitHub Environment has `RAILWAY_TOKEN` + `RAILWAY_SERVICE_ID`
   (optional `RAILWAY_ENVIRONMENT_ID`), syncs `ROLE` / `NETWORK` /
   `ARCH_RPC_URL` / `ORACLE_PROGRAM_ID` / `FEEDS` (and `SIGNER_KEY_B64` when
   that Environment secret is set) then triggers `railway redeploy`.
3. If Railway secrets are missing, the image is still published and the job
   summary lists the manual Railway next steps.

### GitHub Environment secrets / vars (names only)

| Name | Where | Purpose |
|------|-------|---------|
| `SIGNER_KEY_B64` | Environment secret | Stable pusher signer (synced to Railway when present) |
| `ORACLE_PROGRAM_ID` | Environment variable | Oracle program hex id |
| `ORACLE_FEEDS` | Environment variable | Comma-separated Pyth feed ids |
| `RAILWAY_TOKEN` | Environment secret | Railway API token (**manual**) |
| `RAILWAY_SERVICE_ID` | Environment secret | Pusher service id (**manual**) |
| `RAILWAY_ENVIRONMENT_ID` | Environment secret | Optional Railway environment id/name (**manual**) |

`SIGNER_KEY_B64` for testnet can be set from the local gitignored file:

```bash
gh secret set SIGNER_KEY_B64 --env testnet < autara-deploy/.keys-testnet/pusher.signer.b64
```

Railway token/service ids are not in this repo — set them on the GitHub
Environment before expecting an automatic Railway redeploy.

## Sanity check

A market recovers within one push cycle (~5s) once any write lands on its
oracle PDA. If it stays stale: confirm `ORACLE_PROGRAM_ID` matches the program
the markets were created against, and that pushes reach `Status::Processed`
(the signer is funded), not just log `Sending`.

## Testnet repair: `InvalidPythOracleAccount` / `0x1b69`

**Current product testnet** uses oracle `180ec4dd…` (see
`deployments/testnet.json`). Feeds for that oracle should be created at the
new 152-byte layout.

The **legacy compromised** stage markets (`program 53def2dc…`,
`oracle eee682c2…`) still have **120-byte** pre-authority feed PDAs. The
lending program expects `PythPriceAccount` (**152 bytes**). Symptom:
`SupplyApl` fails at `autara-lib/src/oracle/pyth.rs:48` with
`LendingError(InvalidPythOracleAccount)`. Do **not** point new e2e/pusher
defaults at those ids.

Confirm layout:

```bash
cargo run -p autara-pyth --example check_feed_layout
```

Legacy lines look like `data_len=120 layout=LEGACY(PythPrice)`.

Repair (in order):

1. **Pin a stable pusher signer** on the testnet Railway pusher
   (`SIGNER_KEY_B64`). The first successful post-upgrade push binds feed
   `authority` to that key — a throwaway faucet key will strand the feeds.

   ```bash
   # generate + faucet-fund; copy SIGNER_KEY_B64 to clipboard (secret not printed)
   ./autara-deploy/scripts/provision-pusher-signer.sh --network testnet --fund --copy
   ```

   Paste the clipboard into the Railway testnet pusher's `SIGNER_KEY_B64`, set
   `PUSHER_PUBKEY` from the script's printed arch pubkey (optional, for server
   balance metrics), and redeploy/restart the pusher.
2. **Upgrade only the oracle ELF** at `eee682c2…` (note: `autara-upgrade.yml`
   deliberately does **not** touch the oracle — that is how lending got ahead
   of the feeds). From a commit that includes legacy→152-byte realloc on push:

   ```bash
   # build ELF
   ( cd programs/autara-oracle && cargo-build-sbf --features entrypoint )

   # MUST source the testnet env (otherwise defaults to localnet).
   # re-upload oracle only against the live stage keys
   set -a; source autara-deploy/scripts/autara.testnet.env; set +a
   STEP_DEPLOY_PROGRAM=false STEP_DEPLOY_ORACLE=true \
   STEP_INIT_CONFIG=false STEP_TOKEN_SETUP=false STEP_CREATE_MARKET=false \
   DRY_RUN=1 cargo run -p autara-deploy   # preview first
   # then the same with DRY_RUN=0 for the real upload
   ```

3. Restart / let the pusher run one cycle. Feeds should become
   `data_len=152 layout=NEW(PythPriceAccount)`.
4. Re-run `check_feed_layout`, then retry Supply on
   `arch-swap-nine.vercel.app`.

Do **not** fix this by relaxing the on-chain lending loader: that would accept
unowned legacy feeds forever. Mainnet feeds are created at the new size and do
not need this path.

## Testnet repair: `OracleRateTooOld` / `0x1b70`

If Supply/Borrow fails with `LendingError(OracleRateTooOld)` after feeds are
already `NEW` layout, the pusher has stopped writing (max age is 60s). Check:

```bash
cargo run -p autara-pyth --example check_feed_age
```

`age_secs` must stay under ~60. If it is large:

1. Confirm the Railway **testnet** pusher is up and logging successful sends
   (not only `Sending` — watch for continuous cycles every ~5s).
2. Confirm `SIGNER_KEY_B64` is still the key that owns feed `authority`
   (printed by `check_feed_age`). A throwaway signer gets `IncorrectAuthority`.
3. Refill the signer from the faucet if lamports are low:
   `cargo run -p autara-client --example fund_signer -- --key autara-deploy/.keys-testnet/pusher.key --rpc https://rpc.testnet.arch.network --network testnet`
4. Emergency local refresh (same stable key):

   ```bash
   cargo run -p autara-pyth -- \
     --rpc https://rpc.testnet.arch.network --network testnet \
     --program-id 180ec4dd6eb8d7f2435d4d89c5d166c8e6a3fed3c33f58de25be8e64e94a99dd \
     --signer autara-deploy/.keys-testnet/pusher.key \
     --push-interval-secs 5
   ```

This is **not** an arch-swap frontend bug — the lending program rejects stale
oracles regardless of the UI.
