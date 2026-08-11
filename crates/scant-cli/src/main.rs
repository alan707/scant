use std::io::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;

use scant_core::analyze::{self, Thresholds};
use scant_core::{namemap, report};

/// scant -- find unused and barely-used Python dependencies
#[derive(Parser)]
#[command(name = "scant", version)]
struct Cli {
    /// Path to the Python project to scan
    #[arg(default_value = ".")]
    path: PathBuf,

    /// Venv or interpreter directory used to map package names to import
    /// names. Auto-detected from $VIRTUAL_ENV, $CONDA_PREFIX, or a
    /// .venv/venv folder under PATH when not given.
    #[arg(long)]
    python: Option<PathBuf>,

    /// Flag dependencies used on N or fewer lines
    #[arg(long, default_value_t = 3)]
    threshold_lines: u32,

    /// ...and in M or fewer files
    #[arg(long, default_value_t = 2)]
    threshold_files: u32,

    /// ...and importing K or fewer distinct names
    #[arg(long, default_value_t = 1)]
    threshold_symbols: u32,
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    let Some((python_env, _source)) = namemap::detect_python_env(cli.python.as_deref(), &cli.path)
    else {
        eprintln!(
            "Couldn't find a Python environment to read installed package names from. \
             scant checked $VIRTUAL_ENV, $CONDA_PREFIX, and a .venv or venv folder under \
             '{path}', but found none. Activate a virtualenv first, or point at one \
             directly: scant {path} --python .venv",
            path = cli.path.display(),
        );
        return ExitCode::from(2);
    };

    let thresholds = Thresholds {
        lines: cli.threshold_lines,
        files: cli.threshold_files,
        symbols: cli.threshold_symbols,
    };

    let analysis = match analyze::analyze(&cli.path, &python_env, thresholds) {
        Ok(report) => report,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };

    let project_name = cli
        .path
        .canonicalize()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| cli.path.display().to_string());

    let rendered = report::render(&analysis, &project_name, true);
    let _ = write!(anstream::stdout(), "{rendered}");

    if analysis.has_findings() {
        ExitCode::from(1)
    } else {
        ExitCode::from(0)
    }
}
