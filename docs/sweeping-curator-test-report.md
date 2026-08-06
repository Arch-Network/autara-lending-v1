# Sweeping Curator: Test and Risk Report

Date: 2026-08-06
Branch: `codex/sweeping-capital`
Base: `origin/main` at `487453d`

## Verdict

The sweep accounting model behaved correctly in the unit, boundary, and property tests added in this review. The tests did not find a path that creates collateral, erases debt without repayment, worsens LTV during a successful settlement, or lets a non-curator begin or settle a sweep.

The feature should not yet be described as fully end-to-end verified. Four new transaction tests compile. A live run successfully creates and funds the position, then the configured testnet program rejects `begin_capital_sweep` with `InvalidInstructionData`. The deployed program predates the new instruction, so end-to-end verification requires deploying this branch's program artifact to a compatible test environment.

There are also two concrete correctness/observability gaps and two important trust/operational risks described below.

## Coverage added

Eight model/property tests now cover:

- exact eligibility boundaries: `LTV == unhealthy_ltv` is accepted and `LTV == 100%` is rejected;
- a zero repayment cap, which returns all swept collateral and leaves debt unchanged;
- debt and interest growth while collateral is off-chain;
- exact collateral-return slippage bounds and rollback before mutation;
- another sweep after a partial settlement that remains unhealthy but below 100% LTV;
- a partial settlement after insolvency, including the handoff to ordinary liquidation;
- accounting isolation between two positions sharing one collateral vault;
- randomized repayment caps from zero through the full debt, checking debt, collateral, vault, and LTV invariants.

Four transaction-level tests now cover, once the configured testnet works:

- actual collateral leaving the vault and supply/collateral returning during partial settlement;
- healthy oracle recovery with no debt repayment and full collateral return;
- insolvency after the sweep, full debt repayment, and zero collateral return;
- curator-only authorization, pending-operation locking, slippage failure, and state/token rollback.

## Verification evidence

- `autara-lib`: 313 passed, 0 failed, 0 ignored.
- `autara-program`: 111 passed, 0 failed, 0 ignored.
- Integration suite: compiled successfully, including all four new sweep tests.
- Live sweep integration run: reached `begin_capital_sweep`, then the older deployed program rejected the unknown instruction with `InvalidInstructionData`.
- Formatting and diff whitespace checks passed for every changed Rust file.

## Findings and risks

### 1. Pending positions are misreported by read clients — medium correctness risk

Beginning a sweep intentionally sets on-chain deposited collateral to zero while debt shares remain non-zero. The normal health function then attempts to divide debt value by zero collateral value and returns an error. Direct health queries therefore fail while a sweep is pending.

More seriously, aggregated user-position reads catch that error with `unwrap_or_default()`. They can display zero LTV, zero debt, and zero collateral even though debt is still accruing and swept collateral is recorded in the position. This can make monitoring, APIs, and operator dashboards incorrectly show that the position has disappeared or is harmless.

This does not corrupt settlement math: settlement deliberately uses `swept_collateral_atoms` as its reference amount. The public read path should not count the off-chain collateral as ordinary on-chain collateral. Instead, it should expose an explicit `capital_sweep_pending` state with current debt, zero on-chain collateral, the separately labelled swept amount, and either no ordinary LTV or a clearly defined sentinel. Aggregators must not replace the failed calculation with an all-zero health record.

### 2. Sweep events are emitted before token transfers — medium observability risk

Both begin and settle mutate model state and emit their success event before performing token transfers. Transaction atomicity should roll state and balances back if a later transfer fails. However, the client includes parsed inner events in `AutaraTxError`, so a consumer that records events without also enforcing transaction success can observe a `CapitalSweepStarted` or `CapitalSweepSettled` event for an operation that did not commit.

Emit success events after all transfers, and defensively ensure indexers and clients discard state-transition events from failed transactions.

### 3. No on-chain recovery path exists if the curator cannot settle — high-impact accepted trust risk

While a sweep is pending, repay, collateral deposit/withdrawal, borrow, liquidation, and loss socialization are all locked. Debt continues to accrue. There is no timeout, emergency return, alternate settlement authority, or key-rotation path tied to the pending sweep.

This matches the trusted-curator design, but key loss, downtime, compromise, or refusal to act can strand the position indefinitely and prevent normal liquidators from protecting suppliers. If the protocol deliberately accepts this, it needs an operational key-recovery and incident-response policy. Otherwise, an explicit emergency mechanism is required.

### 4. The begin instruction accepts any same-mint destination token account — low operational risk

The program verifies the destination mint but does not verify that the destination is the curator's canonical associated token account, that its token owner is the curator, or that it differs from the market vault. The standard client supplies the canonical curator account, and only the trusted curator can authorize the instruction, so this is not an external privilege bypass. A custom transaction or curator configuration mistake can nevertheless route swept collateral to an unintended or unrecoverable account while the position records it as swept.

Validate the expected associated token address and reject the market vault as the curator destination.

### 5. A partially settled insolvent position cannot start another off-chain sweep — documented limitation

If the position becomes insolvent after begin and the curator performs only a capped partial settlement, the sweep closes and returns the remaining collateral on-chain. A second sweep is rejected because begin only accepts LTV below 100%. The tests confirm that ordinary liquidation is unlocked and can continue, so the position is not stuck; it simply cannot use repeated off-chain sweep rounds after insolvency.

### 6. Off-chain execution remains unverifiable — inherent design risk

The chain cannot verify sale price, execution venue, custody, or whether proceeds exist before settlement. A curator may also settle with a zero repayment cap and return all collateral, effectively cancelling the sweep after debt has continued to accrue. These are consequences of the explicitly trusted-curator model, not arithmetic bugs.

## Release gate recommendation

Before production rollout:

1. Fix the pending-position read representation so debt can never be silently displayed as zero.
2. Fix event ordering or enforce failed-transaction event filtering in every consumer.
3. Deploy the candidate program to an isolated test environment and run all four sweep transaction tests against that exact artifact.
4. Decide explicitly whether curator-loss recovery and canonical destination validation are protocol requirements or accepted operational risks.
