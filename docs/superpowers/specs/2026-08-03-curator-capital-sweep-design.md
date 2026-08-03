# Curator Capital Sweep Design

## Goal

Allow a market curator to remove the collateral of an unhealthy borrow position for an off-chain sale while leaving the position's borrow shares untouched so debt continues accruing. The curator later settles atomically by returning the protocol-computed supply repayment and unused collateral.

## Trust and solvency model

The curator is explicitly trusted for custody and liveness while a sweep is pending. Swept collateral is not counted as on-chain collateral and is exposed separately in position state. The protocol makes no on-chain promise that the curator will settle before the position reaches 100% LTV.

Starting a sweep is allowed only when the position is liquidatable but not yet insolvent:

- `unhealthy_ltv <= virtual_ltv < 100%`
- the position has nonzero debt and nonzero on-chain collateral
- no sweep is already pending
- the signer is the market curator

## Position state

Reuse eight bytes from the fixed-size `BorrowPosition` padding:

```rust
swept_collateral_atoms: u64
```

A nonzero value means a capital sweep is pending. No timestamp or cumulative settlement counters are stored. `borrowed_shares` and `initial_borrowed_atoms` are unchanged at sweep start.

The account remains 224 bytes so existing account allocation and deserialization stay compatible. Existing accounts read the new field as zero because it occupies reserved zeroed padding.

## Begin flow

`BeginCapitalSweep` is curator-only and receives the market, borrow position, curator collateral token account, market collateral vault, token program, and both oracle accounts.

After syncing the market clock, it validates eligibility using the real on-chain position health. It then:

1. copies `collateral_deposited_atoms` into `swept_collateral_atoms`;
2. sets `collateral_deposited_atoms` to zero;
3. decreases the market collateral-vault accounting by the swept amount;
4. transfers that exact amount from the market collateral vault to the curator's collateral token account;
5. emits a `CapitalSweepStarted` event containing the pre-sweep health and swept amount.

Supply-vault debt and borrow shares are not changed, so `sync_clock` continues accruing interest normally.

## Pending-state restrictions

While `swept_collateral_atoms != 0`, ordinary operations that mutate the borrow position are rejected with `CapitalSweepPending`: collateral deposit/withdrawal, borrow, repay, repay-all, public liquidation, and loss socialization. This keeps the settlement inputs stable except for interest and oracle movement. Read paths expose zero on-chain collateral plus the separate swept amount; they must not present swept collateral as protocol custody.

## Settlement flow

`SettleCapitalSweep` is curator-only. Its slippage bounds are:

```rust
max_borrowed_atoms_to_repay: u64
max_collateral_atoms_to_return: u64
```

After syncing interest, the market calculates virtual health from current debt represented by `borrowed_shares` and `swept_collateral_atoms`.

- If virtual LTV remains at or above `unhealthy_ltv`, run the same normal liquidation calculation, target LTV, liquidation bonus, rounding, and max-repay cap used by public liquidation.
- If oracle movement has made virtual LTV healthy, compute a zero repayment and zero liquidation entitlement, so all swept collateral is restored.

Let:

```text
collateral_kept = base_liquidation_collateral + liquidation_bonus
collateral_returned = swept_collateral_atoms - collateral_kept
```

The instruction fails if `collateral_returned > max_collateral_atoms_to_return`. It then atomically:

1. transfers `borrowed_atoms_to_repay` from the curator's supply account into the market supply vault;
2. transfers `collateral_returned` from the curator's collateral account into the market collateral vault;
3. repays the corresponding borrow shares in the supply vault and borrow position;
4. sets real `collateral_deposited_atoms = collateral_returned`;
5. clears `swept_collateral_atoms`;
6. increases collateral-vault accounting by `collateral_returned`;
7. verifies real post-settlement LTV is not worse than virtual pre-settlement LTV;
8. emits `CapitalSweepSettled` with repayment, collateral retained, bonus, collateral returned, and before/after health.

All state and token transfers are atomic. A capped partial settlement may leave the position unhealthy, matching existing partial liquidation behavior; after settlement clears the pending state, normal liquidation can continue. If virtual LTV reaches or exceeds 100%, the existing liquidation branch applies: no bonus, and an uncapped settlement repays all debt and consumes all swept collateral.

## Curator economics

The curator keeps:

- the normal liquidation bonus calculated by the market;
- any additional execution profit or loss from the off-chain sale.

The protocol only requires the calculated supply repayment and unused collateral to be returned. It does not track the curator's off-chain proceeds.

## Compatibility and testing

Instruction and event tags are appended, never reordered. Tests cover fixed account size, sweep state transitions, pending-operation locks, eligibility boundaries, interest accrual during a pending sweep, healthy restoration, partial settlement, full settlement, slippage failure, curator/account validation, instruction serialization, and event serialization.
