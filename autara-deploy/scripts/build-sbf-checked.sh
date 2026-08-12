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
# The related "Stack offset ... exceeded max offset" warning is deliberately
# NOT gated: arch_program's bitcode dep trips it on every build via
# bitcode::histogram, dead code the linker strips before the final ELF
# (symbol absent from target/deploy/*.so), so gating it would block all
# builds without protecting anything that ships.
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
