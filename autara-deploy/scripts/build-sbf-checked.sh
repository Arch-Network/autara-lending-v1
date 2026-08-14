#!/usr/bin/env bash
# Build an SBF program and FAIL on the linker's frame-clobber diagnostic.
#
# cargo-build-sbf exits 0 even when the linker prints
#   "A function call in method ... overwrites values in the frame ...
#    may cause undefined behavior during execution".
# That message is proof of live UB in shipped code: SBPFv1 frames are a fixed
# 4KB and the stack region is contiguously mapped, so an over-frame function
# does not fault — its spill slots overlap the next call frame and the first
# CPI clobbers them (shipped once as a runtime access violation in
# BorrowDepositApl). Treat it as a hard build error.
#
# The companion "Stack offset ... exceeded max offset" diagnostic is gated too,
# but only for our own symbols. LLVM emits the frame-clobber message above only
# for an over-4KB function that still makes a call, so a function that exceeds
# 4096 bytes after its callees are inlined into it (a leaf) reports ONLY the
# offset message — the same UB, silently. The one chronic source of that message
# is arch_program's bitcode dep (bitcode::histogram), dead code the linker strips
# before the final ELF (symbol absent from target/deploy/*.so), so it alone is
# excluded rather than the whole diagnostic.
set -euo pipefail

program_dir="$1"
log="$(mktemp)"
trap 'rm -f "$log"' EXIT

( cd "$program_dir" && cargo-build-sbf --features entrypoint ) 2>&1 | tee "$log"

if grep -q "overwrites values in the frame" "$log"; then
  echo "error: SBF frame-clobber diagnostics detected in $program_dir (see above)." >&2
  echo "error: these are undefined behavior at runtime; refusing this build." >&2
  exit 1
fi

# The bitcode exclusion is only sound while that symbol really is stripped from
# the shipped ELF, so verify that rather than trusting it. If it ever survives
# linking, the exclusion would hide live 4KB-frame UB — the exact class of bug
# that shipped as the BorrowDepositApl access violation.
elf="$(ls "$program_dir"/../../target/deploy/*.so 2>/dev/null || true)"
if [ -n "$elf" ] && command -v nm >/dev/null 2>&1; then
  if nm "$elf" 2>/dev/null | grep -q "bitcode.*histogram"; then
    echo "error: bitcode::histogram is present in $elf — it is no longer dead code," >&2
    echo "error: so the over-frame exclusion below is unsound. Fix the gate." >&2
    exit 1
  fi
fi

# Captured rather than tested with `grep -qv`: BSD grep exits 1 for `-qv` even
# when non-matching lines exist, which would silently disable this gate on macOS.
over_frame="$(grep "exceeded max offset" "$log" | grep -v "bitcode" || true)"
if [ -n "$over_frame" ]; then
  echo "error: over-4KB SBF stack frame in $program_dir:" >&2
  echo "$over_frame" >&2
  echo "error: this function's spill slots overlap the next call frame; refusing this build." >&2
  echo "hint: mark the offending function (or its callees) #[inline(never)]." >&2
  exit 1
fi
