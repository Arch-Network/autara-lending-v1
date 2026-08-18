# Per-Market Liquidator Whitelist Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an optional, curator-managed liquidator whitelist to each market while keeping markets permissionless whenever their active whitelist count is zero.

**Architecture:** Store an active-entry count in reserved `MarketConfig` bytes and store each `(market, liquidator)` authorization in its own 80-byte PDA. Add/remove instructions transition the PDA's active flag and the market count together; restricted `Liquidate` instructions require and validate the actual signer-derived PDA before calculating or mutating a liquidation.

**Tech Stack:** Rust, Arch/Solana-style PDAs and account metadata, Borsh instruction/event serialization, bytemuck zero-copy accounts, Cargo unit and integration tests.

## Global Constraints

- `MarketConfig` remains exactly 192 bytes; replace eight bytes of `pad_2` with `active_whitelisted_liquidators: u64` and reduce `pad_2` from 80 to 72 bytes.
- New and existing markets are permissionless when `active_whitelisted_liquidators == 0`.
- Only `MarketConfig::curator` may add or remove entries.
- The whitelist applies only to ordinary `Liquidate`; curator capital sweep remains unchanged.
- Instruction, event, and error discriminants are appended and never reordered.
- A removed entry remains allocated and inactive, and may later be reactivated.
- Whitelist authorization is checked before liquidation calculations, callbacks, token transfers, or state mutation.

---

### Task 1: Market count, entry state, and PDA derivation

**Files:**
- Create: `autara-lib/src/state/liquidator_whitelist.rs`
- Modify: `autara-lib/src/state/market_config.rs`
- Modify: `autara-lib/src/state/mod.rs`
- Modify: `autara-lib/src/pda.rs`
- Modify: `autara-lib/src/error.rs`

**Interfaces:**
- Produces: `LiquidatorWhitelistEntry::{initialize, activate, deactivate, market, liquidator, bump, is_active}`.
- Produces: `MarketConfig::{active_whitelisted_liquidators, liquidations_are_permissionless, increment_active_whitelisted_liquidators, decrement_active_whitelisted_liquidators}`.
- Produces: `liquidator_whitelist_entry_seed`, `liquidator_whitelist_entry_seed_with_bump`, and `find_liquidator_whitelist_entry_pda`.
- Produces: appended `LendingError::{LiquidatorAlreadyWhitelisted, LiquidatorNotWhitelisted}`.

- [ ] **Step 1: Write failing state and PDA tests**

Add tests that assert the desired API and transitions:

```rust
#[test]
fn whitelist_entry_transitions_are_checked() {
    let market = Pubkey::new_unique();
    let liquidator = Pubkey::new_unique();
    let mut entry = LiquidatorWhitelistEntry::default();
    entry.initialize(market, liquidator, 7).unwrap();
    assert!(entry.is_active());
    assert_eq!(entry.market(), &market);
    assert_eq!(entry.liquidator(), &liquidator);
    assert_eq!(entry.bump(), &[7]);
    assert_eq!(entry.activate(), Err(LendingError::LiquidatorAlreadyWhitelisted.into()));
    entry.deactivate().unwrap();
    assert!(!entry.is_active());
    assert_eq!(entry.deactivate(), Err(LendingError::LiquidatorNotWhitelisted.into()));
    entry.activate().unwrap();
}

#[test]
fn whitelist_count_controls_permissionless_mode() {
    let mut config = test_config();
    assert!(config.liquidations_are_permissionless());
    config.increment_active_whitelisted_liquidators().unwrap();
    assert_eq!(config.active_whitelisted_liquidators(), 1);
    assert!(!config.liquidations_are_permissionless());
    config.decrement_active_whitelisted_liquidators().unwrap();
    assert!(config.liquidations_are_permissionless());
}

#[test]
fn liquidator_whitelist_pda_is_market_and_liquidator_specific() {
    let program = Pubkey::new_unique();
    let market = Pubkey::new_unique();
    let liquidator = Pubkey::new_unique();
    let (pda, bump) = find_liquidator_whitelist_entry_pda(&program, &market, &liquidator);
    assert_eq!(
        Pubkey::create_program_address(
            &liquidator_whitelist_entry_seed_with_bump(&market, &liquidator, &[bump]),
            &program,
        ).unwrap(),
        pda,
    );
}
```

- [ ] **Step 2: Run the focused tests and verify RED**

Run: `cargo test -p autara-lib whitelist --lib`

Expected: compilation fails because the entry type, count accessors, PDA helpers, and errors do not exist.

- [ ] **Step 3: Implement the minimal state model**

Create an 80-byte unique-size zero-copy entry:

```rust
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable, Default)]
pub struct LiquidatorWhitelistEntry {
    market: Pubkey,
    liquidator: Pubkey,
    bump: [u8; 1],
    active: [u8; 1],
    padding: Padding<14>,
}
```

Use `[u8; 1]` rather than `bool` for bytemuck safety. Initialization creates an active entry; activation/deactivation return the dedicated errors on invalid transitions. Add the entry's 80-byte size to the unique-account-size assertion. Add the count to `MarketConfig`, reduce its padding, expose the count/mode accessors, and use checked add/sub helpers.

- [ ] **Step 4: Run state tests and verify GREEN**

Run: `cargo test -p autara-lib whitelist --lib`

Expected: all whitelist-focused library tests pass and the 192-byte `MarketConfig` assertion remains valid.

- [ ] **Step 5: Commit the state layer**

```bash
git add autara-lib/src/state/liquidator_whitelist.rs autara-lib/src/state/market_config.rs autara-lib/src/state/mod.rs autara-lib/src/pda.rs autara-lib/src/error.rs
git commit -m "feat: add liquidator whitelist state"
```

### Task 2: Wire instructions, builders, and events

**Files:**
- Create: `autara-lib/src/ixs/liquidator_whitelist.rs`
- Modify: `autara-lib/src/ixs/mod.rs`
- Modify: `autara-lib/src/ixs/types.rs`
- Modify: `autara-lib/src/ixs/liquidation.rs`
- Modify: `autara-lib/src/event.rs`

**Interfaces:**
- Consumes: `find_liquidator_whitelist_entry_pda` from Task 1.
- Produces: `AddWhitelistedLiquidatorInstruction { liquidator: Pubkey, bump: u8 }`.
- Produces: `RemoveWhitelistedLiquidatorInstruction { liquidator: Pubkey }`.
- Produces: `add_whitelisted_liquidator_ix(...) -> (Pubkey, Instruction)` and `remove_whitelisted_liquidator_ix(...) -> (Pubkey, Instruction)`.
- Changes: `liquidate_ix(..., liquidator_whitelist_entry: Option<Pubkey>, ix_callback: Option<Instruction>)` inserts the optional proof immediately before the Autara program account.
- Produces: `LiquidatorWhitelistUpdatedEvent` and appended add/remove event variants.

- [ ] **Step 1: Write failing serialization and account-order tests**

Add tests asserting instruction tags `22` and `23`, event tags appended after the sweep events, PDA returned by both builders, curator signer/writable status for add funding, system-program presence for add, and the exact liquidation ordering:

```rust
let ix = liquidate_ix(
    program, market, position, liquidator, liquidator_supply, liquidator_collateral,
    market_supply, market_collateral, supply_oracle, collateral_oracle,
    u64::MAX, 0, Some(entry), Some(callback),
);
assert_eq!(ix.accounts[10].pubkey, entry);
assert_eq!(ix.accounts[11].pubkey, program);
assert_eq!(ix.accounts[12].pubkey, callback_program);
```

Also assert that `None` keeps the existing program account at index 10.

- [ ] **Step 2: Run wire tests and verify RED**

Run: `cargo test -p autara-lib whitelist --lib`

Expected: compilation fails because whitelist instructions/events and the new `liquidate_ix` parameter do not exist.

- [ ] **Step 3: Implement appended wire types and builders**

Append the two instruction variants and explicit `TryFrom<u8>` mappings without changing tags 0-21. Build add accounts as market, curator payer/signer, entry PDA, system program, Autara program; build remove accounts as market, curator signer, entry PDA, Autara program. Append two event variants sharing:

```rust
pub struct LiquidatorWhitelistUpdatedEvent {
    pub market: Pubkey,
    pub curator: Pubkey,
    pub liquidator: Pubkey,
    pub active_whitelisted_liquidators: u64,
}
```

- [ ] **Step 4: Run wire tests and verify GREEN**

Run: `cargo test -p autara-lib whitelist --lib`

Expected: whitelist instruction, event, PDA, and liquidation account-order tests pass.

- [ ] **Step 5: Commit the wire layer**

```bash
git add autara-lib/src/ixs/liquidator_whitelist.rs autara-lib/src/ixs/mod.rs autara-lib/src/ixs/types.rs autara-lib/src/ixs/liquidation.rs autara-lib/src/event.rs
git commit -m "feat: add liquidator whitelist instructions"
```

### Task 3: Curator add/remove account validation and processors

**Files:**
- Create: `programs/autara-program/src/ixs/liquidator_whitelist.rs`
- Modify: `programs/autara-program/src/ixs/mod.rs`
- Modify: `programs/autara-program/src/ixs/test_utils.rs`
- Create: `programs/autara-program/src/processor/add_whitelisted_liquidator.rs`
- Create: `programs/autara-program/src/processor/remove_whitelisted_liquidator.rs`
- Modify: `programs/autara-program/src/processor/mod.rs`
- Modify: `programs/autara-program/src/state.rs`
- Modify: `programs/autara-program/src/lib.rs`
- Modify: `programs/autara-program/src/error.rs`

**Interfaces:**
- Consumes: Task 1 state/PDA helpers and Task 2 wire types/events.
- Produces: `AddWhitelistedLiquidatorAccounts` supporting either an uncreated system-owned PDA or an initialized program-owned tombstone.
- Produces: `RemoveWhitelistedLiquidatorAccounts` requiring an initialized program-owned entry.
- Appends: `LendingAccountValidationError::InvalidLiquidatorWhitelistEntry`.

- [ ] **Step 1: Write failing account-validation tests**

Extend `AutaraAccounts` with active whitelist entries and add tests proving: the market curator succeeds; a different or non-signing curator fails; a PDA for another market/liquidator fails; remove rejects mutated owner; and the stored market/liquidator must match the instruction data and PDA.

- [ ] **Step 2: Run account tests and verify RED**

Run: `cargo test -p autara-program liquidator_whitelist --lib`

Expected: compilation fails because account parsers, state initialization support, processors, and dispatch variants do not exist.

- [ ] **Step 3: Implement parser and processor behavior**

Implement `ZeroCopyInitialized for AutaraAccount<LiquidatorWhitelistEntry>`. For first add, create the PDA with `minimum_rent(size_of::<LiquidatorWhitelistEntry>())` and the PDA signer seeds, initialize it active, then increment the market count. For an existing program-owned tombstone, validate stored keys, reactivate it, and increment once. Remove validates the active entry, marks it inactive, and decrements once. Both instructions emit the updated count through `log_ix` and are dispatched from `autara_process_instruction`.

- [ ] **Step 4: Run processor/account tests and verify GREEN**

Run: `cargo test -p autara-program liquidator_whitelist --lib`

Expected: all curator, PDA, owner, stored-data, transition, and event-construction tests pass.

- [ ] **Step 5: Commit curator management**

```bash
git add programs/autara-program/src/ixs/liquidator_whitelist.rs programs/autara-program/src/ixs/mod.rs programs/autara-program/src/ixs/test_utils.rs programs/autara-program/src/processor/add_whitelisted_liquidator.rs programs/autara-program/src/processor/remove_whitelisted_liquidator.rs programs/autara-program/src/processor/mod.rs programs/autara-program/src/state.rs programs/autara-program/src/lib.rs programs/autara-program/src/error.rs
git commit -m "feat: manage per-market liquidator whitelist"
```

### Task 4: Enforce whitelist during ordinary liquidation

**Files:**
- Modify: `programs/autara-program/src/ixs/liquidate.rs`
- Modify: `programs/autara-program/src/error.rs`
- Modify: `autara-lib/src/ixs/liquidation.rs`
- Modify: `autara-integration-tests/tests/autara/liquidate.rs`

**Interfaces:**
- Consumes: `MarketConfig::liquidations_are_permissionless` and `LiquidatorWhitelistEntry`.
- Appends: `LendingAccountValidationError::{MissingLiquidatorWhitelistEntry, LiquidatorNotWhitelisted}`.
- Produces: dynamic `LiquidateAccounts::from_accounts`: no proof consumed at count zero; one typed proof consumed and validated when count is nonzero.

- [ ] **Step 1: Write failing liquidation authorization tests**

Add unit tests for: zero-count liquidation with the old ten parsed accounts; restricted liquidation with the matching active PDA; missing proof; inactive entry; another liquidator's entry; another market's entry; and proof positioned before callback/program suffix. Add an integration case that activates two liquidators, confirms both may liquidate, removes one, confirms only that signer is rejected, removes the last, and confirms permissionless liquidation resumes.

- [ ] **Step 2: Run focused liquidation tests and verify RED**

Run: `cargo test -p autara-program liquidate --lib`

Expected: restricted-market tests fail because `LiquidateAccounts` does not consume or verify a whitelist proof.

- [ ] **Step 3: Implement pre-mutation authorization**

After parsing the existing ten accounts, read the market count. If zero, retain `None`. If nonzero, consume exactly one proof account, convert it to `ZeroCopyOwnedAccount<AutaraAccount<LiquidatorWhitelistEntry>>`, and validate owner, expected PDA from `(market, liquidator signer)`, stored market/liquidator, and active state. Map absence to `MissingLiquidatorWhitelistEntry` and invalid/inactive/substituted entries to `LiquidatorNotWhitelisted`. Keep `process_liquidate` unchanged so authorization necessarily completes before it loads mutable state or invokes transfers/callbacks.

- [ ] **Step 4: Run focused tests and verify GREEN**

Run: `cargo test -p autara-program liquidate --lib`

Expected: permissionless compatibility and all restricted authorization tests pass.

- [ ] **Step 5: Commit enforcement**

```bash
git add programs/autara-program/src/ixs/liquidate.rs programs/autara-program/src/error.rs autara-lib/src/ixs/liquidation.rs autara-integration-tests/tests/autara/liquidate.rs
git commit -m "feat: enforce liquidator whitelist"
```

### Task 5: High-level client management and automatic proofs

**Files:**
- Modify: `autara-client/src/client/tx_builder.rs`
- Modify: `autara-client/src/client/client_with_signer.rs`
- Modify: `autara-integration-tests/tests/fixture/autara_fixture.rs`

**Interfaces:**
- Produces: `TransactionBuilder::{add_whitelisted_liquidator, remove_whitelisted_liquidator}`.
- Produces: `AutaraClient::{add_whitelisted_liquidator, remove_whitelisted_liquidator}` returning `AutaraEvents`.
- Changes: `TransactionBuilder::liquidate` derives and supplies the signer entry PDA only when the fetched market count is nonzero; its public signature remains unchanged.

- [ ] **Step 1: Write failing client/integration tests**

Add tests/build assertions that management methods construct the expected PDA/instruction and that restricted liquidation includes the current authority's PDA while a zero-count market retains the old account shape.

- [ ] **Step 2: Run client checks and verify RED**

Run: `cargo test -p autara-client --all-targets`

Expected: compilation fails because the management methods and new low-level builder argument are not wired.

- [ ] **Step 3: Implement client methods and proof selection**

Add management methods that use the current authority as curator and payer. In `liquidate`, compute:

```rust
let whitelist_entry = (!market.market().config().liquidations_are_permissionless())
    .then(|| find_liquidator_whitelist_entry_pda(
        &self.autara_program_id,
        market_key,
        &self.authority_key,
    ).0);
```

Pass this option to `liquidate_ix` before the callback argument.

- [ ] **Step 4: Run client checks and verify GREEN**

Run: `cargo test -p autara-client --all-targets`

Expected: all client targets compile and tests pass.

- [ ] **Step 5: Commit client support**

```bash
git add autara-client/src/client/tx_builder.rs autara-client/src/client/client_with_signer.rs autara-integration-tests/tests/fixture/autara_fixture.rs
git commit -m "feat: expose liquidator whitelist client methods"
```

### Task 6: Formatting, regression verification, and compatibility audit

**Files:**
- Modify: only files reformatted by `cargo fmt`.

**Interfaces:**
- Consumes: all prior tasks.
- Produces: a fully formatted, verified whitelist implementation without unrelated workspace changes.

- [ ] **Step 1: Format and inspect the diff**

Run: `cargo fmt --all`

Run: `git diff --check`

Run: `git status --short`

Expected: no whitespace errors; only intended tracked files plus the user's pre-existing untracked files appear.

- [ ] **Step 2: Run the library suite**

Run: `cargo test -p autara-lib --all-targets`

Expected: all library tests pass, including fixed sizes, tags, builders, events, and state transitions.

- [ ] **Step 3: Run the program suite**

Run: `cargo test -p autara-program --all-targets`

Expected: all program account-validation and processor tests pass.

- [ ] **Step 4: Run client and integration coverage**

Run: `cargo test -p autara-client --all-targets`

Run: `cargo test -p autara-integration-tests --test tests liquidate`

Expected: client targets pass and whitelist liquidation integration cases pass. If the known workspace conflict between `autara-liquidator`'s `arch_sdk ^0.6.4` and integration tests' `arch_sdk =0.6.3` blocks resolution, temporarily exclude `autara-liquidator`, run the commands, then restore `Cargo.toml` and `Cargo.lock` without committing that workaround.

- [ ] **Step 5: Audit the committed design invariants**

Confirm from the final diff: default count is zero; market size is 192; entry size is unique; add/remove count and active flag transition atomically; the signer and market derive the proof PDA; missing/invalid/inactive proof fails before `process_liquidate`; last removal restores permissionless mode; sweep instructions are untouched; tags/errors are appended.

- [ ] **Step 6: Commit formatting if necessary**

```bash
git add autara-lib autara-client programs/autara-program autara-integration-tests
git commit -m "style: format liquidator whitelist changes"
```
