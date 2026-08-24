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

    /// Python environment used to map package names to import names: a venv
    /// directory, a conda prefix, a bare site-packages dir, or a direct
    /// path to a Python interpreter. Auto-detected from $VIRTUAL_ENV,
    /// $CONDA_PREFIX, or a pyvenv.cfg-marked folder under PATH when not given.
    #[arg(long, alias = "python")]
    env: Option<PathBuf>,

    /// Read installed packages' source to explain dependencies that look
    /// unused. Off by default: scanning site-packages risks reading a
    /// dependency's own imports as your usage, so scant never does it
    /// unasked. When on, it runs only over dependencies already headed for
    /// "drop", and only inside packages you actually import -- enough to
    /// catch a database driver that Django imports on your behalf.
    #[arg(long)]
    safe_to_scan_site_packages: bool,

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

    let python_env = match namemap::detect_python_env(cli.env.as_deref(), &cli.path) {
        namemap::PythonEnvDetection::Found { path, .. } => path,
        namemap::PythonEnvDetection::Ambiguous(candidates) => {
            eprintln!(
                "{}",
                namemap::format_ambiguous_error(&cli.path, &candidates)
            );
            return ExitCode::from(2);
        }
        namemap::PythonEnvDetection::NotFound => {
            eprintln!("{}", namemap::format_not_found_error(&cli.path));
            return ExitCode::from(2);
        }
    };

    let thresholds = Thresholds {
        lines: cli.threshold_lines,
        files: cli.threshold_files,
        symbols: cli.threshold_symbols,
    };

    let analysis = match analyze::analyze(
        &cli.path,
        &python_env,
        thresholds,
        cli.safe_to_scan_site_packages,
    ) {
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
