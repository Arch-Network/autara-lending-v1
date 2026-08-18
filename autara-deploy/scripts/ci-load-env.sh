#!/usr/bin/env bash
# Shared CI config loader — SOURCE this (do not execute) from the deploy /
# initialize / upgrade engine steps so they all resolve config the SAME way and
# cannot drift.
#
#   source autara-deploy/scripts/ci-load-env.sh
#
# It loads the chosen network's env file (the same file deploy.sh uses) and then
# layers CI-controlled overrides on top. Overrides come from environment
# variables the workflow injects (NOT interpolated into shell), so there is no
# script-injection surface:
#
#   NETWORK                      target network (also picks the env file)
#   PROGRAM_KEY_PATH / ORACLE_KEY_PATH / DEPLOYER_KEY_PATH / ADMIN_KEY_PATH
#                                decoded-secret key paths (from $GITHUB_ENV)
#   CI_ARCH_RPC_URL              optional RPC override (secret)
#
# It deliberately does NOT set STEP_DEPLOY_PROGRAM / STEP_DEPLOY_ORACLE /
# STEP_INIT_CONFIG / OUTPUT_PATH overrides — the CALLER decides which phase to
# run (and the upgrade action overrides OUTPUT_PATH itself).

# Preserve the CI-provided key paths: sourcing the env file below would
# otherwise clobber them with the committed keys/ defaults.
__kp_program="${PROGRAM_KEY_PATH:-}"
__kp_oracle="${ORACLE_KEY_PATH:-}"
__kp_deployer="${DEPLOYER_KEY_PATH:-}"
__kp_admin="${ADMIN_KEY_PATH:-}"

__env_file="autara-deploy/scripts/autara.${NETWORK:?NETWORK must be set}.env"
if [ ! -f "$__env_file" ]; then
  echo "::error::env file not found: $__env_file" >&2
  exit 1
fi
echo "Loading base config from $__env_file"
set -a
# shellcheck disable=SC1090
. "$__env_file"
set +a

# --- CI overrides win over the env file ------------------------------------
export PROGRAM_KEY_PATH="$__kp_program"
export ORACLE_KEY_PATH="$__kp_oracle"
export DEPLOYER_KEY_PATH="$__kp_deployer"
export ADMIN_KEY_PATH="$__kp_admin"
export NETWORK
export PROGRAM_ELF_PATH="target/deploy/autara_program.so"
export ORACLE_ELF_PATH="target/deploy/autara_oracle.so"
export OUTPUT_PATH="deployments/${NETWORK}.json"
# The engine builds the ELFs explicitly; nothing here should rebuild. (The
# binary ignores SKIP_BUILD; it is honored only by deploy.sh, set for parity.)
export SKIP_BUILD=1

__ov() { # name value -> export name=value only when value is non-empty
  local name="$1" val="${2:-}"
  if [ -n "$val" ]; then export "$name=$val"; echo "  override $name"; fi
}
__ov ARCH_RPC_URL "${CI_ARCH_RPC_URL:-}"
