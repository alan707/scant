//! Distribution name -> import name resolution, read from installed
//! metadata (`.dist-info/RECORD`, falling back to `top_level.txt`). Never
//! derived from the distribution name by string munging: `Pillow` -> `PIL`,
//! `PyYAML` -> `yaml`, `python-socketio` -> `socketio` have no derivable
//! relationship to their distribution names. See CLAUDE.md non-negotiable #3.

use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::path::{Path, PathBuf};

use pep508_rs::PackageName;

/// import root -> distribution name, and the reverse, for every distribution
/// installed in a resolved site-packages directory.
#[derive(Debug, Default)]
pub struct NameMap {
    dist_imports: HashMap<PackageName, BTreeSet<String>>,
}

impl NameMap {
    /// Import roots owned by this distribution, if it's installed. A
    /// distribution can own more than one root (`setuptools` ships both
    /// `setuptools` and `pkg_resources`).
    pub fn imports_for(&self, dist: &PackageName) -> Option<&BTreeSet<String>> {
        self.dist_imports.get(dist)
    }

    pub fn contains(&self, dist: &PackageName) -> bool {
        self.dist_imports.contains_key(dist)
    }

    pub fn len(&self) -> usize {
        self.dist_imports.len()
    }

    pub fn is_empty(&self) -> bool {
        self.dist_imports.is_empty()
    }
}

#[derive(Debug)]
pub enum NameMapError {
    NoSitePackages { python_path: PathBuf },
}

impl fmt::Display for NameMapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NameMapError::NoSitePackages { python_path } => write!(
                f,
                "Couldn't find installed packages under '{}'. scant looks for a \
                 site-packages directory there (either directly, or under \
                 lib/python3.*/site-packages or Lib/site-packages). Check that \
                 --python points at a virtualenv or Python install with packages \
                 already installed.",
                python_path.display()
            ),
        }
    }
}

impl std::error::Error for NameMapError {}

fn has_dist_info(dir: &Path) -> bool {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .any(|entry| entry.file_name().to_string_lossy().ends_with(".dist-info"))
        })
        .unwrap_or(false)
}

/// Resolves a `--python` path (a venv root, conda prefix, or bare
/// site-packages dir) to an actual site-packages directory.
pub fn resolve_site_packages(python_path: &Path) -> Result<PathBuf, NameMapError> {
    if has_dist_info(python_path) {
        return Ok(python_path.to_path_buf());
    }

    let posix_lib = python_path.join("lib");
    if let Ok(entries) = std::fs::read_dir(&posix_lib) {
        let mut candidates: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().starts_with("python3"))
            .map(|e| e.path().join("site-packages"))
            .filter(|p| p.is_dir())
            .collect();
        candidates.sort();
        if let Some(site_packages) = candidates.pop() {
            return Ok(site_packages);
        }
    }

    let windows_site_packages = python_path.join("Lib").join("site-packages");
    if windows_site_packages.is_dir() {
        return Ok(windows_site_packages);
    }

    Err(NameMapError::NoSitePackages {
        python_path: python_path.to_path_buf(),
    })
}

/// Reads every `*.dist-info/RECORD` (falling back to `top_level.txt`) under
/// `site_packages` and builds the distribution -> import-roots map.
pub fn build(site_packages: &Path) -> NameMap {
    let mut dist_imports: HashMap<PackageName, BTreeSet<String>> = HashMap::new();

    let Ok(entries) = std::fs::read_dir(site_packages) else {
        return NameMap { dist_imports };
    };

    for entry in entries.filter_map(Result::ok) {
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy();
        let Some(dirname) = name.strip_suffix(".dist-info") else {
            continue;
        };
        // dist-info dirnames escape any `-`/`_`/`.` run in the name to a
        // single `_`, so the version's own hyphen is the last one present.
        let Some((escaped_name, _version)) = dirname.rsplit_once('-') else {
            continue;
        };
        let Ok(package_name) = PackageName::new(escaped_name.to_string()) else {
            continue;
        };

        let import_roots = import_roots_for_dist(&entry.path(), site_packages);
        if !import_roots.is_empty() {
            dist_imports
                .entry(package_name)
                .or_default()
                .extend(import_roots);
        }
    }

    NameMap { dist_imports }
}

fn import_roots_for_dist(dist_info_dir: &Path, site_packages: &Path) -> BTreeSet<String> {
    if let Some(roots) = read_top_level_txt(dist_info_dir)
        && !roots.is_empty()
    {
        return roots;
    }
    read_record(dist_info_dir, site_packages)
}

fn read_top_level_txt(dist_info_dir: &Path) -> Option<BTreeSet<String>> {
    let contents = std::fs::read_to_string(dist_info_dir.join("top_level.txt")).ok()?;
    Some(
        contents
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect(),
    )
}

/// RECORD is CSV-shaped (`path,hash,size`) but hand-rolled here rather than
/// via the `csv` crate: comma-containing filenames in real packages are
/// effectively nonexistent, and this keeps the dependency list narrow.
fn read_record(dist_info_dir: &Path, site_packages: &Path) -> BTreeSet<String> {
    let mut roots = BTreeSet::new();
    let Ok(contents) = std::fs::read_to_string(dist_info_dir.join("RECORD")) else {
        return roots;
    };

    for line in contents.lines() {
        let path_field = line.split(',').next().unwrap_or("").trim();
        if path_field.is_empty() || path_field.starts_with("..") {
            continue;
        }
        match path_field.split_once('/') {
            Some((first_segment, _rest)) => {
                if first_segment.ends_with(".dist-info") || first_segment.ends_with(".data") {
                    continue;
                }
                if site_packages
                    .join(first_segment)
                    .join("__init__.py")
                    .is_file()
                {
                    roots.insert(first_segment.to_string());
                }
            }
            None => {
                if let Some(module_name) = path_field.strip_suffix(".py")
                    && !module_name.is_empty()
                {
                    roots.insert(module_name.to_string());
                }
            }
        }
    }

    roots
}

/// Where an auto-detected Python environment came from -- surfaced in
/// output so users understand why a particular venv was picked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PythonEnvSource {
    Explicit,
    VirtualEnv,
    CondaPrefix,
    LocalVenv,
}

/// Resolves the Python environment to read installed metadata from:
/// `--python` if given, else `$VIRTUAL_ENV`, else `$CONDA_PREFIX`, else a
/// `.venv`/`venv` directory under the scanned root. Returns `None` if none
/// of these resolve -- the caller turns that into a friendly, exit-2 error.
/// Bare interpreter-binary paths (no `sysconfig` shell-out) and system-`PATH`
/// interpreter discovery are out of scope for this phase.
pub fn detect_python_env(
    explicit: Option<&Path>,
    root: &Path,
) -> Option<(PathBuf, PythonEnvSource)> {
    if let Some(path) = explicit {
        return Some((path.to_path_buf(), PythonEnvSource::Explicit));
    }
    if let Some(venv) = non_empty_env_var("VIRTUAL_ENV") {
        return Some((PathBuf::from(venv), PythonEnvSource::VirtualEnv));
    }
    if let Some(conda) = non_empty_env_var("CONDA_PREFIX") {
        return Some((PathBuf::from(conda), PythonEnvSource::CondaPrefix));
    }
    for name in [".venv", "venv"] {
        let candidate = root.join(name);
        if candidate.is_dir() {
            return Some((candidate, PythonEnvSource::LocalVenv));
        }
    }
    None
}

fn non_empty_env_var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::str::FromStr;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("scant-namemap-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn dist_name(s: &str) -> PackageName {
        PackageName::from_str(s).unwrap()
    }

    fn write_top_level(site_packages: &Path, dist_info: &str, roots: &[&str]) {
        let dir = site_packages.join(dist_info);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("top_level.txt"), roots.join("\n")).unwrap();
    }

    #[test]
    fn resolves_pillow_to_pil_via_top_level_txt() {
        let site_packages = temp_dir("pillow");
        write_top_level(&site_packages, "Pillow-10.0.0.dist-info", &["PIL"]);

        let map = build(&site_packages);
        let roots = map.imports_for(&dist_name("Pillow")).unwrap();
        assert_eq!(roots, &BTreeSet::from(["PIL".to_string()]));

        fs::remove_dir_all(&site_packages).unwrap();
    }

    #[test]
    fn resolves_pyyaml_to_yaml() {
        let site_packages = temp_dir("pyyaml");
        write_top_level(&site_packages, "PyYAML-6.0.dist-info", &["yaml"]);

        let map = build(&site_packages);
        let roots = map.imports_for(&dist_name("PyYAML")).unwrap();
        assert_eq!(roots, &BTreeSet::from(["yaml".to_string()]));

        fs::remove_dir_all(&site_packages).unwrap();
    }

    #[test]
    fn dash_and_underscore_variants_match_the_same_normalized_name() {
        let site_packages = temp_dir("socketio");
        write_top_level(
            &site_packages,
            "python_socketio-5.0.0.dist-info",
            &["socketio"],
        );

        let map = build(&site_packages);
        // Both spellings normalize to the same PackageName.
        assert!(map.contains(&dist_name("python-socketio")));
        assert!(map.contains(&dist_name("python_socketio")));

        fs::remove_dir_all(&site_packages).unwrap();
    }

    #[test]
    fn pywin32_resolves_to_win32api() {
        let site_packages = temp_dir("pywin32");
        write_top_level(
            &site_packages,
            "pywin32-306.dist-info",
            &["win32api", "win32com", "pythonwin"],
        );

        let map = build(&site_packages);
        let roots = map.imports_for(&dist_name("pywin32")).unwrap();
        assert!(roots.contains("win32api"));

        fs::remove_dir_all(&site_packages).unwrap();
    }

    #[test]
    fn one_dist_owns_multiple_top_level_import_roots() {
        let site_packages = temp_dir("setuptools");
        write_top_level(
            &site_packages,
            "setuptools-68.0.0.dist-info",
            &["setuptools", "pkg_resources"],
        );

        let map = build(&site_packages);
        let roots = map.imports_for(&dist_name("setuptools")).unwrap();
        assert_eq!(
            roots,
            &BTreeSet::from(["setuptools".to_string(), "pkg_resources".to_string()])
        );

        fs::remove_dir_all(&site_packages).unwrap();
    }

    #[test]
    fn falls_back_to_record_when_top_level_txt_is_absent() {
        let site_packages = temp_dir("record-fallback");
        let dist_info = site_packages.join("requests-2.31.0.dist-info");
        fs::create_dir_all(&dist_info).unwrap();
        // Real package dir with __init__.py, plus a lone top-level module.
        let pkg = site_packages.join("requests");
        fs::create_dir_all(&pkg).unwrap();
        fs::write(pkg.join("__init__.py"), "").unwrap();
        let record = "requests/__init__.py,sha256=abc,123\n\
                       requests/api.py,sha256=def,456\n\
                       requests-2.31.0.dist-info/RECORD,,\n\
                       requests-2.31.0.dist-info/METADATA,sha256=xyz,10\n";
        fs::write(dist_info.join("RECORD"), record).unwrap();

        let map = build(&site_packages);
        let roots = map.imports_for(&dist_name("requests")).unwrap();
        assert_eq!(roots, &BTreeSet::from(["requests".to_string()]));

        fs::remove_dir_all(&site_packages).unwrap();
    }

    #[test]
    fn record_bare_top_level_py_module_is_recognized() {
        let site_packages = temp_dir("record-bare-module");
        let dist_info = site_packages.join("six-1.16.0.dist-info");
        fs::create_dir_all(&dist_info).unwrap();
        fs::write(site_packages.join("six.py"), "").unwrap();
        let record = "six.py,sha256=abc,123\n\
                       six-1.16.0.dist-info/METADATA,sha256=xyz,10\n";
        fs::write(dist_info.join("RECORD"), record).unwrap();

        let map = build(&site_packages);
        let roots = map.imports_for(&dist_name("six")).unwrap();
        assert_eq!(roots, &BTreeSet::from(["six".to_string()]));

        fs::remove_dir_all(&site_packages).unwrap();
    }

    #[test]
    fn namespace_package_without_init_py_does_not_panic() {
        // Namespace-package disambiguation is explicitly out of scope for
        // Phase 1 -- this just proves it doesn't crash the whole namemap
        // build, per plans/Phase1.md.
        let site_packages = temp_dir("namespace-pkg");
        let dist_info = site_packages.join("google-cloud-storage-2.0.0.dist-info");
        fs::create_dir_all(&dist_info).unwrap();
        // No __init__.py under `google/` -- namespace package.
        fs::create_dir_all(site_packages.join("google").join("cloud").join("storage")).unwrap();
        let record = "google/cloud/storage/client.py,sha256=abc,123\n";
        fs::write(dist_info.join("RECORD"), record).unwrap();

        // Must not panic; may legitimately resolve to zero roots.
        let _map = build(&site_packages);

        fs::remove_dir_all(&site_packages).unwrap();
    }

    #[test]
    fn resolve_site_packages_posix_venv_layout() {
        let venv = temp_dir("venv-posix");
        let site_packages = venv.join("lib").join("python3.11").join("site-packages");
        fs::create_dir_all(&site_packages).unwrap();
        fs::create_dir_all(site_packages.join("foo-1.0.dist-info")).unwrap();

        let resolved = resolve_site_packages(&venv).unwrap();
        assert_eq!(resolved, site_packages);

        fs::remove_dir_all(&venv).unwrap();
    }

    #[test]
    fn resolve_site_packages_direct_dist_info() {
        let dir = temp_dir("direct-dist-info");
        fs::create_dir_all(dir.join("foo-1.0.dist-info")).unwrap();

        let resolved = resolve_site_packages(&dir).unwrap();
        assert_eq!(resolved, dir);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn resolve_site_packages_missing_is_friendly_error() {
        let dir = temp_dir("missing-site-packages");
        let err = resolve_site_packages(&dir).unwrap_err();
        assert!(err.to_string().contains("Couldn't find installed packages"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn detect_python_env_prefers_explicit_over_env_vars() {
        let root = temp_dir("detect-explicit");
        let explicit = PathBuf::from("/explicit/path");
        let (path, source) = detect_python_env(Some(&explicit), &root).unwrap();
        assert_eq!(path, explicit);
        assert_eq!(source, PythonEnvSource::Explicit);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn detect_python_env_falls_back_to_local_venv_dir() {
        let root = temp_dir("detect-local-venv");
        fs::create_dir_all(root.join(".venv")).unwrap();

        // SAFETY: test-only env var manipulation, single-threaded per test process
        // section (no other test in this module touches these vars concurrently
        // within the same process run due to Rust's default test isolation being
        // per-test-thread but env vars being process-global -- acceptable here
        // since we clear them immediately after).
        unsafe {
            std::env::remove_var("VIRTUAL_ENV");
            std::env::remove_var("CONDA_PREFIX");
        }

        let (path, source) = detect_python_env(None, &root).unwrap();
        assert_eq!(path, root.join(".venv"));
        assert_eq!(source, PythonEnvSource::LocalVenv);

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn detect_python_env_none_when_nothing_resolves() {
        let root = temp_dir("detect-none");
        unsafe {
            std::env::remove_var("VIRTUAL_ENV");
            std::env::remove_var("CONDA_PREFIX");
        }
        assert!(detect_python_env(None, &root).is_none());
        fs::remove_dir_all(&root).unwrap();
    }
}
