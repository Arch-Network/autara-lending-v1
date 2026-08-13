#!/usr/bin/env bash
# Upgrade the LIVE Arch testnet Autara program in place and publish its IDL,
# in one sitting. Run from the repo root:
#
#   ./autara-deploy/scripts/testnet-idl-upgrade.sh
#
# Publishing is deliberately part of THIS script rather than a follow-up step:
# `idl_create_account` authorizes on `from.is_signer` alone and stamps that
# signer as the IDL authority, and the account address is publicly derivable —
# so between `deploy` and `publish_idl` anyone can claim it, after which every
# Write fails `check_authority`. Keep the gap to seconds.
#
# The program is NON-EXECUTABLE between [1/4] retract and [4/4] deploy — all
# lending instructions, including liquidations, revert. Run in a quiet window.
# If it aborts mid-write, just re-run: retract and resize are skipped by their
# guards and the write pass rewrites only the chunks still missing.
set -euo pipefail

STAGE_ID="53def2dc8516302842b10e356914d2a5f6b33425ba42aec684f706aa1cf64192"
PROGRAM_B58="6eQ1vLSAwmbT6SD3KQbNawAqis7LpzwpNTd7SJ1GU5cm"

cd "$(dirname "$0")/../.."

# `id()` is compiled into the program's ownership checks (state.rs) and its
# global-config PDA derivation, and publish_idl reads it to derive the IDL
# account. main pins the MAINNET id, so a testnet build needs the stage id
# swapped in — as an uncommitted working-tree change.
if ! grep -q "$STAGE_ID" programs/autara-program/src/lib.rs; then
  echo "error: programs/autara-program/src/lib.rs does not carry the testnet stage id." >&2
  echo "       swap it in (uncommitted) before running:" >&2
  echo "       $STAGE_ID" >&2
  exit 1
fi

echo "==> building ELF (frame-clobber + over-4KB-frame gates enforced)"
make build-program-autara

echo
echo "==> preconditions"
cargo run -q -p autara-client --bin check_program

echo
echo "==> backing up the current on-chain ELF (rollback target)"
cargo run -q -p autara-client --bin backup_program_elf

echo
echo "==> upgrading LIVE program $PROGRAM_B58 — program is DOWN until [4/4]"
cargo run -q -p autara-client --bin upgrade_program -- --fund

echo
echo "==> publishing IDL immediately (closes the authority land-grab window)"
cargo run -q -p autara-client --bin publish_idl

echo
echo "✓ done. Verify decoding with:"
echo "    cargo run -q -p autara-client --bin check_program"
echo "    curl -s https://explorer.arch.network/api/v1/testnet/transactions/<txid>/instructions | jq '.[].decoded'"
