# Dual-Network Liquidator Health Command

**Status:** Approved design

**Date:** 2026-08-13

## Objective

Update the existing Claude `/liquidator-health` command so one command can audit
either the mainnet or testnet liquidator deployment without duplicating health
logic or silently checking the wrong network.

## Invocation

- `/liquidator-health` defaults to `mainnet` for backward compatibility.
- `/liquidator-health mainnet` selects mainnet explicitly.
- `/liquidator-health testnet` selects testnet explicitly.
- Any other argument must stop with a concise usage error before SSH is run.

## Shared Audit

For the selected network, the command must perform these checks in as few SSH
sessions as practical:

1. systemd unit is enabled, active, stable, and not restart-looping;
2. startup logs contain the expected network, live mode, bot public key, RPC,
   lending program, PropAMM readiness, and CLAMM readiness;
3. the scanner is producing recent position statistics;
4. recent liquidation routes/outcomes and inventory alerts are summarized;
5. recent warnings, errors, panics, or reload failures are reported;
6. native and token balances are checked with read-only tools;
7. PropAMM health and current CLAMM liquidity are checked without signing or
   broadcasting.

The report ends with `HEALTHY`, `DEGRADED`, or `DOWN`, a compact checklist,
position/exposure summary, inventory, and recommended action.

## Network Profiles

### Mainnet

- unit: `autara-liquidator.service`
- config: `/home/ubuntu/autara/liquidator/liquidator-config.json`
- network/RPC: `Bitcoin` / `https://rpc.mainnet.arch.network`
- lending program: `19da5f9b75103d4384b5b78e4b7535198ab0c788578df142db5992a62e30bad0`
- market: `f2af193cddd13528ac155e77bb45838c75d1aeb9f9eb120a8650bc3dc51cb916`
- aUSD: `aec8ca1598d74bc27721536f1a88b5648740bc6a856546a0a47817ff7fe7437c`
- aBTC: `225b03d6f9e05fd834cd18906b019fb46372544b0eeb9f6f8b615472467d46b0`
- PropAMM health: current local mainnet endpoint used by the legacy service
- CLAMM pool: `7caf3541b5d2d9bf06453480acbed988c1c9ebe9ff0edf6deb2f17e0e2e9cb32`

The mainnet profile retains legacy log and backend expectations until the
mainnet service is upgraded to the RFQ implementation.

### Testnet

- unit: `autara-liquidator-testnet.service`
- config: `/home/ubuntu/autara/liquidator-testnet/liquidator-config.json`
- network/RPC: `Testnet` / `https://rpc.testnet.arch.network`
- lending program: `53def2dc8516302842b10e356914d2a5f6b33425ba42aec684f706aa1cf64192`
- market: `9a5a237ddb156c367952ea3562ab3d05f3cdaf0e9bf6ba4fb7b76e233e181f53`
- aUSD: `55c6cee38a31732e2dad821ab1c38f902a7c51efaefb3641d51f3485c4617a45`
- aBTC: `1d46e0dd87393236e4e01252439f46dcbaec7c2255d1fd734e61771a00e8f4e9`
- PropAMM: `https://propamm.arch.network/testnet`
- CLAMM pool: `06db06761eb1f114167ea2bbc4cf98cf8f98fbfc0ad18d1821e724cfeeb03461`
- expected native balance: `2,000,000` lamports (`0.002 ARCH`)
- provisioned inventory baseline: `10,000,000,000` aUSD atoms and `20,000,000`
  aBTC atoms (`10,000 aUSD` and `0.2 aBTC`)

Testnet venue checks use the deployed read-only `check_venues` binary when
available. Missing preflight tooling is `DEGRADED`, not permission to skip the
check or broadcast a transaction.

## Safety

- The command is read-only. It must not restart services, edit configs, mint
  tokens, sign transactions, or invoke a faucet.
- Mint-authority keys are not needed and must not be copied to or read on the
  server.
- The expected bot public key for both profiles is
  `be6fec0e8983f218a5af6ed2f7a95bba8e83ad26d5183fee3231f5e837182b46`.
- A selected profile must never fall back to the other network after a failed
  check.

## Acceptance Criteria

- Mainnet remains the no-argument default.
- Both explicit network arguments select only their matching systemd unit and
  deployment values.
- Invalid arguments fail before SSH.
- The final report identifies the audited network and differentiates a service
  outage from venue, balance, or observability degradation.
- No secret material or state-changing command is introduced.
