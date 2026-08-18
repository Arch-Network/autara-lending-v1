//! Print the Arch pubkey for a secret-key file (no secret bytes on stdout).
//!
//! Usage:
//!   cargo run -p autara-client --example print_pubkey -- --key path/to.key

use anyhow::{anyhow, Result};
use arch_sdk::with_secret_key_file;
use clap::Parser;

#[derive(Parser, Debug)]
struct Args {
    #[clap(long)]
    key: String,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let (_kp, pubkey) =
        with_secret_key_file(&args.key).map_err(|e| anyhow!("load {}: {e}", args.key))?;
    println!("{}", hex::encode(pubkey.serialize()));
    Ok(())
}
