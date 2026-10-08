//! `cao-parity`: the differential parity harness for the Rust port (#472).
//!
//! Subcommands:
//!
//! - `run`: the Rust driver, writing `RunFacts` JSON;
//! - `corpus`: generate cases, run both builds, compare and report;
//! - `case <id>`: replay one case;
//! - `calibrate`: measure the BC7/BC6H PSNR floor with the oracle.
//!
//! The fact pipeline they share lives in the library. The subcommands
//! themselves need the composition root and the corpus generator, which land
//! in later slices, so each one reports that it is not available yet.

use anyhow::{Result, bail};

const USAGE: &str = "usage: cao-parity <run|corpus|case <id>|calibrate> [options]";

fn main() -> Result<()> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    match arguments.first().map(String::as_str) {
        Some(command @ ("run" | "corpus" | "case" | "calibrate")) => {
            bail!(
                "`cao-parity {command}` is not implemented yet; only the fact pipeline exists so far"
            )
        }
        Some("--help" | "-h") => {
            println!("{USAGE}");
            Ok(())
        }
        _ => bail!("{USAGE}"),
    }
}
