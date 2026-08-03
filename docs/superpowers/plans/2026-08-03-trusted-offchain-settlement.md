# Curator Capital Sweep Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add curator-only off-chain collateral sweeping with continuously accruing debt and atomic settlement using normal liquidation math.

**Architecture:** Store only the swept collateral amount in reserved `BorrowPosition` bytes. Begin removes all collateral from protocol custody without changing borrow shares; settle calculates against current debt and swept collateral, then atomically restores unused collateral and repays supply debt. Ordinary position mutations are locked while the sweep is pending.

**Tech Stack:** Rust, Arch Program SDK, APL Token, Borsh, bytemuck, existing fixed-point liquidation math.

## Global Constraints

- Keep `BorrowPosition` exactly 224 bytes and consume only reserved padding.
- Append instruction and event tags; do not reorder existing discriminants.
- Only the market curator may begin or settle a capital sweep.
- Swept collateral must never be counted as on-chain collateral in public health state.
- Borrow shares remain untouched at sweep start so interest continues accruing.
- Settlement must use the existing liquidation target, bonus, cap, and rounding logic.
- Curator retains liquidation collateral, liquidation bonus, and off-chain execution profit.
- No timeout, bond, cumulative payment tracking, or supplier-loss finalizer is added.

---

### Task 1: Borrow-position sweep state and locks

**Files:**
- Modify: `autara-lib/src/state/borrow_position.rs`
- Modify: `autara-lib/src/error.rs`
- Test: `autara-lib/src/state/borrow_position.rs`

**Interfaces:**
- Produces: `swept_collateral_atoms() -> u64`, `capital_sweep_pending() -> bool`, `ensure_capital_sweep_inactive() -> LendingResult`, `begin_capital_sweep() -> LendingResult<u64>`, and `settle_capital_sweep(shares_repaid, collateral_returned) -> LendingResult`.

- [ ] Add failing unit tests proving a sweep moves collateral into the swept field without changing debt, a second begin fails, settlement restores returned collateral and reduces debt, and initialization clears sweep state.
- [ ] Run `cargo test -p autara-lib state::borrow_position::tests -- --nocapture` and confirm failure because the API is absent.
- [ ] Replace eight padding bytes with `swept_collateral_atoms: u64`, reduce padding from 128 to 120 bytes, add the state-transition API, and append `CapitalSweepPending` plus `NoCapitalSweepPending` errors.
- [ ] Run the focused tests and confirm they pass while the 224-byte structural assertion remains valid.

### Task 2: Market begin and settlement accounting

**Files:**
- Modify: `autara-lib/src/state/market.rs`
- Modify: `autara-lib/src/state/market_wrapper.rs`
- Test: `autara-lib/src/state/market.rs`
- Test: `autara-lib/src/state/market_wrapper.rs`

**Interfaces:**
- Produces: `CapitalSweepSettlementResult` containing liquidation result, virtual health before, real health after, and collateral returned.
- Produces: wrapper methods `begin_capital_sweep` and `settle_capital_sweep`.

- [ ] Add failing tests for healthy/ineligible begin, begin at or above 100% LTV, debt unchanged at begin, debt growth after `sync_clock`, healthy restoration, partial settlement, full settlement, slippage rejection, and locks on ordinary mutation paths.
- [ ] Run the focused market tests and confirm expected failures.
- [ ] Generalize health/liquidation calculation to accept an explicit collateral amount internally while keeping public normal-liquidation behavior unchanged.
- [ ] Implement begin: validate LTV range, move position state, and decrement collateral-vault accounting.
- [ ] Implement settle: calculate from current debt plus swept collateral, use zero liquidation when virtual health is healthy, apply supply-vault repayment, restore computed unused collateral, clear the sweep, update vault accounting, and reject worsening LTV or a collateral-return amount above the curator's bound.
- [ ] Add pending-state guards to deposit, withdraw, borrow, repay, repay-all, public liquidation, and socialize-loss paths.
- [ ] Run focused library tests and confirm pass.

### Task 3: Instruction builders, tags, and events

**Files:**
- Modify: `autara-lib/src/ixs/liquidation.rs`
- Modify: `autara-lib/src/ixs/types.rs`
- Modify: `autara-lib/src/event.rs`
- Test: corresponding module tests

**Interfaces:**
- Produces: `BeginCapitalSweepInstruction`, `SettleCapitalSweepInstruction`, `begin_capital_sweep_ix`, `settle_capital_sweep_ix`, `CapitalSweepStartedEvent`, and `CapitalSweepSettledEvent`.

- [ ] Add failing serialization round-trip and stable-discriminant tests for instruction tags 20/21 and appended event tags.
- [ ] Run focused serialization tests and confirm failure because variants are absent.
- [ ] Append the two instruction variants and builders with exact required account order.
- [ ] Append the two event variants and Borsh serializers/deserializers.
- [ ] Run focused tests and confirm pass.

### Task 4: Program account validation and processors

**Files:**
- Create: `programs/autara-program/src/ixs/begin_capital_sweep.rs`
- Create: `programs/autara-program/src/ixs/settle_capital_sweep.rs`
- Create: `programs/autara-program/src/processor/begin_capital_sweep.rs`
- Create: `programs/autara-program/src/processor/settle_capital_sweep.rs`
- Modify: `programs/autara-program/src/ixs/mod.rs`
- Modify: `programs/autara-program/src/processor/mod.rs`
- Modify: `programs/autara-program/src/lib.rs`
- Test: new account-validation modules

**Interfaces:**
- Consumes: the Task 2 wrapper methods and Task 3 instruction/event types.
- Produces: dispatched on-chain instructions with atomic APL token transfers.

- [ ] Add failing account-validation tests for correct accounts, non-signing curator, wrong curator, wrong market, wrong vaults, and wrong token mints.
- [ ] Run `cargo test -p autara-program capital_sweep -- --nocapture` and confirm failure because account types are absent.
- [ ] Implement begin accounts and processor: sync clock, call market begin, emit event, transfer all swept collateral market-to-curator with market PDA signing.
- [ ] Implement settle accounts and processor: sync clock, call market settle, enforce bound through the library, emit event, transfer exact supply and returned collateral curator-to-market.
- [ ] Append dispatch wiring and module exports.
- [ ] Run focused program tests and confirm pass.

### Task 5: Regression verification

**Files:**
- Modify only if verification exposes a feature regression.

**Interfaces:**
- Consumes: all prior tasks.
- Produces: formatted, compiling, tested feature branch.

- [ ] Run `cargo fmt --all -- --check` and fix only feature-related formatting.
- [ ] Run `cargo test -p autara-lib --all-targets`.
- [ ] Run `cargo test -p autara-program --all-targets`.
- [ ] Run `cargo test -p autara-integration-tests --test tests capital_sweep -- --nocapture` if the repository dependency graph resolves; otherwise record the pre-existing `arch_sdk` conflict verbatim.
- [ ] Inspect `git diff --check`, `git diff --stat`, and the final diff for accidental changes or compatibility breaks.
- [ ] Re-check every global constraint against implementation and tests.
