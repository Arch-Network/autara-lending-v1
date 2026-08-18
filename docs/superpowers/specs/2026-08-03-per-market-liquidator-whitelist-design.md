# Per-Market Liquidator Whitelist Design

## Goal

Allow each lending market to restrict ordinary public liquidation to multiple curator-approved liquidators, while preserving permissionless liquidation by default. The market curator can add, remove, and later reactivate liquidators without changing the market address or reallocating the market account.

Capital-sweep liquidation is outside this whitelist because it is already authorized directly by the market curator.

## Permission model

Each market stores an active whitelist-entry count in reserved `MarketConfig` bytes:

```rust
active_whitelisted_liquidators: u64
```

The count determines the market mode:

- `0`: ordinary liquidation is permissionless and requires no whitelist proof;
- greater than `0`: ordinary liquidation is restricted to active whitelist entries.

New and existing markets read the reserved bytes as zero and therefore remain permissionless. Removing the final active entry returns the market to permissionless mode automatically.

Only the market curator may add or remove entries. Adding and removing use checked count arithmetic and reject an already-active add or already-inactive remove, preventing the market count from drifting from entry state.

## Whitelist entry account

Each `(market, liquidator)` pair has a dedicated program-owned PDA derived from:

```text
["liquidator_whitelist", market_pubkey, liquidator_pubkey]
```

The fixed-size account records:

```rust
LiquidatorWhitelistEntry {
    market: Pubkey,
    liquidator: Pubkey,
    active: bool,
    // reserved padding for alignment and future compatibility
}
```

The market and liquidator fields intentionally duplicate the PDA seeds so validation and account inspection do not rely only on the supplied address.

There is no fixed maximum number of liquidators because entries are independent accounts. A client can enumerate a market's entries by querying program accounts filtered by the stored market pubkey; liquidation authorization itself never scans entries.

## Adding a liquidator

`AddWhitelistedLiquidator` receives the market, curator signer, target liquidator pubkey, whitelist-entry PDA, system program, and any account required to fund first-time creation.

The program:

1. verifies the signer is the market curator;
2. derives and verifies the exact PDA for the market and target liquidator;
3. creates and initializes the PDA when it does not yet exist, or loads it when it is an inactive tombstone;
4. rejects an already-active entry;
5. marks the entry active;
6. increments `active_whitelisted_liquidators` using checked arithmetic;
7. emits a `LiquidatorWhitelistEntryAdded` event.

The curator funds first-time entry creation. The target liquidator does not need to sign.

## Removing a liquidator

`RemoveWhitelistedLiquidator` receives the market, curator signer, target liquidator pubkey, and whitelist-entry PDA.

The program verifies the curator, PDA, program ownership, stored market, stored liquidator, and active state. It then marks the entry inactive, decrements the market count using checked arithmetic, and emits `LiquidatorWhitelistEntryRemoved`.

The PDA remains allocated as an inactive tombstone rather than being closed. This avoids account-close and rent-recipient complexity and permits later reactivation. An inactive entry never authorizes liquidation.

## Liquidation authorization

The ordinary `Liquidate` instruction continues its existing signer and account validation, then applies this rule before any liquidation calculation or mutation:

1. If `active_whitelisted_liquidators == 0`, proceed permissionlessly and do not require a whitelist account.
2. Otherwise, require the whitelist PDA associated with the market and the actual liquidator signer.
3. Derive the expected PDA from the market and signer pubkeys.
4. Verify the supplied account address, program ownership, account type, stored market, stored liquidator, and `active == true`.
5. Reject the instruction before token movement or state mutation when any check fails.

The transaction cannot claim authorization for another liquidator because the expected PDA is derived from the transaction's signer. A valid entry from another market is also rejected.

The optional proof account is inserted before the program/callback account suffix. Permissionless markets keep the existing account order, preserving compatibility with existing liquidation transactions. Once a market becomes restricted, liquidation clients must include the derived proof account.

## Client behavior

The client exposes curator methods to add and remove a liquidator. Its high-level liquidation builder reads the market configuration:

- for a permissionless market, it builds the existing instruction shape;
- for a restricted market, it derives and includes the signer's whitelist-entry PDA.

Low-level instruction builders expose the optional proof account explicitly so callers can construct transactions without hidden network reads.

## Events and errors

New events record the market, affected liquidator, curator, and resulting active-entry count. Dedicated errors distinguish unauthorized curator access, invalid whitelist PDA/account data, duplicate addition, inactive removal, missing proof, and a non-whitelisted liquidator.

Instruction, event, account-type, and error discriminants are appended rather than reordered.

## Compatibility and invariants

- `MarketConfig` keeps its existing fixed size by consuming reserved padding.
- Existing markets remain permissionless because reserved bytes are zero.
- The active count changes exactly once with each active-state transition.
- Restricted liquidation authorization is checked before calculations, transfers, callbacks, or state mutation.
- Removing the final entry immediately restores permissionless liquidation.
- Curator capital sweep remains curator-only and does not consult this whitelist.

Tests cover default permissionless behavior, first-time creation, multiple active entries, duplicate addition, unauthorized add/remove, removal, reactivation, last-removal permissionless fallback, count arithmetic, member liquidation, non-member rejection, wrong-market and wrong-liquidator PDAs, inactive entries, callback account ordering, serialization, and fixed account sizes.
