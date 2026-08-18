#!/usr/bin/env bash
#
# Thin wrapper around the `autara-deploy` Rust tool.
#
#   NETWORK=testnet ./autara-deploy/scripts/deploy.sh --dry-run
#   ENV_FILE=autara-deploy/scripts/autara.testnet.env ./autara-deploy/scripts/deploy.sh --dry-run
#   ./autara-deploy/scripts/deploy.sh autara-deploy/scripts/autara.testnet.env --dry-run
#
# Between networks, the ONLY things that should change are:
#   - ARCH_RPC_URL
#   - the *_KEY_PATH keypair files
#   - the token mints (TOKENS)
# Everything else lives in the env file. Always dry-run first.

set -euo pipefail

# Resolve repo root (two levels up from autara-deploy/scripts).
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
cd "${REPO_ROOT}"

# --- Pick the env file -------------------------------------------------------
# Priority: first positional arg (if it is an existing file) > $ENV_FILE >
# autara-deploy/scripts/autara.<network>.env (network defaults to localnet).
NETWORK="${NETWORK:-localnet}"
DEFAULT_ENV_FILE="${SCRIPT_DIR}/autara.${NETWORK}.env"

ENV_FILE_ARG=""
if [[ $# -gt 0 && -f "$1" ]]; then
  ENV_FILE_ARG="$1"
  shift
fi
ENV_FILE="${ENV_FILE_ARG:-${ENV_FILE:-${DEFAULT_ENV_FILE}}}"

if [[ -f "${ENV_FILE}" ]]; then
  echo "Loading env from ${ENV_FILE}"
  set -a
  # shellcheck disable=SC1090
  source "${ENV_FILE}"
  set +a
else
  echo "WARN: env file '${ENV_FILE}' not found; relying on inherited environment." >&2
fi

# --- Optionally build the program ELFs ---------------------------------------
# The deploy tool needs target/deploy/autara_program.so and autara_oracle.so.
# Skip with SKIP_BUILD=1 if you already have fresh builds.
if [[ "${SKIP_BUILD:-0}" != "1" ]]; then
  echo "Building autara-program (cargo-build-sbf --features entrypoint)..."
  ( cd programs/autara-program && cargo-build-sbf --features entrypoint )
  echo "Building autara-oracle (cargo-build-sbf --features entrypoint)..."
  ( cd programs/autara-oracle && cargo-build-sbf --features entrypoint )
else
  echo "SKIP_BUILD=1 set; using existing target/deploy/*.so"
fi

# --- Run the deploy tool -----------------------------------------------------
# Remaining args (e.g. --dry-run) are passed straight through.
echo "Running autara-deploy..."
cargo run -p autara-deploy -- "$@"
