# Autara Arch Testnet Deployment

**Network:** Arch testnet (refreshed)  
**Program deploy date (UTC):** 2026-08-14  
**aUSD/aBTC market fix (UTC):** 2026-08-15  
**Build commit (program ELF):** `5c2a0d3` (main at program deploy; `autara_program::id()` synced to the new program key before ELF build)  
**RPC endpoint:** `https://rpc.testnet.arch.network`

## Replaces compromised-key deployment

This deployment **replaces** the earlier testnet stack that used key material previously tracked under `keys/` (compromised / public-git exposure). Those program/oracle ids must **not** be reused:

| Legacy (compromised) | Old address |
| --- | --- |
| Autara program | `53def2dc8516302842b10e356914d2a5f6b33425ba42aec684f706aa1cf64192` |
| Oracle program | `eee682c27db375bebbc17ed9a76aaa935c8b72bc7de50d736f03e2dfbed84b15` |

New deploy keys live only under gitignored `autara-deploy/.keys-testnet/` (see `docs/key-hygiene.md`). Upgrade authority for the new programs is the **new** deployer pubkey below — not the legacy deployer.

## Programs & config

| Item | Address (hex) |
| --- | --- |
| Autara program | `2aa41c8f71f0ede3f374c15ea1ca6096c3f1c15a10da6530c0ecfa48ba109513` |
| Oracle program | `180ec4dd6eb8d7f2435d4d89c5d166c8e6a3fed3c33f58de25be8e64e94a99dd` |
| Global config PDA | `c17fc27733ed4aaf0faed136032122bb58af5b965e36466dd6d9804b5ade6d1e` |
| Deployer (upgrade authority) | `e63aa66604bf22a88816db487403c19fb03a4d016d48919ce2ad29b71cd26e1c` |
| Admin / fee receiver / curator | `e0be0c3520ca9a06cfccf1f80b5819568e1a4acc0cc3248d405eb4bf0cf03db0` |

Protocol fee share: `5000` bps. Lending market fee: `100` bps.

On-chain post-deploy check (`autara-deploy --dry-run`): `program_on_chain: executable`, `oracle_on_chain: executable`, `program_id_guard: ok`.

## Market parameters (create_market defaults)

| Param | Value |
| --- | --- |
| max_ltv | 0.8 |
| unhealthy_ltv | 0.9 |
| liquidation_bonus | 0.05 |
| max_utilisation | 0.9 |

## Token mints (product)

Arch Network APL mints (public addresses; reused — independent of lending program id). Decimals verified on-chain 2026-08-15:

| Label | Mint (base58) | Mint (hex) | Decimals |
| --- | --- | --- | --- |
| aUSD | `6mqUuwPYehXei6mGBY4bQ6XK1z7e6rrFAZRzYKdH8qkp` | `55c6cee38a31732e2dad821ab1c38f902a7c51efaefb3641d51f3485c4617a45` | 6 |
| aBTC | `2yHWVNYyjnsxZqpnvTbPzWiHwpNQ2zBQU6BC4Lnbu7sW` | `1d46e0dd87393236e4e01252439f46dcbaec7c2255d1fd734e61771a00e8f4e9` | 8 |

Default env: `autara-deploy/scripts/autara.testnet.env` (`MARKET_PAIRS=aUSD/aBTC`). Same mints in `autara.testnet.ausd-abtc.env`.

## Markets

| Pair (supply/collateral) | Market (hex) | Created |
| --- | --- | --- |
| aUSD/aBTC | `e66015deca68f6357b4ebfd7195a7808617082ad310b9c2b7f3db3ab3ca1a5b9` | 2026-08-15 (create-market only; program not redeployed) |

On-chain: market owned by Autara program; supply mint = aUSD, collateral mint = aBTC (`autara-deploy verify` mints/market checks pass). Oracle feed accounts for BTC/USDC still need the pusher before borrow/lend e2e.

### Incorrect markets from 2026-08-14 (do not use as product default)

The initial redeploy accidentally created faucet-mint markets. They remain on-chain but are **not** the Autara product path:

| Pair | Market (hex) |
| --- | --- |
| USDC/BTC | `3f0a4e07765588ff00c02328a94fc82afdb29029ab7d5c03ebfdb2e6fb188e74` |
| USDC/ETH | `fc8cb543f63fb355b723bcaeb9f3498c3f9cbd3bad3ef763f174ac2b02423183` |

## Notable transactions

| Step | Txid |
| --- | --- |
| create program account (autara_program) | `fe5c1d7c7189a751f2e51e6e148d75c3196fe29f493668da27cbc999d3536550` |
| create program account (autara_oracle) | `43dc0fff536a77ea71d2c7b41a868ff21f181f83622ccdf29aaa9f5d15fe7364` |
| create_global_config | `9cc4182ca9cf373e0278d422240c9c4817aed330439e9c521a247565341c2ef5` |
| create_market:aUSD/aBTC | `209b6380e094c617627313472162a0c89079215665cddf0c05ed4ce733f9d791` |

Program/oracle ELF upload used many chunked write transactions (Arch **1232-byte** tx limit; write chunk 997). Explorer: `https://explorer.arch.network/testnet/tx/<txid>`

## Notes

- Keys: only `autara-deploy/.keys-testnet/` (never `keys/*.key`).
- ELF upload path: `elf_upload` sized for the 1232-byte limit.
- `autara_program::id()` in `programs/autara-program/src/lib.rs` was synced to the new program pubkey and ELFs rebuilt before the 2026-08-14 program deploy.
- 2026-08-15: create-market only (`STEP_DEPLOY_*=false`, `STEP_INIT_CONFIG=false`) against the existing program — no program/oracle redeploy.
- Artifact: `deployments/testnet.json` (dated copies: `deployments/testnet-2026-08-14.json`, `deployments/testnet-2026-08-15.json`).
- History purge of old `keys/` material remains a separate coordinated step (not part of this redeploy).
