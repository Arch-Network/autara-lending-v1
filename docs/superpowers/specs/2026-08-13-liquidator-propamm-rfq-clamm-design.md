# Liquidator PropAMM RFQ and CLAMM Testnet Update

**Status:** Approved design

**Date:** 2026-08-13

## Objective

Update the liquidator bot on `liquidator-deploy` for the current Autara lending,
PropAMM, and CLAMM testnet deployments. The bot must obtain PropAMM quotes through
the public RFQ service and must never load or possess the PropAMM quote-signer
secret. CLAMM must remain available as the atomic callback route.

The lending liquidation permission model is unchanged. There is no liquidator
allowlist, no whitelist transaction to send, and no new authorization account to
check.

## Confirmed Deployment State

The three systems use the same current testnet asset pair:

- aBTC: `1d46e0dd87393236e4e01252439f46dcbaec7c2255d1fd734e61771a00e8f4e9`
  with 8 decimals.
- aUSD: `55c6cee38a31732e2dad821ab1c38f902a7c51efaefb3641d51f3485c4617a45`
  with 6 decimals.

The PropAMM testnet service is available at
`https://propamm.arch.network/testnet`. Its observed deployment on 2026-08-13
reported:

- build `af65871`;
- program `7a68831501d3a9806feff162e82815a36e1732964a2edd2b461faf69575c3628`;
- config `265c3b556ab8baf2406d0cb911d889874f5bb5f734f292c05de0cb89c0e2f12e`;
- a maximum RFQ lifetime of 30 seconds.

The current CLAMM deployment is:

- program `0a0129c4d864d1728c4b6e8f6e0e473197cb111153e379a59b9d93c038efe918`;
- config `5dcbc567a5434cc84303079bfb54be234993e50962a91fda63a17ba8026c8fd0`;
- aBTC/aUSD pool `06db06761eb1f114167ea2bbc4cf98cf8f98fbfc0ad18d1821e724cfeeb03461`.

At 2026-08-13 12:27 JST, the pool had active liquidity `95,489,467,848`,
`79.38557647` aBTC in vault A, and `1,271,038.31156` aUSD in vault B. These
values are a readiness snapshot, not configuration constants; the bot must quote
against current state on every opportunity.

## Scope

This change includes:

1. Rebasing the liquidator work onto current lending `main`.
2. Replacing local PropAMM signing and pricing with the two-step RFQ API.
3. Updating CLAMM routing for the current program, pool, and SDK.
4. Isolating incompatible Arch dependency versions.
5. Updating testnet configuration, validation, logging, and tests.

This change does not include:

- a lending program modification or liquidator allowlist;
- a PropAMM server or program modification;
- CLAMM liquidity provisioning;
- making PropAMM execution atomic with lending liquidation;
- a workspace-wide Arch SDK migration.

## Branch Reconciliation

The `liquidator-deploy` branch diverged substantially from `main`: it contains 14
liquidator commits while `main` contains 54 commits not present at the branch
tip. Reconciliation must be an interactive rebase onto current `origin/main`,
not a merge commit and not an unreviewed replay of every intermediate commit.

During the rebase:

- retain the final liquidator service, scanner, CLAMM router, deployment config,
  and operational fixes;
- squash or drop superseded WIP/deadlock commits;
- resolve shared lending client, transaction-builder, Pyth layout, ATA, and
  dependency files in favor of the current `main` behavior;
- regenerate `Cargo.lock` only after the version-isolated venue dependencies are
  settled;
- keep the design document in the rebased history.

The result must build and test as a normal lending workspace before venue logic
is changed. This separates rebase failures from RFQ or CLAMM failures.

## Architecture

### Scanner and route selection

The scanner remains responsible for finding unhealthy positions and previewing
the amount of collateral that liquidation will seize. It asks both venue clients
for exact-input quotes and compares executable output amounts in debt-token
atoms.

Venue-specific SDK and HTTP types must not escape their modules. The scanner
consumes small bot-native values:

- input mint and amount;
- output mint and estimated amount;
- venue identifier;
- a venue-owned execution handle.

If one venue is unavailable, malformed, unsupported, or cannot quote the amount,
the scanner may use the other venue. If neither venue returns a valid quote, it
must skip the position without broadcasting a liquidation.

### PropAMM RFQ client

`autara-liquidator/src/propamm.rs` becomes a typed HTTP client. It owns one reused
`reqwest::Client`, the base URL, timeouts, slippage, health/market metadata, and
RFQ response validation. It does not construct an `ExecuteTrade` instruction and
does not submit a PropAMM transaction directly to Arch RPC.

Configuration retains only public service and safety information:

```json
{
  "base_url": "https://propamm.arch.network/testnet",
  "expected_program_id": "7a68831501d3a9806feff162e82815a36e1732964a2edd2b461faf69575c3628",
  "slippage_bps": 100,
  "request_timeout_ms": 8000
}
```

The old `quote_signer_keypair`, PropAMM config account, vault addresses, decimal
copies, local price calculation, replicated quote/instruction definitions, and
direct broadcast path are removed. The quote-signer public identifier obtained
from service metadata is used to validate the required signer set; no
quote-signer secret or keypair path is permitted in bot configuration.

### CLAMM adapter

The CLAMM adapter owns pool discovery, exact-input quote construction, selection
of the best initialized pool, and conversion of the selected swap instruction
into the bot's lending-compatible callback representation.

The current high-level CLAMM SDK uses exact APL and Arch `0.6.8` packages while
lending `main` intentionally remains on exact `0.6.2`. Cargo cannot include the
two APL patch releases in one dependency graph: their semver ranges are
compatible, so Cargo must unify them, but their exact constraints conflict. A
minimal standalone Cargo reproduction confirmed this behavior.

The bot therefore must not depend on the high-level `orca_whirlpools` crate and
must not force a workspace-wide upgrade. Its CLAMM adapter uses lending's
`0.6.2` RPC and program types, decodes only the current Whirlpool and TickArray
wire fields it needs, and uses the version-independent `whirlpool-core` quote
engine. It derives the current CLAMM PDAs and serializes the current SwapV2 wire
instruction locally. These wire definitions are frozen by fixture tests against
the current CLAMM generated client. The adapter verifies that the callback
targets the expected CLAMM program and pool before handing it to the scanner.

The initial implementation retains the existing operational requirement that,
for each CLAMM opportunity, the liquidator's input-token ATA has standing
balance at least equal to the quoted input. The SDK currently ignores its
`quote_only` argument and performs this input-balance check even though the
atomic liquidation supplies that input before the callback executes. Removing
this artificial float requirement is a separate CLAMM SDK improvement, not part
of this update.

Only the swap instruction may be embedded as the lending callback. If the SDK
returns ATA setup or cleanup instructions, the adapter must either prove the
required ATAs already exist and select exactly the swap instruction, or reject
the quote. It must never silently use the first instruction in an arbitrary
instruction list.

### Version isolation

The binary may contain multiple semver-incompatible Arch dependency versions:

- lending-native `0.6.2` types for the scanner and lending transactions;
- PropAMM-compatible `0.7.0` transaction types or explicit JSON wire DTOs inside
  the RFQ client.

The CLAMM adapter remains on lending-native `0.6.2` and depends only on the
Arch-independent `whirlpool-core` math crate from the current CLAMM checkout.
PropAMM dependencies use clear Cargo aliases where direct access is needed. No
public function in the PropAMM module may expose `0.7.0` Arch types to the
scanner. Conversion is explicit by fixed-size byte arrays and serialized
instruction/message data; unsafe transmutation is prohibited.

## Execution Flows

### CLAMM atomic route

1. Preview the liquidation and derive the expected seized collateral amount.
2. Request a CLAMM exact-input quote for collateral to debt token.
3. Validate the pool, callback program, accounts, amount, and slippage threshold.
4. Embed the single CLAMM swap instruction as the lending liquidation callback.
5. Sign and broadcast one lending transaction.
6. Confirm it as one atomic liquidation-and-swap operation.

If the callback or liquidation fails, the whole transaction fails and the bot
retains its pre-transaction balances.

### PropAMM RFQ route

1. Send `POST /rfq/quote` with the base and quote mints, side, exact-input amount,
   liquidator public key, and slippage basis points.
   Use Sell when the seized collateral is the market base mint and Buy when the
   seized collateral is the market quote mint.
2. Use `estimated_quote.quote_amount` for a Sell payout and
   `estimated_quote.base_amount` for a Buy payout when comparing venues.
3. Validate the unsigned transaction before considering the quote executable.
4. If PropAMM wins, record the collateral ATA balance and submit the lending
   liquidation without a callback.
5. After confirmation, reload the ATA and use its positive balance delta as the
   actual seized amount. Do not consume unrelated standing inventory.
6. Request a fresh RFQ for that exact delta. The first quote is only a routing
   estimate and must not be reused after waiting for liquidation confirmation.
7. Reject the refreshed quote if its effective output rate has moved below the
   first quote by more than the configured slippage tolerance. Because the
   preview and actual seized amounts may differ, compare rates with integer
   cross-multiplication rather than comparing raw output amounts.
8. Sign the validated message hash with the liquidator key using BIP322, attach
   exactly the single user signature, and send the transaction to
   `POST /rfq/swap`.
9. Let the server recheck price and vault skew, add its quote-signer signature,
   broadcast, and return `transaction_hash`.
10. Wait for that hash to reach processed status through the lending-native RPC
    client.

The PropAMM route is intentionally non-atomic. Once lending liquidation has
landed, an RFQ failure leaves the seized collateral in the liquidator wallet.
The bot must log and alert this inventory state and must not start another
liquidation for the same position or automatically attempt a CLAMM fallback.

## RFQ Transaction Validation

The PropAMM server is an external transaction builder, so the bot must validate
every returned unsigned transaction before signing. At minimum it must verify:

- transaction version is `0` and the signature vector is empty;
- the message hash and message are internally consistent;
- the liquidator is a required signer and the expected fee payer;
- the other required signer matches the quote-signer public key advertised by
  the healthy service;
- the trade instruction targets the configured PropAMM program;
- base mint, quote mint, user, side, amount, and minimum output match the request
  and returned estimate;
- all writable accounts are expected market, vault, and user token accounts;
- no unrelated system transfer or additional executable instruction is present;
- expiry leaves enough time to sign and submit.

Any validation failure marks PropAMM unavailable for that opportunity and emits
a structured security error. The bot must never sign a partially signed,
unexpected, or opaque transaction.

## Error Handling and Recovery

RFQ quote timeouts, unsupported markets, stale health data, and server `5xx`
responses make PropAMM unavailable during route selection. They do not stop the
scanner when CLAMM remains viable.

For RFQ submission:

- `rfq_swap_in_progress` is retried with the identical signed transaction after
  a short bounded delay;
- a repeated successfully submitted RFQ may return the cached transaction hash
  and is treated as success;
- expired, missing, message-mismatch, invalid-signature, price-tolerance, or
  minimum-output errors are terminal for that RFQ;
- transport or gateway failures may retry the identical signed request only
  while the quote is still valid;
- an unknown confirmation result is reconciled by transaction hash and token
  balances before any further action.

CLAMM discovery and quote failures are scoped to the affected pool. A pool with
zero active liquidity or zero relevant vault balance is ignored until a later
refresh. Static pool configuration remains supported to avoid depending on a
full program-account scan.

All errors include position, market, venue, input amount, expected output, and
transaction or RFQ message hash when available. Secrets and full key material
must never be logged.

## Startup and Runtime Checks

At startup the bot verifies:

- the configured lending program and market accounts are readable;
- the liquidator key and required debt/collateral ATAs are present;
- PropAMM `/health` is ready and `/markets` contains the configured mint pair;
- PropAMM service metadata matches the expected program ID;
- the configured CLAMM pool belongs to the expected program/config and mint pair;
- CLAMM has active liquidity and nonzero relevant vault balances;
- the CLAMM input ATA exists. The quote-specific standing-balance requirement is
  checked for each opportunity; an insufficient float disables only that quote
  and produces a clear warning.

Readiness is venue-specific. A failed PropAMM check does not disable CLAMM, and a
failed CLAMM check does not disable PropAMM. The scanner may start if lending is
healthy and at least one venue is ready.

## Testing Strategy

### Unit tests

- Map collateral/debt mint order to RFQ Buy or Sell exact-input semantics.
- Select the side-specific RFQ payout field.
- Reject a nonzero RFQ version, existing signatures, wrong signer set, wrong fee
  payer, wrong program, changed amount/mints/user, extra instructions, and short
  expiry.
- Sign the returned message hash with only the liquidator key.
- Classify RFQ HTTP/status error codes into unavailable, retryable, and terminal
  outcomes.
- Verify refreshed-quote drift and collateral balance-delta calculations.
- Decode current CLAMM Whirlpool/TickArray wire fixtures and build a SwapV2
  instruction byte-for-byte equivalent to the current generated client.
- Select the CLAMM swap instruction explicitly and reject unexpected setup or
  cleanup sequences.
- Choose the venue with the highest valid debt-token output and handle one or
  both venues being unavailable.

### HTTP and RPC integration tests

- Use a local mock RFQ server for quote, validation, signing, idempotent retry,
  expiry, and swap-response behavior.
- Decode a captured response from the current live RFQ service to guard JSON and
  RuntimeTransaction compatibility without broadcasting.
- Quote the configured aBTC/aUSD CLAMM pool through the adapter and assert a
  positive output for representative liquidation sizes.
- Build the atomic lending callback and assert the callback program, pool,
  source/destination ATAs, exact input, and minimum output.

### Testnet acceptance

1. Start the bot with no PropAMM quote-signer secret or key path in its files or
   process environment.
2. Confirm both venue readiness checks pass against the current aBTC/aUSD pair.
3. Exercise a liquidation where CLAMM wins and verify one atomic transaction.
4. Exercise a liquidation where PropAMM wins and verify lending confirmation,
   fresh RFQ issuance, one user signature, server submission, and processed hash.
5. Stop or invalidate one venue and confirm the other remains usable.
6. Force an RFQ settlement failure after liquidation and verify collateral is
   retained, alerted, and not accidentally combined with a later RFQ amount.

## Completion Criteria

The update is complete when:

- `liquidator-deploy` is cleanly rebased onto current `main`;
- the workspace builds with the version-isolated venue adapters;
- no PropAMM quote-signer secret, keypair, or key path exists in the bot;
- route selection uses executable RFQ and CLAMM quotes rather than PropAMM
  `/health` price math;
- PropAMM execution uses only `/rfq/quote` and `/rfq/swap` and confirms the
  returned server transaction hash;
- CLAMM routes through the funded current testnet pool atomically;
- unit and integration tests pass;
- both route types complete successfully on testnet with the current aBTC/aUSD
  market.
