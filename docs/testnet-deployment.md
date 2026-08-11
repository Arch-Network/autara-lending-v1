# Autara Arch Testnet Deployment

**Network:** Arch testnet (refreshed)  
**Deployment date (UTC):** 2026-08-11T19:00:11Z  
**Build commit:** `b10eeaa` (main; includes ELF 1232-byte upload fix from [PR #36](https://github.com/Arch-Network/autara-lending-v1/pull/36))  
**RPC endpoint:** `https://rpc.testnet.arch.network`

## Programs & config

| Item | Address (hex) |
| --- | --- |
| Autara program | `53def2dc8516302842b10e356914d2a5f6b33425ba42aec684f706aa1cf64192` |
| Oracle program | `eee682c27db375bebbc17ed9a76aaa935c8b72bc7de50d736f03e2dfbed84b15` |
| Global config PDA | `e8ac4212c5a46b5e548925091662d72f29218c16d2cb451f51b3808002df5982` |
| Deployer | `5247da872ea2c9dd563072a70c552c2e09da5671035e970c2c9ba7161584b1de` |
| Admin / fee receiver / curator | `9fe2d81600314dc3db735bd6924b655b6a515a4de6f084cbbd23139e9da924ec` |

Protocol fee share: `5000` bps. Lending market fee: `100` bps.

## Market parameters (create_market defaults)

| Param | Value |
| --- | --- |
| max_ltv | 0.8 |
| unhealthy_ltv | 0.9 |
| liquidation_bonus | 0.05 |
| max_utilisation | 0.9 |

## Token mints

| Label | Mint | Decimals |
| --- | --- | --- |
| BTC | `36a97410055bbbdc52b421d0c95f76d85eca066b83db8b14f64665b178c93d8b` | 8 |
| ETH | `7250792453cc3a0bd015778f240dd50b552c48c153b7b83e3ef0c441aff9483c` | 8 |
| USDC | `a80fa79ee82952b0a127f50e7d469dae1a51315d4267ca38d7907ad5df5cb3cb` | 6 |

## Markets

| Pair (supply/collateral) | Market | Created this run |
| --- | --- | --- |
| USDC/BTC | `5b69ba6c8801e5236a6a20a54ca488c4828153add49984f846f6e1d240da1744` | no (already existed) |
| USDC/ETH | `3c7181cbca07e3494be8956cef7400d0adb459ee3df239e7b59dc87dcf934d83` | yes |

## Notable transactions

| Step | Txid |
| --- | --- |
| create_market:USDC/ETH | `6113742f5b3c3e7c004f70369f11866f3e370a57571e711d2d905a084a801c1a` |

Program/oracle redeploy used many ELF write transactions (chunked); see deploy logs. Explorer: `https://explorer.arch.network/testnet/tx/<txid>`

## Notes

- ELF upload uses the fixed `elf_upload` path sized for Arch’s **1232-byte** tx limit (write chunk 997).
- Both `autara_program` and `autara_oracle` were redeployed (on-chain ELF mismatch → rewrite + make executable).
- Global config already existed and was left unchanged.
- Liquidity / supply positions were not seeded by this deploy script.
- Artifact: `deployments/testnet.json` (also copied locally as `deployments/testnet-redeploy-20260811.json`).
- For this redeploy, `autara_program::id()` was temporarily synced to the stage program key `53def2dc…` in the local build worktree so the program-id guard matched `keys/autara-stage.key` (main currently pins the production id for mainnet builds).
