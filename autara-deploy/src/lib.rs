//! Library surface for deploy tooling shared with other workspace crates.
//!
//! The bin target (`src/main.rs`) keeps its own module tree; this lib exposes
//! only [`elf_upload`], the network-safe ELF uploader (write chunks sized for
//! Arch's 1232-byte transaction limit), so `autara-client`'s upgrade/dry-run
//! bins reuse the exact chunking proven by the testnet redeploys instead of
//! `arch_sdk` 0.6.2's stale 10 KiB probe.

pub mod elf_upload;
