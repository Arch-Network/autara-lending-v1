#!/usr/bin/env bash
# Verify the e2e_flow targets actually exist on-chain before running the flow.
#
# Without this, a stale id surfaces as an opaque failure deep inside an
# instruction — a wiped testnet deployment showed up as "IncorrectProgramId"
# from InitializeAccount3, which reads like a token bug rather than a missing
# mint. Checking up front names the offending variable instead.
#
# Reads E2E_RPC, E2E_PROGRAM_ID, E2E_MARKET, E2E_AUSD_MINT, E2E_ABTC_MINT and
# ORACLE_PROGRAM_ID from the environment. Exits non-zero if any are absent, or
# if the market is not owned by the program (a mismatched pairing).

set -euo pipefail

: "${E2E_RPC:?E2E_RPC must be set}"
: "${E2E_PROGRAM_ID:?E2E_PROGRAM_ID must be set}"
: "${E2E_MARKET:?E2E_MARKET must be set}"
: "${E2E_AUSD_MINT:?E2E_AUSD_MINT must be set}"
: "${E2E_ABTC_MINT:?E2E_ABTC_MINT must be set}"
: "${ORACLE_PROGRAM_ID:?ORACLE_PROGRAM_ID must be set}"

python3 - <<'PY'
import json, os, sys, urllib.request

RPC = os.environ["E2E_RPC"]

def read_account(hexid):
    body = json.dumps({
        "jsonrpc": "2.0", "id": 1,
        "method": "read_account_info",
        "params": list(bytes.fromhex(hexid)),
    }).encode()
    req = urllib.request.Request(RPC, data=body,
                                 headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=30) as resp:
        return json.load(resp)

TARGETS = [
    ("E2E_PROGRAM_ID", os.environ["E2E_PROGRAM_ID"], True),
    ("ORACLE_PROGRAM_ID", os.environ["ORACLE_PROGRAM_ID"], True),
    ("E2E_MARKET", os.environ["E2E_MARKET"], False),
    ("E2E_AUSD_MINT", os.environ["E2E_AUSD_MINT"], False),
    ("E2E_ABTC_MINT", os.environ["E2E_ABTC_MINT"], False),
]

missing, problems, owners = [], [], {}

for name, hexid, want_exec in TARGETS:
    try:
        bytes.fromhex(hexid)
    except ValueError:
        problems.append(f"{name}={hexid} is not valid hex")
        continue
    try:
        res = read_account(hexid)
    except Exception as exc:
        problems.append(f"{name}: RPC call failed: {exc}")
        continue
    if "error" in res:
        missing.append(f"{name}={hexid} ({res['error'].get('message')})")
        continue
    acct = res["result"]
    owner = acct.get("owner")
    owner = bytes(owner).hex() if isinstance(owner, list) else owner
    owners[name] = owner
    if want_exec and not acct.get("is_executable"):
        problems.append(f"{name}={hexid} exists but is not executable")
    print(f"  ok  {name} = {hexid}")

# A market from a different deployment resolves fine on its own but fails
# confusingly once the flow starts issuing instructions against it.
market_owner = owners.get("E2E_MARKET")
program = os.environ["E2E_PROGRAM_ID"]
if market_owner and market_owner != program:
    problems.append(
        f"E2E_MARKET is owned by {market_owner}, not E2E_PROGRAM_ID {program}"
    )

if missing or problems:
    print("\ne2e targets are not usable on this network:", file=sys.stderr)
    for m in missing:
        print(f"  MISSING  {m}", file=sys.stderr)
    for p in problems:
        print(f"  PROBLEM  {p}", file=sys.stderr)
    print(
        "\nThe deployment these ids point at may have been wiped or replaced.\n"
        "Update the defaults, or override via E2E_* / ORACLE_PROGRAM_ID.\n"
        "Current testnet ids: deployments/testnet-ausd-abtc.json",
        file=sys.stderr,
    )
    sys.exit(1)

print("All e2e targets present.")
PY
