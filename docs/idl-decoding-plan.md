# Autara IDL Decoding — Architecture & Options

Goal: get Autara lending instructions decoded in the Arch Explorer via the new
IDL-driven decoder (PR #66 / `feat/idl-decoding*`), replacing the hand-coded
builtin from indexer PR #59.

This doc is grounded in the actual code of `autara-lending-v1` and
`arch-rust-indexer` (paths cited). **Headline: a full contract rebuild + redeploy
is almost certainly NOT required.** The premise that we must "redeploy with IDL
handlers" comes from assuming Autara is a Satellite `#[program]`. It isn't — and the
indexer's decoder reads IDLs from its DB first, so we can decode Autara with zero
contract changes.

---

## 1. What Autara actually is (corrects the premise)

Autara is a **fully native Arch program**, not a Satellite/Anchor `#[program]`:

- `programs/autara-program/src/lib.rs:40` uses the raw `arch_program::entrypoint!`
  macro, not `#[program]`.
- `programs/autara-program/Cargo.toml` depends on `arch_program`, `apl-token`,
  `borsh`, `num_enum`, etc. — **there is no `arch-satellite-lang` dependency.**
- Dispatch (`lib.rs:57`) is `<Box<AurataInstruction>>::deserialize(data)` followed
  by `match`. The "1-byte tag" is simply the **Borsh enum discriminant** of
  `AurataInstruction` (variant index 0..N as the first byte). `lib.rs:57` —
  `.map_err(|_| ProgramError::InvalidInstructionData)` — is the exact line the
  publish attempt failed on.

**Consequence:** there are **no Satellite IDL dispatch handlers to "un-strip."**
The `#[program]` macro that generates `__idl_dispatch` (and the `no-idl`/`idl-build`
features) was never in play here. So "just make sure the handlers aren't stripped"
does not apply to Autara. Getting an IDL *published on-chain* would require
*adding* IDL-instruction support to a native program (Path B below) — a real change,
not a flag flip.

Good news that makes this mostly moot: **on-chain publishing isn't the only way to
feed the decoder.**

---

## 2. How the explorer's decoder resolves an IDL (the key finding)

`api-server/src/api/idl_registry.rs::build_idl_registry` resolves IDLs in two
steps, **DB first**:

```sql
-- step 1: cached/imported IDLs
SELECT program_id, idl_json FROM program_idls
WHERE program_id = ANY($hexes) AND is_active = true
```

```rust
// step 2 (only for programs still missing, and only if an RPC client is passed):
fetch_idl_from_chain(rpc, bytes)   // derive anchor:idl PDA, inflate, persist
```

So **any active row in `program_idls` is used to decode** — the decoder does not
require the IDL to be on-chain. The generic decoder (`shared/src/idl/decoder.rs`)
then matches each instruction by the IDL's variable-length `discriminator`
(`find_instruction`: `&data[start..start+disc.len()] == disc`), which already
**honors 1-byte discriminators**. Autara's tags live at offset 0, so even the base
decoder (no `discriminatorOffset`) handles them.

This gives us two real paths.

---

## 3. Path A — Import the IDL into the indexer (RECOMMENDED, no redeploy)

Insert the hand-authored Autara IDL into `program_idls` as an active row. The
decoder picks it up immediately. **No contract change, no redeploy, no risk to the
lending program.**

### Mechanism

The existing operator tool `api-server/src/bin/import_program_verification.rs`
imports an IDL from a JSON file into `program_idls` (`upsert_idl`, with
`--activate-idl` flipping prior IDLs inactive and the new one active).

### Steps

1. Wrap the IDL JSON in the import artifact shape:

   ```json
   {
     "program_id": "53def2dc8516302842b10e356914d2a5f6b33425ba42aec684f706aa1cf64192",
     "display_name": "Autara Lending",
     "idl": {
       "schema_format": "anchor-idl-compatible",
       "schema_version": "0.1.0",
       "is_active": true,
       "idl_json": { /* the hand-authored Autara IDL, 9 instructions, 1-byte discriminators */ }
     }
   }
   ```

   - `program_id` must be the **hex** form (`53def2dc…`) — the registry queries by
     hex (it converts the base58 from the tx tree to hex at the boundary).
   - `idl_json` must deserialize into the indexer's `Idl` struct
     (`shared/src/idl/spec.rs`): `address`, `metadata{name,version}`,
     `instructions[{name, discriminator: [u8], discriminator_offset?, accounts, args}]`,
     `types[]`. The discriminators must equal the `AurataInstruction` Borsh variant
     indices (source of truth: the enum in `autara-lib/src/ixs/types.rs`).

2. Run the importer against the indexer DB:

   ```bash
   import_program_verification --file autara-idl-import.json --activate-idl \
     --database-url "$INDEXER_DATABASE_URL"
   ```

3. Done — transactions touching Autara now decode via the IDL. PR #59's hardcoded
   builtin can be removed once this is verified (the IDL path supersedes it).

### Tradeoffs

- ✅ Zero contract risk, fastest, fully achieves IDL-driven decoding.
- ✅ Trivially reversible (`is_active = false`).
- ⚠️ The IDL lives in **your indexer's DB**, not canonically on-chain. Your explorer
  decodes Autara; a third-party explorer instance wouldn't (until the on-chain IDL
  exists). For a first-party testnet program decoded by our own explorer, this is
  the right call.
- Dependency (shared with Path B): PR #66 / the `feat/idl-decoding*` work must be
  **merged and deployed** to the explorer instance. Until then nothing decodes from
  any IDL, DB-imported or on-chain.

---

## 4. Path B — Publish the IDL on-chain (canonical, but requires a redeploy)

Only needed if we want the IDL discoverable on-chain by *any* explorer/tool (the
"Arch-native blessed flow"), independent of our indexer's DB. Because Autara is
native, this is a genuine contract change, not a feature toggle. Two sub-options:

### B1 — Add a native IDL instruction to Autara (recommended for Path B)

Keep the native program, the Borsh-enum 1-byte scheme, and the program ID. Add one
isolated instruction that implements the on-chain IDL-account protocol so
`arch-cli idl init` can publish.

- **Dispatch change** (`lib.rs`): intercept the IDL instruction *before* the Borsh
  enum decode:

  ```rust
  pub fn autara_process_instruction(...) -> LendingProgramResult {
      if is_idl_instruction(instruction_data) {           // matches the arch-cli IDL selector
          return process_idl_instruction(program_id, accounts, instruction_data);
      }
      let ix = <Box<AurataInstruction>>::deserialize(&mut &instruction_data[..])
          .map_err(|_| ProgramError::InvalidInstructionData)?;
      match &*ix { /* unchanged */ }
  }
  ```

  No collision risk: `AurataInstruction` discriminants are 0..N (single low byte);
  the IDL selector is a distinct sentinel, so existing instructions are unaffected.

- **IDL handler** (`process_idl_instruction`): implement create / write / resize /
  set-authority on the IDL account, where the account is
  `create_with_seed(find_program_address([], program_id).0, "anchor:idl", program_id)`
  with the 44-byte header (8-byte disc + 32-byte authority + 4-byte len) + zlib
  payload. Lift this from `arch-satellite-lang`'s IDL instruction module rather than
  hand-rolling — it's a well-defined protocol. **Source the exact selector + sub-op
  encoding from arch-network PR #2341** (the `arch-cli idl` flow) so the program and
  CLI agree; that PR is unreleased, so pin to its branch.

- **Build / deploy / publish:**
  1. Implement + unit-test the IDL instruction (create/write/set-authority; reject
     writes to anything but the derived IDL PDA; authority checks for upgrades).
  2. Rebuild the program; deploy to the **same program ID** (testnet).
  3. `arch-cli -p testnet idl init 6eQ1vLSAwmbT6SD3KQbNawAqis7LpzwpNTd7SJ1GU5cm \
     --filepath autara_lending.idl.json --authority <keypair.json>` (build arch-cli
     from PR #2341).
  4. Verify with `arch-cli idl fetch` and confirm the explorer decodes.

- **Risk:** the IDL instruction never touches lending state or fund flows, but it
  *is* a change to a fund-holding program and a redeploy is itself the risk event.
  Mitigations: isolation (no shared state), thorough tests, testnet-only for now,
  same program ID (no client migration, no discriminator change → existing
  integrations and the 9 instruction tags are untouched).

### B2 — Port Autara to `arch-satellite-lang` `#[program]` (NOT recommended)

Gets `__idl_dispatch` "for free," but Satellite's macro emits **8-byte** Anchor
discriminators by default, which **changes the on-wire instruction format** — breaking
every existing client and the byte-for-byte 1-byte IDL. That's a full rewrite +
re-audit + client migration of a live lending protocol purely for explorer
decoding. Avoid.

---

## 5. Recommendation

1. **Now:** Path A. Import the hand-authored IDL into `program_idls` (active). It
   fully achieves IDL-driven decoding for our explorer with zero contract risk, and
   lets us retire PR #59's builtin. Gate only on PR #66 being merged + deployed.
2. **Later (optional):** Path B1, if/when we want the Autara IDL canonical on-chain
   for third-party tooling. Treat it as a small, isolated, well-tested instruction
   addition + testnet redeploy at the same program ID — not a Satellite port.

| | Path A (DB import) | Path B1 (native IDL ix + redeploy) | B2 (Satellite port) |
|---|---|---|---|
| Contract change | none | small, isolated | full rewrite |
| Redeploy a lending program | no | yes (same id) | yes (new discriminators) |
| Breaks existing clients | no | no | yes |
| Canonical on-chain IDL | no | yes | yes |
| Risk | minimal | moderate | high |
| Effort | hours | days + review | weeks + re-audit |

---

## 6. Open items / dependencies

1. **PR #66 merge + deploy status** — neither path decodes anything until the
   `feat/idl-decoding*` work is live on the target explorer. Confirm with the
   indexer team.
2. **IDL JSON conformance** — verify the hand-authored IDL deserializes into the
   indexer's `Idl` struct (`shared/src/idl/spec.rs`), and that its discriminators
   match the `AurataInstruction` Borsh order in `autara-lib/src/ixs/types.rs`. A
   quick way: run the indexer's `idl_decoding` test path against the IDL + a known
   Autara tx.
3. **Path B only** — the exact `arch-cli idl` selector + IDL-account sub-op encoding
   from arch-network PR #2341 (unreleased; build from branch). Needed so the on-chain
   handler and the CLI agree.
4. **IDL authority (Path B)** — whoever runs `idl init` becomes the authority for
   future upgrades; publish from a team-controlled keypair.
