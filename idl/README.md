# Autara Lending IDL — Path A import (explorer decoding, no redeploy)

Artifacts for getting Autara instructions decoded by the Arch Explorer's
IDL-driven decoder (`arch-rust-indexer` PR #66 / `feat/idl-decoding*`) **without
any contract change** — see `docs/idl-decoding-plan.md` for the full rationale.

## Files

- **`autara_lending.idl.json`** — the canonical IDL. Authored directly from source:
  - Discriminators = `AurataInstructionTag` declaration-order indices
    (`autara-lib/src/ixs/types.rs`), double-confirmed against the enum's
    `TryFrom<u8>` impl and the indexer PR #59 tag table (exact match).
  - Args = the Borsh payload structs (`autara-lib/src/ixs/{supply,borrow,liquidation}.rs`),
    incl. the `Option<Instruction>` callback (types defined per arch_program 0.6.2
    derived-Borsh layout: `Vec` = u32-LE-prefixed, `Option` = 1-byte tag).
  - Accounts = `from_accounts` consumption order
    (`programs/autara-program/src/ixs/*.rs`) + the trailing `autara_program`
    account the client builders append.
  - Covers **16 of 20** instructions. Deliberately omitted (complex nested config
    payloads, decode falls through to the program display name):
    `create_market` (0), `update_config` (9), `create_global_config` (12),
    `update_global_config` (15). Add later if needed.
- **`autara-idl-import.json`** — generated wrapper in the
  `import_program_verification` artifact format (program_id in **hex**, IDL
  embedded, `is_active: true`). Regenerate after editing the IDL:

  ```bash
  python3 - <<'EOF'
  import json
  idl = json.load(open('autara_lending.idl.json'))
  json.dump({"program_id": "53def2dc8516302842b10e356914d2a5f6b33425ba42aec684f706aa1cf64192",
             "display_name": "Autara Lending",
             "idl": {"schema_format": "anchor-idl-compatible", "schema_version": "0.1.0",
                      "is_active": True, "idl_json": idl}},
            open('autara-idl-import.json','w'), indent=2)
  EOF
  ```

## Import runbook

Prereq: the IDL-decoding work (PR #66 / `feat/idl-decoding*`) is merged + deployed
on the target indexer. Until then nothing decodes from any IDL.

```bash
# from arch-rust-indexer/arch-indexer-microservices
cargo run -p api-server --bin import_program_verification -- \
  --file /path/to/autara-lending-v1/idl/autara-idl-import.json \
  --activate-idl \
  --database-url "$INDEXER_DATABASE_URL"   # the TESTNET network DB
```

Notes:
- `program_id` must stay **hex** (`53def2dc…`) — the decoder's registry
  (`idl_registry.rs::build_idl_registry`) looks IDLs up by hex program id.
- `--activate-idl` deactivates any previously active IDL for the program and
  activates this one (idempotent upsert on `(program_id, idl_hash)`).
- Networks have separate DBs — import into the testnet DB (where Autara is
  deployed).

## Verify

1. `GET /api/v1/testnet/programs/{id}/idl` returns the imported IDL.
2. Open a known Autara transaction in the explorer (any supply/borrow/repay tx) —
   instructions should show named actions + decoded args instead of
   `Custom Instruction`.
3. Compound flows: `liquidate` / `borrow_deposit_apl` / `withdraw_repay_apl` may
   carry extra trailing accounts beyond the IDL list (the optional callback's
   accounts) — expected; the decoder maps the named fixed set by index.

## Local testing (building on PR #66)

How decoding is wired (from the PR): enrichment is **request-time** in the
api-server — `enrich_tree_nodes_with_idls` runs on every `/transactions/{txid}/tree`
(and instruction-list) response and applies `decode_with_idl` to nodes the builtin
decoders left **undecoded**. So after importing the IDL you just re-request a tx;
**no re-indexing needed**.

### Rule 0 — build WITHOUT PR #59 in the tree

Per the PR's own design doc (`docs/idl-driven-decoding-and-generic-enrichment.md`):
a program registered in `get_program_name` is **excluded from IDL candidacy** —
builtin path and IDL path are mutually exclusive. PR #59 both registers the name
("Arch Lending Program") and adds `decode_arch_lending`, either of which prevents
the IDL path from ever running for Autara. **Test on `origin/feat/idl-decoding`
as-is** (it does not contain #59):

```bash
git checkout origin/feat/idl-decoding   # confirm this maps to PR #66
```

### Layer 1 — decoder unit test (seconds, no DB/chain)

Copy the IDL into the decoder's testdata and assert it against the real decoder:

```bash
cp autara_lending.idl.json \
  ../arch-rust-indexer/arch-indexer-microservices/shared/src/idl/testdata/
```

Add to `shared/src/idl/decoder.rs` tests (mirrors `decodes_real_initialize_pool`):

```rust
#[test]
fn decodes_autara_supply_apl() {
    let idl = Idl::from_json(include_bytes!("testdata/autara_lending.idl.json")).unwrap();
    let mut data = vec![2u8];                       // AurataInstructionTag::SupplyApl
    data.extend_from_slice(&100_000u64.to_le_bytes());
    let (action, decoded) = decode_with_idl(&idl, &data, &[]).expect("disc [2] matches");
    assert!(action.unwrap().contains("supply_apl"));
    assert_eq!(decoded.unwrap()["args"]["amount"], 100_000);
}

#[test]
fn decodes_autara_withdraw_repay_no_callback() {
    let idl = Idl::from_json(include_bytes!("testdata/autara_lending.idl.json")).unwrap();
    let mut data = vec![17u8];                      // WithdrawRepayApl
    data.extend_from_slice(&5_000u64.to_le_bytes()); // repay_amount
    data.extend_from_slice(&7_000u64.to_le_bytes()); // withdraw_amount
    data.extend_from_slice(&[1, 0, 0]);              // repay_all, withdraw_all, ix_callback = None
    let (_, decoded) = decode_with_idl(&idl, &data, &[]).expect("disc [17] matches");
    let args = &decoded.unwrap()["args"];
    assert_eq!(args["repay_all"], true);
    assert_eq!(args["ix_callback"], serde_json::Value::Null);
}

#[test]
fn autara_unlisted_tag_returns_none() {
    let idl = Idl::from_json(include_bytes!("testdata/autara_lending.idl.json")).unwrap();
    assert!(decode_with_idl(&idl, &[0u8, 1, 2, 3], &[]).is_none()); // create_market not in IDL
}
```

```bash
cargo test -p arch-indexer-shared idl
```

If a discriminator matches but args fail to Borsh-decode, the decoder returns
`(action, None)` — seeing a label with null `decoded` means the IDL's arg types
are wrong, not the discriminator.

### Layer 2 — local stack end-to-end

Same setup you used to test PR #59 (postgres + db-init + indexer + api-server),
just on the #66 branch, with Autara txs in the DB (either your local-validator
test txs or a testnet sync covering known supply/borrow txs).

1. Import the IDL into the **local** DB (see runbook above, `--database-url` =
   the local api-server's DB).
2. `GET /api/v1/programs/53def2dc…/idl` → the imported IDL, active.
3. `GET /api/v1/transactions/{autara_txid}/tree` (or the tx page in the local
   explorer UI) → named actions + decoded args + positional account labels,
   instead of `Custom Instruction`. Works immediately — request-time enrichment.
4. Negative: a `create_market` tx (tag 0, unlisted) renders the program label,
   no decode, no error.
5. Display name: with `feat/idl-program-display-names` included, the program
   shows as **Autara Lending** (`metadata.name`).
6. Optional — exercise the **on-chain fetch** path with a program that already
   has a published IDL: the PR's test pins arch-bitcoin-defi `amm`
   (`4443bfc2…c698`, IDL account `2209537b…fe16`, testnet). With an empty
   `program_idls`, request a tx touching it and confirm the registry lazily
   fetches + persists the IDL.

### Merge-interaction watchpoints

- **PR #62 (yours):** it serves the *saved* `instructions_tree`; #66 enriches
  after load in `get_transaction_tree_from_pool`. Compatible in principle, but
  verify the enrichment still runs on the saved-tree path when both merge.
- **PR #59:** per the design doc, the IDL path replaces it — to switch Autara to
  IDL decoding in production, #59's `get_program_name` registration and
  `decode_arch_lending` must be **removed**, not just left unused.

## Follow-ups

- Cross-check against Brian's hand-authored IDL when he shares it; reconcile any
  naming differences (discriminators/args/accounts here are source-verified).
- Once verified end-to-end, retire PR #59's builtin entirely — both
  `decode_arch_lending` (`shared/src/instruction_decoder.rs`) **and** the
  `get_program_name` / `get_program_name_from_hex` registrations
  (`shared/src/program_ids.rs`). This is required, not optional: builtin
  registration excludes the program from IDL candidacy by design.
- On-chain publishing (canonical IDL, requires adding a native IDL instruction +
  redeploy) remains optional Path B — see `docs/idl-decoding-plan.md` §4.
