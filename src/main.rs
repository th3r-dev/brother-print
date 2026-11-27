use brother_print::{PrintOptions, encode, job_name, render, send};
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "usage: brother-print <host[:port]> <file> [--copies N] [--pages 1-3,5] \
[--paper A4|Letter|...] [--toner-save] [--dry-run out.prn]";

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let mut opts = PrintOptions::default();
    let mut dry_run: Option<PathBuf> = None;
    let mut positional = Vec::new();
    while let Some(a) = args.next() {
        let mut value = || args.next().ok_or(format!("{a} needs a value"));
        match a.as_str() {
            "--copies" => {
                opts.copies = value()?
                    .parse()
                    .ok()
                    .filter(|&n| n >= 1)
                    .ok_or("--copies needs a number from 1 to 65535")?
            }
            "--pages" => opts.page_ranges = Some(value()?),
            "--paper" => opts.paper = value()?.parse()?,
            "--toner-save" => opts.toner_save = true,
            "--dry-run" => dry_run = Some(value()?.into()),
            "-h" | "--help" => return Err(USAGE.into()),
            s if s.starts_with("--") => return Err(format!("unknown option {s}\n{USAGE}")),
            _ => positional.push(a),
        }
    }
    let [host, file] = <[String; 2]>::try_from(positional).map_err(|_| USAGE.to_string())?;
    let file = PathBuf::from(file);
    let pages = render(&file, &opts).map_err(|e| e.to_string())?;
    let job = encode(&pages, &job_name(&file));
    match dry_run {
        Some(out) => {
            std::fs::write(&out, &job).map_err(|e| format!("{}: {e}", out.display()))?;
            eprintln!(
                "{} pages, {} bytes -> {}",
                pages.len(),
                job.len(),
                out.display()
            );
        }
        None => {
            send(&host, &job).map_err(|e| format!("{host}: {e}"))?;
            eprintln!("{} pages, {} bytes sent to {host}", pages.len(), job.len());
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("brother-print: {e}");
            ExitCode::FAILURE
        }
    }
}
