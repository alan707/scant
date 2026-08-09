use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ignore::WalkBuilder;
use rayon::prelude::*;

/// Directories never scanned, regardless of .gitignore: scanning them reads a
/// dependency's own internal imports as project usage. Hard-pruned at the
/// walk level rather than filtered after.
const DENYLIST: &[&str] = &[
    ".venv",
    "venv",
    "site-packages",
    "node_modules",
    "build",
    "dist",
    "__pycache__",
    ".git",
    ".tox",
    ".eggs",
];

pub struct ScanResult {
    pub files_scanned: usize,
    pub elapsed: Duration,
}

pub fn scan(root: &Path) -> ScanResult {
    let start = Instant::now();

    let paths: Vec<PathBuf> = WalkBuilder::new(root)
        .filter_entry(|entry| {
            let name = entry.file_name().to_string_lossy();
            !DENYLIST.contains(&name.as_ref()) && !name.ends_with(".egg-info")
        })
        .build()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_some_and(|ft| ft.is_file()))
        .map(|entry| entry.into_path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "py"))
        .collect();

    let files_scanned = paths
        .par_iter()
        .filter(|path| {
            std::fs::read_to_string(path)
                .ok()
                .is_some_and(|source| ruff_python_parser::parse_module(&source).is_ok())
        })
        .count();

    ScanResult {
        files_scanned,
        elapsed: start.elapsed(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("scant-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn counts_python_files_and_skips_denylisted_dirs() {
        let dir = temp_dir("counts");
        fs::write(dir.join("a.py"), "import os\n").unwrap();
        fs::write(dir.join("b.py"), "x = 1\n").unwrap();
        fs::write(dir.join("readme.md"), "not python\n").unwrap();

        let venv = dir.join(".venv");
        fs::create_dir_all(&venv).unwrap();
        fs::write(venv.join("c.py"), "should not be counted\n").unwrap();

        let result = scan(&dir);
        assert_eq!(result.files_scanned, 2);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn skips_files_that_fail_to_parse() {
        let dir = temp_dir("badparse");
        fs::write(dir.join("bad.py"), "def broken(:\n").unwrap();
        fs::write(dir.join("good.py"), "x = 1\n").unwrap();

        let result = scan(&dir);
        assert_eq!(result.files_scanned, 1);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_root_scans_zero_without_panicking() {
        let dir = std::env::temp_dir().join("scant-test-does-not-exist");
        let _ = fs::remove_dir_all(&dir);

        let result = scan(&dir);
        assert_eq!(result.files_scanned, 0);
    }
}
