//! `dbfiles`: dump a folder of extracted DBC files into one `DBFilesClient` sqlite file, one
//! table per DBC (named for the file without its `.dbc`). The benilla client reads a table when
//! it exists and the DBC file is still in the patch chain, and falls back to the file otherwise;
//! a file that is not a WDBC is skipped by name, so the client reads it from the chain as before.

use std::path::PathBuf;

use anyhow::{Context, Result};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let (db_dir, out) = match args.len() {
        3 => (PathBuf::from(&args[1]), PathBuf::from(&args[2])),
        _ => {
            eprintln!("usage: dbfiles <dbc-dir> <output-file>");
            std::process::exit(2);
        }
    };
    let summary = benilla_formats::dbfiles::dump_directory(&db_dir, &out)
        .with_context(|| format!("dumping {} -> {}", db_dir.display(), out.display()))?;
    eprintln!(
        "wrote {} ({} tables, {} records, {} bytes)",
        out.display(),
        summary.tables,
        summary.records,
        summary.out_bytes
    );
    for name in &summary.skipped {
        eprintln!("skipped {name}: not a WDBC");
    }
    Ok(())
}
