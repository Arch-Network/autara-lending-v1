#!/usr/bin/env bash
#
# Sync the compiled-in Autara program id to a new program keypair's pubkey.
#
# The on-chain `autara-program` derives the global-config PDA and runs its
# ownership checks against a COMPILED-IN id exposed by `autara_program::id()`:
#
#   pub const fn id() -> Pubkey {
#       Pubkey(hex_literal::hex!("....64-char-hex...."))
#   }
#
# in programs/autara-program/src/lib.rs. For a FRESH deploy with a new program
# key, this constant MUST be rewritten to the new pubkey, then the ELF rebuilt —
# otherwise create_global_config / market instructions target the wrong PDA.
#
#   ./autara-deploy/scripts/sync-program-id.sh <PROGRAM_PUBKEY_HEX>
#
# PROGRAM_PUBKEY_HEX is the 64-char hex (arch_program Pubkey) printed as
# `program_id:` by `autara-deploy --dry-run` (== the program keypair's pubkey).
#
# NO CLIENT EDIT IS NEEDED. Unlike CLAMM (whose generated client hardcodes a
# WHIRLPOOL_ID byte array), the Autara client derives every program id from the
# runtime key files (see autara-client `file_to_pubkey` / `autara_stage_program_id`)
# and `autara-lib` PDA helpers take `program_id` as an argument. The only
# compile-time copy of the id that affects runtime PDA/ownership lives in
# programs/autara-program/src/lib.rs.
#
# The edit is idempotent: re-running with the same id is a no-op. After a change
# you MUST rebuild the ELF before deploying:
#   cd programs/autara-program && cargo-build-sbf --features entrypoint

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
cd "${REPO_ROOT}"

LIB_RS="programs/autara-program/src/lib.rs"

die() {
  echo "ERROR: $*" >&2
  exit 1
}

NEW_HEX="${1:-}"
[ -n "${NEW_HEX}" ] || die "usage: $0 <PROGRAM_PUBKEY_HEX> (64-char hex)"
NEW_HEX="$(printf '%s' "${NEW_HEX}" | tr '[:upper:]' '[:lower:]' | tr -d '[:space:]')"

if ! printf '%s' "${NEW_HEX}" | grep -qE '^[0-9a-f]{64}$'; then
  die "program id must be exactly 64 hex chars, got: '${NEW_HEX}'"
fi

[ -f "${LIB_RS}" ] || die "not found: ${LIB_RS}"
command -v perl >/dev/null 2>&1 || die "perl is required for the in-place edit"

# Current id inside hex_literal::hex!("...") (may span lines, so slurp the file).
CURRENT_HEX="$(perl -0777 -ne 'print $1 if /hex_literal::hex!\(\s*"([0-9a-fA-F]{64})"\s*\)/' "${LIB_RS}" | tr '[:upper:]' '[:lower:]' || true)"

echo "== autara program-id sync =="
echo "file:               ${LIB_RS}"
echo "current id():       ${CURRENT_HEX:-<none found>}"
echo "target  id():       ${NEW_HEX}"

[ -n "${CURRENT_HEX}" ] || die "could not find hex_literal::hex!(\"...\") in ${LIB_RS}"

if [ "${CURRENT_HEX}" = "${NEW_HEX}" ]; then
  echo "id() already matches target; nothing to do (idempotent no-op)."
else
  # Rewrite the 64-char hex inside hex_literal::hex!("..."). Slurp mode so the
  # match works whether or not the literal is split across lines.
  perl -0777 -pi -e 's/(hex_literal::hex!\(\s*")[0-9a-fA-F]{64}("\s*\))/${1}'"${NEW_HEX}"'${2}/' "${LIB_RS}"
fi

# Verify the file now reflects the target id.
AFTER_HEX="$(perl -0777 -ne 'print $1 if /hex_literal::hex!\(\s*"([0-9a-fA-F]{64})"\s*\)/' "${LIB_RS}" | tr '[:upper:]' '[:lower:]' || true)"
[ "${AFTER_HEX}" = "${NEW_HEX}" ] || die "id() update failed (got '${AFTER_HEX}')"

echo "updated:"
echo "  - ${LIB_RS} (autara_program::id())"
echo
echo "REBUILD REQUIRED before deploying:"
echo "  cd programs/autara-program && cargo-build-sbf --features entrypoint"
echo "(also rebuild the host tool so the guard sees the new id: cargo build -p autara-deploy)"
