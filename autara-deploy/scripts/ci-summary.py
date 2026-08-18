#!/usr/bin/env python3
"""Render an Autara deployment artifact as a GitHub Actions job-summary (markdown).

Usage: ci-summary.py <artifact.json> <network>

Reads the JSON written by `autara-deploy` (see autara-deploy/src/artifact.rs)
and prints markdown (addresses, tokens, and per-tx explorer links) to stdout.
Contains addresses and tx ids only — never secrets. Best-effort: any missing
field is simply skipped.
"""
import json
import sys


def explorer_tx_base(network: str) -> str:
    # Per Arch explorer convention: mainnet has no path segment, testnet uses /testnet.
    if network == "mainnet":
        return "https://explorer.arch.network/tx/"
    return f"https://explorer.arch.network/{network}/tx/"


def main() -> int:
    if len(sys.argv) < 3:
        print("usage: ci-summary.py <artifact.json> <network>", file=sys.stderr)
        return 2

    path, network = sys.argv[1], sys.argv[2]
    with open(path) as fh:
        d = json.load(fh)

    out = []

    def row(label, value):
        if value not in (None, ""):
            out.append(f"| {label} | `{value}` |")

    out.append("### Deployment record")
    out.append("")
    out.append("| Field | Value |")
    out.append("| --- | --- |")
    row("network", d.get("network"))
    row("arch_rpc_url", d.get("arch_rpc_url"))
    row("program_id", d.get("program_id"))
    row("oracle_id", d.get("oracle_id"))
    row("build_commit", d.get("build_commit"))
    row("program_elf_sha256", d.get("program_elf_sha256"))
    row("oracle_elf_sha256", d.get("oracle_elf_sha256"))
    row("deployer", d.get("deployer"))
    row("admin", d.get("admin"))
    row("fee_receiver", d.get("fee_receiver"))
    row("protocol_fee_share_bps", d.get("protocol_fee_share_bps"))
    row("global_config", d.get("global_config"))
    out.append("")

    tokens = d.get("tokens") or []
    if tokens:
        out.append("### Tokens")
        out.append("")
        out.append("| label | mint | decimals |")
        out.append("| --- | --- | --- |")
        for t in tokens:
            out.append(
                f"| {t.get('label')} | `{t.get('mint')}` | {t.get('decimals')} |"
            )
        out.append("")

    markets = d.get("markets") or []
    if markets:
        out.append("### Markets")
        out.append("")
        out.append("| pair | market | created |")
        out.append("| --- | --- | --- |")
        for m in markets:
            pair = f"{m.get('supply_label')}/{m.get('collateral_label')}"
            out.append(
                f"| {pair} | `{m.get('market')}` | {m.get('created')} |"
            )
        out.append("")

    txs = d.get("transactions") or []
    if txs:
        base = explorer_tx_base(network)
        out.append("### Transactions")
        out.append("")
        out.append("| Step | Explorer |")
        out.append("| --- | --- |")
        for tx in txs:
            txid = tx.get("txid", "")
            step = tx.get("step", "")
            out.append(f"| {step} | [{txid[:16]}…]({base}{txid}) |")
        out.append("")

    print("\n".join(out))
    return 0


if __name__ == "__main__":
    sys.exit(main())
