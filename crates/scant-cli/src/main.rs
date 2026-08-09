use std::path::PathBuf;

use clap::Parser;

/// scant -- find unused and barely-used Python dependencies
#[derive(Parser)]
#[command(name = "scant", version)]
struct Cli {
    /// Path to the Python project to scan
    #[arg(default_value = ".")]
    path: PathBuf,
}

fn main() {
    let cli = Cli::parse();
    let result = scant_core::discover::scan(&cli.path);
    println!(
        "scanned {} files in {:?}",
        result.files_scanned, result.elapsed
    );
}
