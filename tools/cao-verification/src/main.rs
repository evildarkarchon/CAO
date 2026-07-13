#![forbid(unsafe_code)]
//! Tracetide parity replay composition root.

use cao_verification::run_setup_replay;
use std::path::PathBuf;

/// Starts the manifest-driven verification replay harness.
fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

/// Parses the stable command-line contract and replays the governed setup tracer.
fn run() -> Result<(), String> {
    let mut arguments = std::env::args_os().skip(1);
    let mut root = None;
    let mut manifest = None;
    while let Some(argument) = arguments.next() {
        match argument.to_str() {
            Some("--root") => {
                root = Some(PathBuf::from(
                    arguments
                        .next()
                        .ok_or_else(|| "--root requires a path".to_owned())?,
                ));
            }
            Some("--manifest") => {
                manifest = Some(PathBuf::from(
                    arguments
                        .next()
                        .ok_or_else(|| "--manifest requires a path".to_owned())?,
                ));
            }
            Some(other) => return Err(format!("unknown argument {other}")),
            None => return Err("arguments must be valid Unicode".to_owned()),
        }
    }

    let root = root.ok_or_else(|| "missing required --root".to_owned())?;
    let manifest = manifest.ok_or_else(|| "missing required --manifest".to_owned())?;
    let report = run_setup_replay(&root, &manifest).map_err(|error| error.to_string())?;
    println!(
        "setup replay passed for {} case(s)",
        report.case_ids().len()
    );
    Ok(())
}
