#!/usr/bin/env bash
#
# set-github-secrets.sh — provision the four Autara deploy keypairs and register
# them as GitHub *Environment* secrets consumed by the Phase 3 CI engine
# (.github/workflows/_autara-action.yml, which sources
#  autara-deploy/scripts/ci-load-env.sh).
#
# The CI engine decodes, per GitHub Environment (e.g. `testnet`, `mainnet`),
# these secrets — each the base64 of a keypair file — into 0600 *_KEY_PATH temp
# files that autara-deploy/src/config.rs reads:
#
#   PROGRAM_KEYPAIR_B64   base64(program  keypair file)  -> PROGRAM_KEY_PATH
#   ORACLE_KEYPAIR_B64    base64(oracle   keypair file)  -> ORACLE_KEY_PATH
#   DEPLOYER_KEYPAIR_B64  base64(deployer keypair file)  -> DEPLOYER_KEY_PATH
#   ADMIN_KEYPAIR_B64     base64(admin    keypair file)  -> ADMIN_KEY_PATH
#   ARCH_RPC_URL          (optional) plain RPC url string (NOT base64)
#
# Keypair files use the arch_sdk `with_secret_key_file` format: a 64-char hex
# secp256k1 secret key (the same format already on disk in
# autara-deploy/.keys-testnet/). Public keys are derived with the crate's OWN
# loader (the autara-deploy binary's dry-run preflight), so generated keys are
# byte-for-byte compatible with a real deploy.
#
# SAFETY:
#   - This script NEVER prints private key bytes. Only base64 is piped to
#     `gh secret set` over STDIN (never in argv / shell history); only PUBLIC
#     keys are printed.
#   - It defaults to --dry-run: nothing is sent to GitHub until you pass --apply.
#   - `--generate` only writes into a gitignored key dir and refuses to clobber
#     existing files without --force.
#   - An explicit --dry-run with --generate is a pure simulation: key generation
#     is only reported ("would generate"), NOTHING is written to disk.
#
set -euo pipefail

# ---------------------------------------------------------------------------
# Layout: this script lives in autara-deploy/scripts/ ; the repo root is two
# levels up. All cargo / git / path work happens relative to the repo root.
# ---------------------------------------------------------------------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
BIN="$REPO_ROOT/target/debug/autara-deploy"

# The four roles, the keypair file basename for each, and the GitHub secret each
# one populates. Index-aligned arrays (no associative arrays: macOS ships bash
# 3.2). The file basenames match what the Phase-2 deploy wrote to .keys-testnet/.
ROLES=(program oracle deployer admin)
FILES=(program.json oracle.json deployer.json admin.json)
SECRETS=(PROGRAM_KEYPAIR_B64 ORACLE_KEYPAIR_B64 DEPLOYER_KEYPAIR_B64 ADMIN_KEYPAIR_B64)

usage() {
  cat <<'EOF'
set-github-secrets.sh — set the Autara deploy keypair GitHub Environment secrets.

USAGE:
  set-github-secrets.sh --env <github-environment> (--from-dir <dir> | --generate) [options]

REQUIRED:
  --env <name>          GitHub Environment to set secrets on (e.g. testnet, mainnet).

KEY SOURCE (exactly one):
  --from-dir <dir>      Use EXISTING keypair files in <dir> (program.json, oracle.json,
                        deployer.json, admin.json). Use this for an already-deployed env
                        so CI manages the current keys (e.g. autara-deploy/.keys-testnet).
  --generate            Create FOUR brand-new keypairs (compatible format) into a
                        gitignored dir, then use those. Use this for a fresh env.
    --out-dir <dir>     Where --generate writes keys (default: autara-deploy/.keys-<env>).
    --force             Allow --generate to overwrite existing key files in the out dir.

OPTIONAL:
  --rpc-url <url>       Also set the ARCH_RPC_URL secret (plain string, not base64).
  --repo <owner/repo>   Target repository (default: auto-detected via gh / git remote).
  --apply               Actually call `gh secret set`. WITHOUT this it is a DRY RUN.
  --no-dry-run          Alias for --apply.
  --dry-run             Force dry run (the default). Prints the plan, sends nothing.
                        When passed EXPLICITLY together with --generate, key
                        generation is only simulated ("would generate"): no files
                        or directories are created or modified on disk.
  -h, --help            Show this help.

EXAMPLES:
  # Preview the testnet plan from the existing keys (sends nothing):
  set-github-secrets.sh --env testnet --from-dir autara-deploy/.keys-testnet --dry-run

  # Apply the testnet secrets from the existing keys:
  set-github-secrets.sh --env testnet --from-dir autara-deploy/.keys-testnet --apply

  # Generate fresh keys for a new env and apply (also set the RPC url):
  set-github-secrets.sh --env mainnet --generate --rpc-url https://rpc.mainnet.arch.network --apply

NOTES:
  - Secrets are set on the GitHub *Environment*, which must already exist
    (repo Settings -> Environments -> <env>). --apply needs `gh` authenticated
    with admin scope on the repo.
  - The program keypair's pubkey becomes the deployed PROGRAM ID; the CI engine
    runs sync-program-id.sh so autara_program::id() matches it.
EOF
}

die() { echo "error: $*" >&2; exit 1; }

# ---------------------------------------------------------------------------
# Argument parsing.
# ---------------------------------------------------------------------------
ENV_NAME=""
FROM_DIR=""
GENERATE=0
OUT_DIR=""
FORCE=0
RPC_URL=""
REPO=""
DRY_RUN=1   # default: dry-run
DRY_RUN_EXPLICIT=0   # 1 only when --dry-run was passed on the command line

while [ $# -gt 0 ]; do
  case "$1" in
    --env)        ENV_NAME="${2:-}"; shift 2 ;;
    --from-dir)   FROM_DIR="${2:-}"; shift 2 ;;
    --generate)   GENERATE=1; shift ;;
    --out-dir)    OUT_DIR="${2:-}"; shift 2 ;;
    --force)      FORCE=1; shift ;;
    --rpc-url)    RPC_URL="${2:-}"; shift 2 ;;
    --repo)       REPO="${2:-}"; shift 2 ;;
    --apply|--no-dry-run) DRY_RUN=0; DRY_RUN_EXPLICIT=0; shift ;;
    --dry-run)    DRY_RUN=1; DRY_RUN_EXPLICIT=1; shift ;;
    -h|--help)    usage; exit 0 ;;
    *)            die "unknown argument: $1 (see --help)" ;;
  esac
done

# ---------------------------------------------------------------------------
# Validate the option combination.
# ---------------------------------------------------------------------------
[ -n "$ENV_NAME" ] || die "--env is required (see --help)"

if [ -n "$FROM_DIR" ] && [ "$GENERATE" -eq 1 ]; then
  die "--from-dir and --generate are mutually exclusive"
fi
if [ -z "$FROM_DIR" ] && [ "$GENERATE" -eq 0 ]; then
  die "choose a key source: --from-dir <dir> or --generate (see --help)"
fi
if [ "$GENERATE" -eq 0 ] && { [ -n "$OUT_DIR" ] || [ "$FORCE" -eq 1 ]; }; then
  die "--out-dir/--force only apply to --generate"
fi

# Resolve the key directory.
if [ "$GENERATE" -eq 1 ]; then
  KEY_DIR="${OUT_DIR:-autara-deploy/.keys-$ENV_NAME}"
else
  KEY_DIR="$FROM_DIR"
fi

# An explicit --dry-run combined with --generate is a pure simulation: nothing
# on disk may be created, modified, or deleted (a real --generate against an
# existing key dir once clobbered live keys).
SIMULATE_GENERATE=0
if [ "$GENERATE" -eq 1 ] && [ "$DRY_RUN_EXPLICIT" -eq 1 ]; then
  SIMULATE_GENERATE=1
fi

# ---------------------------------------------------------------------------
# Make sure the crate binary exists (used to derive pubkeys with the same loader
# a real deploy uses, and — in --generate mode — to create the keys).
# ---------------------------------------------------------------------------
if [ "$SIMULATE_GENERATE" -eq 0 ] && [ ! -x "$BIN" ]; then
  command -v cargo >/dev/null 2>&1 || die "autara-deploy binary not built and cargo not found; run: cargo build -p autara-deploy"
  echo "Building autara-deploy (needed to derive pubkeys)…" >&2
  ( cd "$REPO_ROOT" && cargo build -q -p autara-deploy ) || die "cargo build -p autara-deploy failed"
fi

# ---------------------------------------------------------------------------
# Prepare the key files.
#   --from-dir : all four must already exist (we never write here).
#   --generate : refuse to clobber without --force; the binary's loader creates
#                any missing file in the compatible hex format on first load.
# ---------------------------------------------------------------------------
if [ "$SIMULATE_GENERATE" -eq 1 ]; then
  # Explicit --dry-run: only report what a real --generate would do. Nothing on
  # disk is created, modified, or deleted (no mkdir, no rm, no key writes).
  echo "[dry-run] --generate simulation for $KEY_DIR/ (nothing written to disk):" >&2
  for f in "${FILES[@]}"; do
    path="$REPO_ROOT/$KEY_DIR/$f"
    if [ -e "$path" ]; then
      if [ "$FORCE" -eq 1 ]; then
        echo "  would OVERWRITE existing $KEY_DIR/$f (--force)" >&2
      else
        echo "  $KEY_DIR/$f already exists — a real run would REFUSE without --force" >&2
      fi
    else
      echo "  would generate $KEY_DIR/$f" >&2
    fi
  done
elif [ "$GENERATE" -eq 1 ]; then
  # The out dir must be gitignored so freshly generated secrets can never be
  # committed by accident.
  mkdir -p "$REPO_ROOT/$KEY_DIR" 2>/dev/null || mkdir -p "$KEY_DIR"
  if ! ( cd "$REPO_ROOT" && git check-ignore -q "$KEY_DIR" ); then
    die "refusing to generate keys into '$KEY_DIR': it is NOT gitignored (add it to .gitignore first)"
  fi
  for f in "${FILES[@]}"; do
    path="$REPO_ROOT/$KEY_DIR/$f"
    if [ -e "$path" ]; then
      [ "$FORCE" -eq 1 ] || die "key file exists: $KEY_DIR/$f (pass --force to overwrite)"
      rm -f "$path"
    fi
  done
  echo "Generating four fresh keypairs into $KEY_DIR/ …" >&2
else
  for f in "${FILES[@]}"; do
    path="$REPO_ROOT/$KEY_DIR/$f"
    [ -f "$path" ] || die "missing keypair file: $KEY_DIR/$f"
  done
fi

# ---------------------------------------------------------------------------
# Derive the four public keys using the crate's own loader. The dry-run
# preflight prints `program_id:`, `oracle_id:`, `deployer:`, `admin:` lines (all
# x-only hex pubkeys). We run with a clean env (env -i) and an unreachable RPC so
# nothing touches a real node and the committed env file cannot leak in. In
# --generate mode this same call CREATES the missing key files, so it MUST NOT
# run in the --generate --dry-run simulation (placeholders are shown instead).
# ---------------------------------------------------------------------------
if [ "$SIMULATE_GENERATE" -eq 1 ]; then
  PUB_program="(not derived in --generate dry-run)"
  PUBS=("$PUB_program" "$PUB_program" "$PUB_program" "$PUB_program")
else
  derive_out="$(
    cd "$REPO_ROOT" && env -i HOME="$HOME" PATH="$PATH" \
      NETWORK=localnet ARCH_RPC_URL=http://127.0.0.1:1 \
      PROGRAM_KEY_PATH="$KEY_DIR/program.json" \
      ORACLE_KEY_PATH="$KEY_DIR/oracle.json" \
      DEPLOYER_KEY_PATH="$KEY_DIR/deployer.json" \
      ADMIN_KEY_PATH="$KEY_DIR/admin.json" \
      "$BIN" --dry-run 2>/dev/null || true
  )"

  # Map role -> derived pubkey via the preflight label for that role.
  declare_pub() { awk -v pat="$1" '$1==pat{print $2; exit}' <<<"$derive_out"; }
  PUB_program="$(declare_pub 'program_id:')"
  PUB_oracle="$(declare_pub 'oracle_id:')"
  PUB_deployer="$(declare_pub 'deployer:')"
  PUB_admin="$(declare_pub 'admin:')"

  PUBS=("$PUB_program" "$PUB_oracle" "$PUB_deployer" "$PUB_admin")
  for i in "${!ROLES[@]}"; do
    if ! printf '%s' "${PUBS[$i]}" | grep -qE '^[0-9a-f]{64}$'; then
      die "could not derive a valid pubkey for ${ROLES[$i]} from $KEY_DIR/${FILES[$i]}"
    fi
  done
fi

# ---------------------------------------------------------------------------
# Resolve the target repo (only needed for the plan display + apply).
# ---------------------------------------------------------------------------
if [ -z "$REPO" ]; then
  REPO="$( ( cd "$REPO_ROOT" && gh repo view --json nameWithOwner -q .nameWithOwner ) 2>/dev/null || true )"
fi
if [ -z "$REPO" ]; then
  # Fall back to parsing the origin remote (owner/repo, sans optional .git).
  origin="$( ( cd "$REPO_ROOT" && git remote get-url origin ) 2>/dev/null || true )"
  REPO="$(printf '%s' "$origin" | sed -E 's#^.*github.com[:/]+##; s#\.git$##')"
fi
[ -n "$REPO" ] || die "could not determine target repo; pass --repo <owner/repo>"

# ---------------------------------------------------------------------------
# Print the plan (safe: pubkeys + secret names only). Always shown.
# ---------------------------------------------------------------------------
mode_desc="from-dir ($KEY_DIR)"
[ "$GENERATE" -eq 1 ] && mode_desc="generate -> $KEY_DIR"

echo "== Autara GitHub Environment secrets plan =="
echo "repo:         $REPO"
echo "environment:  $ENV_NAME"
echo "key source:   $mode_desc"
echo
echo "Derived public keys (safe to share) -> secret name:"
for i in "${!ROLES[@]}"; do
  printf '  %-9s %s  ->  %s\n' "${ROLES[$i]}" "${PUBS[$i]}" "${SECRETS[$i]}"
done
if [ -n "$RPC_URL" ]; then
  printf '  %-9s %s  ->  %s\n' "rpc-url" "$RPC_URL" "ARCH_RPC_URL"
fi
echo
echo "NOTE: the program key pubkey ($PUB_program) becomes the on-chain PROGRAM ID."
echo "      The CI engine runs sync-program-id.sh so autara_program::id() matches it."
echo

# ---------------------------------------------------------------------------
# Dry run stops here.
# ---------------------------------------------------------------------------
if [ "$DRY_RUN" -eq 1 ]; then
  echo "[dry-run] No secrets were set. Re-run with --apply to set them on '$ENV_NAME'."
  exit 0
fi

# ---------------------------------------------------------------------------
# Apply: preflight gh auth + repo + environment, then set each secret over STDIN.
# ---------------------------------------------------------------------------
command -v gh >/dev/null 2>&1 || die "gh (GitHub CLI) not found; required for --apply"
gh auth status >/dev/null 2>&1 || die "gh is not authenticated; run: gh auth login"
gh repo view "$REPO" --json nameWithOwner >/dev/null 2>&1 || die "repo not resolvable: $REPO"
if ! gh api "repos/$REPO/environments/$ENV_NAME" >/dev/null 2>&1; then
  die "GitHub Environment '$ENV_NAME' does not exist on $REPO (create it: Settings -> Environments)"
fi

echo "Applying secrets to environment '$ENV_NAME' on $REPO …"
for i in "${!ROLES[@]}"; do
  b64="$(base64 < "$REPO_ROOT/$KEY_DIR/${FILES[$i]}" | tr -d '\n')"
  printf '%s' "$b64" | gh secret set "${SECRETS[$i]}" --env "$ENV_NAME" --repo "$REPO"
  unset b64
  echo "  set ${SECRETS[$i]} (${ROLES[$i]})"
done
if [ -n "$RPC_URL" ]; then
  printf '%s' "$RPC_URL" | gh secret set ARCH_RPC_URL --env "$ENV_NAME" --repo "$REPO"
  echo "  set ARCH_RPC_URL"
fi

echo
echo "Done. Secrets set on '$ENV_NAME'. The CI workflows can now run for this environment."
