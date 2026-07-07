# Liquidator ↔ CLAMM end-to-end test on testnet4 (tBTC/tUSDC)

Date: 2026-06-02 · Branch: `liquidator`

## Goal
Prove `autara-liquidator` detects an undercollateralized tBTC/tUSDC loan and liquidates it
**live on testnet4**, seizing tBTC and swapping tBTC→tUSDC via the CLAMM whirlpool in the same
transaction. Verified by bot logs + explorer.testnet.arch.network + on-chain state diffs.

## Verified facts (probed on testnet4, block ~15.16M)
| Component | Address (hex) | Status |
|---|---|---|
| Autara lending program | `53def2dc8516302842b10e356914d2a5f6b33425ba42aec684f706aa1cf64192` (= pubkey of keys/autara-stage.key) | deployed |
| Autara oracle program | `eee682c27db375bebbc17ed9a76aaa935c8b72bc7de50d736f03e2dfbed84b15` (= keys/autara-pyth-stage.key) | deployed |
| Oracle signer | `b5eb801401791f83345cf81bf8d4c04daf34fa203e715467dc73a6995e2d21de` (= keys/autara-cli-signer.key) | funded |
| CLAMM whirlpool program | `5c748cd0eb8a1a4aa5793f744f3ba00b814a7bdbb3ec568cc9cbb985480fbe98` | deployed |
| CLAMM whirlpools_config | `00a54bcf74a19d04124e92b7fb14001cedb4273a72afd8f006a5b37690957e2e` | deployed (NOTE: example config's `082ca6…` is STALE) |
| tBTC/tUSDC pool | `6f6ce6e99c35eb75c99abc41acc06ed28fc2b63cec50dc0472d0c1b5b17cd022` | deployed |
| tBTC mint (8 dec) | `726179cf49b6dc407c1438cec98815d92277b625b09de81818f5f3a57989f1f1` | exists |
| tUSDC mint (6 dec) | `a2ff4e218e9ddda64c35ee926c00a7715ec7116065b04d8c537f6030c87e49e5` | exists |

### Target markets (supply=tUSDC / collateral=tBTC) — both owned by lending program
- Conservative 80% LTV: `30272668a9327f79a559343879c40d802249a3494153cae6660b273121ad54b3`
- Balanced 86% LTV: `e5420175199b803a1dd85df0fc1722095d44241da679cbd58a47d2759ec2b24d`

## Keys / roles (secp256k1; .key files are 64-char ASCII hex = 32-byte secret)
| Role | Key / source | Purpose |
|---|---|---|
| Oracle signer | keys/autara-cli-signer.key | push tBTC/tUSDC prices to oracle program |
| Mint authority tBTC | priv `d29022a340318f94e18319e6b9c187e8c68aeb10e5a2adf3b9405f3552044c01` | mint tBTC test funds |
| Mint authority tUSDC | priv `e4586753d92f716cea88441ad6caa4b0e1b85dd43daf5d94dfcdae38ea1a73c4` | mint tUSDC test funds |
| Borrower | dedicated key | deposit tBTC, borrow tUSDC |
| Liquidator (bot) | dedicated key | funded: gas + tUSDC buffer + tBTC/tUSDC ATAs |
| Supplier | minted tUSDC | seed borrowable liquidity if market is short |

## Mechanism findings
- Liquidatable when `ltv = borrow_value/collateral_value >= unhealthy_ltv`. collateral uses oracle
  lower bound (rate−conf), borrow uses upper bound (rate+conf). conf default ≈1% of price.
- Oracle price set via `autara-oracle` program — **any signer** can push (`PythPrice::from_dummy(feed_id, price_f64)`,
  expo −8). `max_age = 60s`: a stale price ⇒ liquidation reverts ⇒ must re-push every <60s.
- Liquidator (scanner.rs): scan all positions → if `ltv>=unhealthy_ltv`, quote CLAMM swap
  collateral→supply for full collateral, pass swap ix as **callback** into `liquidate()`, broadcast.
  Key log lines: `LIQUIDATABLE position…`, `ROUTE found: pool=…`, `Liquidation SUCCESS … events=…`.

## Flow
0. **Build** autara-client CLI + autara-liquidator (WIP branch — fix compile breakage).
1. **Preflight** read both markets: confirm supply=tUSDC/collateral=tBTC, capture oracle feed pubkeys,
   `unhealthy_ltv`, decimals, current supply liquidity + utilization.
2. **Seed + borrow** per market: push tBTC≈$100k / tUSDC=$1; mint+supply tUSDC if short; mint tBTC to
   borrower; deposit ~0.1 tBTC; borrow tUSDC near max_ltv. Confirm healthy.
3. **Force undercollateralization** push low tBTC price (computed so ltv≥unhealthy_ltv); run feeder loop
   re-pushing every <60s.
4. **Run liquidator live** config: testnet4 RPC, program `53def2dc…`, whirlpools_config `00a54bcf…`,
   network testnet, dry_run=false, poll 5s, restrict_tokens=[tBTC,tUSDC]. Capture RUST_LOG output.
5. **Verify** liquidation tx on explorer; position debt/collateral dropped; liquidator tUSDC up; CLAMM
   swap present in tx; bonus received.

## Risks / open items (resolve in execution)
- WIP branch may not compile.
- Native-gas funding for fresh wallets (faucet vs transfer from funded admin).
- Whether liquidate+swap-callback needs upfront tUSDC ⇒ fund bot generously.
- Confirm CLI can mint with external authority + push arbitrary oracle price; else add tiny Rust bin.

## Success criteria
A real testnet4 liquidation tx (explorer-confirmed) on each market, containing a CLAMM tBTC→tUSDC swap,
that reduces the borrow position's LTV, driven by the unmodified liquidator bot loop (logs show the
full detect→route→liquidate sequence).

## RESULT (2026-06-02) — PASS, both markets

Liquidator detected → routed via CLAMM → liquidated BOTH bad-debt positions live on testnet4.

- 86% market `e5420175`, position `c6a5a843`: liquidation tx sig
  `b4370c4059625dd0c94a577eff324f4f3b41641d17c4c4a78a1ee6f0cee4ee07f2d4f55cdaf4a96e2704094791f5deb9675f31cfc7ec5cc546cd7800e4ca0eaf`
  — health ltv 1.154 → 0, supply_repaid 8000000776, collateral_liquidated 10000000.
- 80% market `30272668`, position `33953a8e`: liquidation tx sig
  `86d074a86a3daddf8d02fe111dc6bf11b3bb2dcfc7f679f3db2f551b0c2a6e1856343aafb87fa181a11e9258fed731db76e2e60195b688641f44c78a0f16b4dd`
  — health ltv 1.154 → 0, supply_repaid 8000000300, collateral_liquidated 10000000.
- Over-borrow setup txs: `48fc4d27…` (80%), `40af82b1…` (86%).
- Post-state: both borrower positions collateral=0 / ltv=0. Liquidator tBTC net 0 (sold 0.2, seized 0.2),
  tUSDC −$5,863 = $16,000 repaid − ~$10,137 CLAMM swap proceeds. Numbers reconcile ⇒ swap settled.

## Issues found (need follow-up before production)
1. **[RESOLVED via standing float] Liquidator needs collateral inventory to even quote.** CLAMM SDK
   `rust-sdk/whirlpool/src/token.rs:133` (`prepare_token_accounts_instructions`) hard-checks the signer's
   input-token balance; the `_quote_only` flag in `swap_instructions_with_options` is unused/ignored. The
   bot holds 0 of the input token at quote time (it only receives collateral *during* the liquidate
   callback), so swap quotes fail with "Insufficient balance for mint …". **Resolution chosen:** keep a
   standing inventory in the bot wallet of BOTH tokens — tBTC (input for tBTC→tUSDC swaps in
   tBTC-collateral markets) and tUSDC (input for tUSDC→tBTC swaps in the reverse markets). Funded the bot
   to **2.15 tBTC + ~$51k tUSDC** and verified it liquidates a fresh bad-debt position with NO last-second
   top-up (tx `003f4050…0920da`, ltv 1.159 → 0, 0 errors; inventory after: 2.15 tBTC + $48,083 tUSDC).
   Limitation: the float must exceed the largest single position's seized collateral; the alternative
   structural fix is to make the SDK honor `_quote_only` so no float is needed.
   Side fix: `autara-cli token mint` now creates the recipient ATA only if missing, so balance top-ups work.
2. **[FIXED] Swap was sized to FULL collateral + max_repay=u64::MAX** (`scanner.rs`). Only correct for
   bad-debt (ltv≥1 ⇒ full seizure). For an unhealthy-but-ltv<1 position the program does a PARTIAL
   liquidation (seizes < full), but the callback sold the full collateral ⇒ oversell (revert / inventory
   bleed). **Fix:** the scanner now previews the liquidation via
   `market_wrapper.market().compute_liquidation_result_with_fee(pos, collateral_oracle, supply_oracle, u64::MAX)`
   and sizes the swap to `total_collateral_atoms_to_liquidate()` (collateral + bonus = what the liquidator
   actually receives). Re-tested on a partial position (80% market):
   - position ltv 0.9254 → 0.8006 (target), NOT wiped (0.0177 tBTC + ~$980 debt remain);
   - `ROUTE found collateral_in=8224459` ≈ on-chain seized `collateral_liquidated 7837349 + fee 391867`;
   - liquidator earned the 5% bonus; tBTC float preserved (15.00M → 15.00M atoms, no bleed); 0 failures.
   - partial-liquidation txs: mine `1d15be4e221ff0ec…c8208afc` (pos 33953a8e), foreign `5259e14c…304419d4`.
3. **Stale `whirlpools_config` in the example config** (`082ca6…`); correct CLAMM config is `00a54bcf…`.
4. **arch_sdk version conflict** with CLAMM (`=0.6.3` vs orca `^0.6.4`); bumped workspace to `=0.6.4`.

## PropAMM second liquidity path (2026-06-02) — PASS, profit-routed

Added PropAMM (RFQ vault AMM, program `63595891…`) as a second swap venue, routed by output.

- **Constraint:** PropAMM's `ExecuteTrade` needs the `quote_signer` to co-sign → it cannot be an atomic
  liquidate CPI callback (a callback only inherits the outer tx's signers). So **hybrid execution**:
  CLAMM = atomic callback (unchanged); PropAMM = decoupled `liquidate (no callback, repay from float)`
  then a separate swap tx signed `[quote_signer, user]` (we hold the quote_signer key locally).
- **Routing:** per liquidation the scanner quotes BOTH venues for the seized collateral→supply swap and
  picks the higher output. New module `autara-liquidator/src/propamm.rs` (Quote/ix replicated to dodge an
  arch_program pin clash; price from backend `GET /health`; exact integer amount math).
- **Test (scripts/liq-test-suite.mjs with `propamm` enabled): 10/10 PASS.** Routing is genuinely
  per-direction (not "always PropAMM"):
  - tBTC→tUSDC (sell): PropAMM ($68.9k) > CLAMM pool ($50.6k) ⇒ **PropAMM** (S1 full, S2 partial). swaps OK.
  - tUSDC→tBTC (reverse buy): CLAMM's cheap tBTC yields more tBTC ⇒ **CLAMM** (S4 full, atomic). 
  - healthy S3/S5 untouched; idempotent re-run; 0 failures; PropAMM swaps certified.

## Repo changes made
- `Cargo.toml`: arch deps `=0.6.3` → `=0.6.4`.
- `autara-liquidator/src/scanner.rs`: size the CLAMM swap to the actually-seized collateral (fix #2).
- `autara-client/src/bin/cli.rs`: `token mint` creates the ATA only if missing (enables top-ups; fix #1 funding).
- `autara-liquidator/src/propamm.rs`: PropAMM RFQ venue client (quote/estimate/execute).
- `autara-liquidator/src/{config,main,scanner}.rs`: PropAMM config + venue wiring + two-venue profit routing.
- `autara-liquidator/Cargo.toml`: borsh + reqwest + apl-token deps.
- `scripts/liq-test-suite.mjs`: re-runnable rigorous suite (fresh borrower/run; CLAMM + PropAMM scenarios).
- `liquidator-config.local.json`: `propamm` section; `keys/propamm-quote-signer.key`.
- `autara-client/src/bin/over_borrow.rs` (+ `[[bin]]`): test harness — atomic [create/deposit/push-inflated/borrow].
- `liquidator-config.local.json`: live config (admin-stage liquidator, dry_run=false, restrict tBTC/tUSDC).
- `keys/liqtest-tbtc-authority.key`, `keys/liqtest-tusdc-authority.key`: mint-authority keyfiles (testnet).
