use std::collections::HashSet;
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

fn is_denied(name: &str) -> bool {
    DENYLIST.contains(&name) || name.ends_with(".egg-info")
}

pub struct ScanResult {
    pub files_scanned: usize,
    pub elapsed: Duration,
}

/// Walks `root`, returning every `.py` file found, with the hard denylist
/// (`.venv`, `site-packages`, etc.) pruned at the walk level so we never
/// descend into a dependency's own installed copy of itself.
pub fn walk(root: &Path) -> Vec<PathBuf> {
    WalkBuilder::new(root)
        .filter_entry(|entry| {
            let name = entry.file_name().to_string_lossy();
            !is_denied(&name)
        })
        .build()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_some_and(|ft| ft.is_file()))
        .map(|entry| entry.into_path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "py"))
        .collect()
}

/// Top-level directories directly under `root` that contain an `__init__.py`
/// -- the project's own first-party packages. Non-recursive (a `src/`-layout
/// project is a known, accepted gap: see plans/Phase1.md).
///
/// This runs before namemap resolution so a project's own top-level package
/// name is never mistaken for a same-named installed dependency.
pub fn first_party_packages(root: &Path) -> HashSet<String> {
    let mut packages = HashSet::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return packages;
    };
    for entry in entries.filter_map(Result::ok) {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if is_denied(&name) {
            continue;
        }
        if entry.path().join("__init__.py").is_file() {
            packages.insert(name);
        }
    }
    packages
}

pub fn scan(root: &Path) -> ScanResult {
    let start = Instant::now();

    let paths = walk(root);

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

    #[test]
    fn first_party_packages_finds_top_level_init_py_dirs_only() {
        let dir = temp_dir("firstparty");
        let pkg = dir.join("mypkg");
        fs::create_dir_all(&pkg).unwrap();
        fs::write(pkg.join("__init__.py"), "").unwrap();

        // Nested package -- must NOT be picked up (non-recursive).
        let nested = pkg.join("nested");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("__init__.py"), "").unwrap();

        // Directory without __init__.py -- not a package.
        let not_pkg = dir.join("scripts");
        fs::create_dir_all(&not_pkg).unwrap();

        // Denylisted dir even with __init__.py -- must be excluded.
        let venv = dir.join(".venv");
        fs::create_dir_all(&venv).unwrap();
        fs::write(venv.join("__init__.py"), "").unwrap();

        let packages = first_party_packages(&dir);
        assert_eq!(packages, HashSet::from(["mypkg".to_string()]));

        fs::remove_dir_all(&dir).unwrap();
    }
}
