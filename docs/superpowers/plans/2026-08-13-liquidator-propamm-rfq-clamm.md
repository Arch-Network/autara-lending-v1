# Liquidator PropAMM RFQ and CLAMM Update Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rebase the liquidator onto current lending `main`, replace its embedded PropAMM signer with the RFQ service, and preserve atomic CLAMM routing through the current funded testnet pool.

**Architecture:** The scanner consumes bot-native quote data. `propamm/` owns Arch `0.7.0` RFQ transactions, validation, signing, and HTTP submission; `router.rs` owns CLAMM/Arch `0.6.8`; lending code remains on Arch `0.6.2`. PropAMM re-quotes only the actual seized-collateral balance delta after liquidation.

**Tech Stack:** Rust 2024, Tokio, Reqwest, Serde, Borsh, Autara client, Arch SDK `0.6.2`, CLAMM SDK `0.6.8`, PropAMM `0.7.0`.

## Global Constraints

- Do not add a lending liquidator allowlist; behavior is unchanged.
- Keep lending on exact Arch SDK `0.6.2`.
- Never load or configure the PropAMM quote-signer secret.
- PropAMM URL/program: `https://propamm.arch.network/testnet` / `7a68831501d3a9806feff162e82815a36e1732964a2edd2b461faf69575c3628`.
- CLAMM program/config/pool: `0a0129c4d864d1728c4b6e8f6e0e473197cb111153e379a59b9d93c038efe918` / `5dcbc567a5434cc84303079bfb54be234993e50962a91fda63a17ba8026c8fd0` / `06db06761eb1f114167ea2bbc4cf98cf8f98fbfc0ad18d1821e724cfeeb03461`.
- aBTC/aUSD: `1d46e0dd87393236e4e01252439f46dcbaec7c2255d1fd734e61771a00e8f4e9` (8) / `55c6cee38a31732e2dad821ab1c38f902a7c51efaefb3641d51f3485c4617a45` (6).
- Use TDD and preserve unrelated `.claude/` files.

---

### Task 1: Reconcile the Branch in an Isolated Worktree

**Files:** Rebase `Cargo.toml`, `Cargo.lock`, and `autara-liquidator/**`; preserve the approved spec and this plan.

**Interfaces:** Produces `codex/liquidator-rfq-clamm`, based on local `main`, with final bot files and no obsolete branch-only shared-client changes.

- [ ] **Step 1: Create the worktree**

```bash
git worktree add .worktrees/liquidator-rfq-clamm -b codex/liquidator-rfq-clamm liquidator-deploy
```

- [ ] **Step 2: Rebase**

```bash
git rebase main
```

For conflicts, retain `main` lending client/Pyth/ATA/dependency behavior and final `autara-liquidator/` behavior.

- [ ] **Step 3: Audit and baseline-check**

```bash
git diff --name-status main...HEAD
git diff main...HEAD -- autara-client autara-lib Cargo.toml
cargo check -p autara-liquidator
```

Expected: shared changes already superseded by main are absent; remaining failures are venue integration failures.

---

### Task 2: Add Neutral Venue Types and Safe Configuration

**Files:**
- Modify: `Cargo.toml`, `autara-liquidator/Cargo.toml`, `src/config.rs`, `src/main.rs`
- Create: `autara-liquidator/src/venue.rs`
- Test: inline `config::tests`, `venue::tests`

**Interfaces:**

```rust
pub enum Venue { Clamm, PropAmm }
pub struct VenueQuote<T> {
    pub venue: Venue,
    pub amount_in: u64,
    pub estimated_out: u64,
    pub execution: T,
}
```

- [ ] **Step 1: Write failing config tests**

Assert RFQ defaults `slippage_bps=100`, `request_timeout_ms=8000`, `minimum_expiry_headroom_ms=3000`; assert unknown `quote_signer_keypair` is rejected.

- [ ] **Step 2: Verify red**

```bash
cargo test -p autara-liquidator config::tests -- --nocapture
```

- [ ] **Step 3: Implement the new config**

```rust
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PropAmmConfig {
    pub base_url: String,
    pub expected_program_id: String,
    #[serde(default = "default_slippage_bps")]
    pub slippage_bps: u16,
    #[serde(default = "default_request_timeout_ms")]
    pub request_timeout_ms: u64,
    #[serde(default = "default_minimum_expiry_headroom_ms")]
    pub minimum_expiry_headroom_ms: u64,
}
```

Delete quote-signer loading, config/vault/mint/decimal copies, and local pricing.

- [ ] **Step 4: Add isolated dependencies**

```toml
arch_sdk_propamm = { package = "arch_sdk", version = "=0.7.0" }
bitcoin_propamm = { package = "bitcoin", version = "=0.32.7" }
propammprogram = { path = "../../prop-amm/program", features = ["no-entrypoint"] }
orca_whirlpools = { path = "../../CLAMM/rust-sdk/whirlpool" }
```

- [ ] **Step 5: Verify green and commit**

```bash
cargo test -p autara-liquidator config::tests venue::tests -- --nocapture
git add Cargo.toml Cargo.lock autara-liquidator
git commit -m "refactor: isolate liquidator venue dependencies"
```

---

### Task 3: Implement PropAMM RFQ Discovery and Validation

**Files:** Replace `src/propamm.rs` with `src/propamm/{mod.rs,types.rs,validation.rs}`; add inline tests.

**Interfaces:**

```rust
pub enum RfqSide { Buy, Sell }
pub struct RfqQuote {
    transaction: arch_sdk_propamm::RuntimeTransaction,
    pub side: RfqSide,
    pub amount_in: u64,
    pub estimated_out: u64,
    pub expiry_ts: u128,
}
pub async fn quote_exact_in(
    &self,
    input_mint: [u8; 32],
    output_mint: [u8; 32],
    amount_in: u64,
    user: [u8; 32],
) -> anyhow::Result<Option<RfqQuote>>;
```

- [ ] **Step 1: Write failing side/payout tests**

Test base→quote Sell using `estimated_quote.quote_amount`, quote→base Buy using `estimated_quote.base_amount`, unsupported pair, and zero amount.

- [ ] **Step 2: Write failing validation tests**

Using `execute_trade_instruction`, reject wrong version/signatures/fee payer/signer set/quote signer/program, extra instructions, changed mint/user/side/input/min-out, unexpected writable accounts, and expiry below headroom. Accept the exact fixture.

- [ ] **Step 3: Verify red**

```bash
cargo test -p autara-liquidator propamm:: -- --nocapture
```

- [ ] **Step 4: Implement DTOs and discovery**

```rust
#[derive(Deserialize)]
struct QuoteResponse {
    #[serde(flatten)]
    transaction: arch_sdk_propamm::RuntimeTransaction,
    estimated_quote: EstimatedQuote,
}
#[derive(Serialize)]
struct QuoteRequest {
    base_mint: String,
    quote_mint: String,
    side: &'static str,
    amount: u64,
    user_pubkey: String,
    slippage_bps: u16,
}
```

Reuse one HTTP client. Validate `/health` program ID and cache `/markets` plus the public quote-signer ID.

- [ ] **Step 5: Implement strict message validation**

Decode the single instruction with `parse_instruction`; resolve compiled indices; recompute ATAs/user nonce; require exact account order/flags; require min-out at least estimate minus slippage.

- [ ] **Step 6: Verify green and commit**

```bash
cargo test -p autara-liquidator propamm:: -- --nocapture
git add autara-liquidator/src/propamm autara-liquidator/src/propamm.rs Cargo.lock
git commit -m "feat: validate PropAMM RFQ quotes"
```

---

### Task 4: Implement Liquidator-Only RFQ Signing and Submission

**Files:** Modify `propamm/mod.rs`, `propamm/types.rs`; add mock-server tests.

**Interfaces:**

```rust
pub async fn execute_quote(
    &self,
    quote: RfqQuote,
    liquidator: &arch_sdk::arch_program::bitcoin::key::Keypair,
    network: arch_sdk::arch_program::bitcoin::Network,
) -> anyhow::Result<String>;
```

- [ ] **Step 1: Write failing Axum tests**

Assert `/rfq/swap` receives one nonzero user signature and unchanged message; identical-body retry for `rfq_swap_in_progress`; cached-hash success; no retry for expired/message/signature/price/min-output errors.

- [ ] **Step 2: Verify red**

```bash
cargo test -p autara-liquidator propamm::tests::execute -- --nocapture
```

- [ ] **Step 3: Implement signing/submission**

Reconstruct a temporary `bitcoin_propamm::key::Keypair` from liquidator secret bytes, sign `message.hash()` with `sign_message_bip322`, attach exactly one signature, and POST the identical transaction until success or expiry headroom. Never broadcast locally.

- [ ] **Step 4: Verify green and commit**

```bash
cargo test -p autara-liquidator propamm:: -- --nocapture
git add autara-liquidator/src/propamm
git commit -m "feat: submit liquidator-signed PropAMM RFQs"
```

---

### Task 5: Update the CLAMM Adapter

**Files:** Modify `src/router.rs`, `src/venue.rs`; add inline tests.

**Interfaces:**

```rust
pub struct ClammExecution {
    pub pool: arch_sdk::arch_program::pubkey::Pubkey,
    pub callback: arch_sdk::arch_program::instruction::Instruction,
}
pub async fn best_quote_exact_in(
    &self,
    input_mint: Pubkey,
    output_mint: Pubkey,
    amount_in: u64,
    signer: Pubkey,
) -> anyhow::Result<Option<VenueQuote<ClammExecution>>>;
```

- [ ] **Step 1: Write failing conversion/callback tests**

Assert byte-for-byte conversion across SDK versions. From setup/swap/cleanup instructions select the unique expected-program instruction containing the configured pool; reject zero or multiple matches.

- [ ] **Step 2: Verify red**

```bash
cargo test -p autara-liquidator router::tests -- --nocapture
```

- [ ] **Step 3: Implement the boundary**

Call CLAMM `0.6.8` discovery/quote APIs using its pubkeys. Convert only the selected output/callback to lending types. Reject wrong config/mints, zero active liquidity/output, and unexpected instruction sets.

- [ ] **Step 4: Verify green and commit**

```bash
cargo test -p autara-liquidator router::tests -- --nocapture
git add autara-liquidator/src/router.rs autara-liquidator/src/venue.rs Cargo.lock
git commit -m "feat: route through current CLAMM adapter"
```

---

### Task 6: Integrate Routing and Safe Settlement

**Files:** Modify `src/scanner.rs`, `src/main.rs`; create `src/balances.rs`; add inline tests.

**Interfaces:**

```rust
fn choose_venue(clamm_out: Option<u64>, propamm_out: Option<u64>) -> Option<Venue>;
fn rate_within_slippage(
    initial_in: u64, initial_out: u64,
    fresh_in: u64, fresh_out: u64,
    slippage_bps: u16,
) -> bool;
```

- [ ] **Step 1: Write failing selection/balance tests**

Cover each venue alone, higher output, ties preferring atomic CLAMM, neither venue, positive balance delta, unchanged/decreased balances, standing inventory exclusion, and checked `u128` rate cross-products for differing inputs.

- [ ] **Step 2: Verify red**

```bash
cargo test -p autara-liquidator scanner::tests balances::tests -- --nocapture
```

- [ ] **Step 3: Integrate quotes**

Quote venues concurrently with timeouts. Compare debt-token atoms. Pass `ClammExecution.callback` directly, never the first arbitrary instruction.

- [ ] **Step 4: Implement PropAMM settlement**

Read collateral ATA before no-callback liquidation; after confirmation use only its positive delta. Fresh-quote that delta, apply the rate guard, execute RFQ, parse the hash into lending `Hash`, and wait for processed status. Post-liquidation failure emits an inventory alert and never falls back to CLAMM; pre-liquidation failure may use the existing CLAMM quote.

- [ ] **Step 5: Verify green and commit**

```bash
cargo test -p autara-liquidator scanner::tests balances::tests -- --nocapture
cargo check -p autara-liquidator
git add autara-liquidator/src
git commit -m "feat: settle liquidations through best venue"
```

---

### Task 7: Add Readiness and Current Testnet Configuration

**Files:** Modify `main.rs`, `config.rs`, `liquidator-config.example.json`, and `src/bin/check_atas.rs`; add readiness tests.

- [ ] **Step 1: Write failing readiness tests**

Test both ready, either one ready, and both failed. Only both failed prevents scanning.

- [ ] **Step 2: Verify red**

```bash
cargo test -p autara-liquidator config::tests::readiness -- --nocapture
```

- [ ] **Step 3: Implement checks/config**

Check lending program/ATAs, PropAMM health/program/market, and CLAMM program/config/mints/liquidity/vaults independently. Use current IDs. RFQ config contains only URL, expected program, slippage, timeout, and expiry headroom.

- [ ] **Step 4: Verify green, scan secrets, commit**

```bash
cargo test -p autara-liquidator config::tests -- --nocapture
rg -n "quote_signer_keypair|propamm.*secret" autara-liquidator
git add autara-liquidator
git commit -m "chore: configure liquidator for current testnet venues"
```

---

### Task 8: Full Verification and Operations Guide

**Files:** Create `autara-liquidator/README.md`; modify code only for verified defects.

- [ ] **Step 1: Run static checks**

```bash
cargo fmt --all -- --check
cargo check -p autara-liquidator
cargo clippy -p autara-liquidator --all-targets -- -D warnings
```

- [ ] **Step 2: Run tests**

```bash
cargo test -p autara-liquidator --all-targets -- --nocapture
cargo test --workspace --all-targets
```

- [ ] **Step 3: Probe live services without broadcasting**

Probe PropAMM health/markets/RFQ quote and the CLAMM exact-input quote. Require matching IDs/mints and positive outputs.

- [ ] **Step 4: Dry-run and document operations**

Run with `dry_run: true`; require state reload, both venue readiness results, quotes, and no broadcasts. Document dry-run, guarded live acceptance, inventory alerts, failure injection, and rollback in `autara-liquidator/README.md`.

- [ ] **Step 5: Final checks and commit**

```bash
git diff --check main...HEAD
rg -n "quote_signer_keypair|BEGIN.*PRIVATE|propamm.*secret" autara-liquidator
git status --short
git add autara-liquidator/README.md
git commit -m "docs: add liquidator testnet operations guide"
```
