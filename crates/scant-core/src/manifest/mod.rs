//! Manifest loading: finds a project's directly-declared dependencies.
//!
//! **Precedence, not union.** A project may have more than one
//! manifest-shaped file (e.g. `pyproject.toml` for real direct deps plus a
//! `requirements.txt` that's just `pip freeze` output for deployment
//! pinning). Blindly merging every format found would silently reintroduce
//! the exact bug CLAUDE.md non-negotiable #6 exists to prevent: transitive
//! packages read as "directly declared." So [`load`] tries detectors in
//! priority order -- `pyproject.toml` -> `setup.cfg` -> `setup.py` ->
//! `requirements.txt` -- and uses the first one that yields at least one
//! dependency as the sole authoritative source.

pub mod pyproject;
pub mod requirements;
pub mod setup_cfg;
pub mod setup_py;

use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};

use pep508_rs::PackageName;

/// One declared dependency: normalized name for identity/lookup, original
/// spelling for display (`PyYAML`, not `pyyaml`).
#[derive(Debug, Clone)]
pub struct Dependency {
    pub name: PackageName,
    pub display_name: String,
}

/// Which manifest file ended up authoritative.
#[derive(Debug, Clone)]
pub enum ManifestSource {
    PyProject(PathBuf),
    SetupCfg(PathBuf),
    SetupPy(PathBuf),
    Requirements(PathBuf),
}

impl ManifestSource {
    pub fn path(&self) -> &Path {
        match self {
            ManifestSource::PyProject(p)
            | ManifestSource::SetupCfg(p)
            | ManifestSource::SetupPy(p)
            | ManifestSource::Requirements(p) => p,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            ManifestSource::PyProject(_) => "pyproject.toml",
            ManifestSource::SetupCfg(_) => "setup.cfg",
            ManifestSource::SetupPy(_) => "setup.py",
            ManifestSource::Requirements(_) => "requirements.txt",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Manifest {
    pub source: ManifestSource,
    pub dependencies: Vec<Dependency>,
    pub warnings: Vec<String>,
}

#[derive(Debug)]
pub enum ManifestError {
    NotFound { root: PathBuf },
    Malformed { path: PathBuf, message: String },
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ManifestError::NotFound { root } => write!(
                f,
                "Couldn't find a dependency file in '{}'. scant looks for \
                 pyproject.toml, setup.cfg, setup.py, or requirements.txt. \
                 Run it from your project's top folder, or pass the path: \
                 scant path/to/project",
                root.display()
            ),
            ManifestError::Malformed { path, message } => write!(
                f,
                "There's a problem reading '{}': {message}. Fix that and try again.",
                path.display()
            ),
        }
    }
}

impl std::error::Error for ManifestError {}

/// Result of one detector examining the project root: the file it read
/// (which may differ from the file it was pointed at -- e.g. a pip-compile
/// lockfile resolves to its sibling `.in` file), the dependencies it found,
/// and any per-line/per-entry warnings along the way.
#[derive(Debug, Clone)]
pub(crate) struct DetectorResult {
    pub path: PathBuf,
    pub dependencies: Vec<Dependency>,
    pub warnings: Vec<String>,
}

/// Parses one PEP 508 requirement string into a [`Dependency`], keeping the
/// name exactly as spelled (before PEP 503 normalization) for display.
/// Never hand-splits on version operators -- always goes through
/// `pep508_rs`, which also discards any environment marker (a
/// marker-gated dependency, e.g. `colorama; platform_system == 'Windows'`,
/// is still declared).
pub(crate) fn parse_requirement(raw: &str) -> Result<Dependency, String> {
    let trimmed = raw.trim();
    match trimmed.parse::<pep508_rs::Requirement>() {
        Ok(req) => {
            let display_name = extract_display_name(trimmed)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| req.name.to_string());
            Ok(Dependency {
                name: req.name,
                display_name,
            })
        }
        Err(e) => Err(e.to_string()),
    }
}

/// The leading name token of a PEP 508 requirement string, before any
/// version/extras/marker syntax -- used to preserve the manifest's original
/// spelling for display, since `pep508_rs::Requirement::name` is already
/// PEP 503-normalized.
fn extract_display_name(raw: &str) -> Option<&str> {
    let raw = raw.trim();
    let end = raw
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.'))
        .unwrap_or(raw.len());
    if end == 0 { None } else { Some(&raw[..end]) }
}

fn dedupe(deps: Vec<Dependency>) -> Vec<Dependency> {
    let mut seen: HashSet<PackageName> = HashSet::new();
    deps.into_iter()
        .filter(|d| seen.insert(d.name.clone()))
        .collect()
}

/// Loads the project's directly-declared dependencies. See module docs for
/// the precedence rule.
pub fn load(root: &Path) -> Result<Manifest, ManifestError> {
    if let Some(result) = pyproject::load(root)?
        && !result.dependencies.is_empty()
    {
        return Ok(Manifest {
            source: ManifestSource::PyProject(result.path),
            dependencies: dedupe(result.dependencies),
            warnings: result.warnings,
        });
    }

    if let Some(result) = setup_cfg::load(root)?
        && !result.dependencies.is_empty()
    {
        return Ok(Manifest {
            source: ManifestSource::SetupCfg(result.path),
            dependencies: dedupe(result.dependencies),
            warnings: result.warnings,
        });
    }

    if let Some(result) = setup_py::load(root)
        && !result.dependencies.is_empty()
    {
        return Ok(Manifest {
            source: ManifestSource::SetupPy(result.path),
            dependencies: dedupe(result.dependencies),
            warnings: result.warnings,
        });
    }

    if let Some(result) = requirements::load(root)
        && !result.dependencies.is_empty()
    {
        return Ok(Manifest {
            source: ManifestSource::Requirements(result.path),
            dependencies: dedupe(result.dependencies),
            warnings: result.warnings,
        });
    }

    Err(ManifestError::NotFound {
        root: root.to_path_buf(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("scant-manifest-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn not_found_when_nothing_present() {
        let dir = temp_dir("nothing");
        let err = load(&dir).unwrap_err();
        assert!(matches!(err, ManifestError::NotFound { .. }));
        assert!(err.to_string().contains("pyproject.toml"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn pyproject_takes_precedence_over_setup_py() {
        let dir = temp_dir("precedence");
        fs::write(
            dir.join("pyproject.toml"),
            "[project]\nname = \"x\"\ndependencies = [\"requests\"]\n",
        )
        .unwrap();
        fs::write(
            dir.join("setup.py"),
            "from setuptools import setup\nsetup(install_requires=['flask'])\n",
        )
        .unwrap();

        let manifest = load(&dir).unwrap();
        assert!(matches!(manifest.source, ManifestSource::PyProject(_)));
        assert_eq!(manifest.dependencies.len(), 1);
        assert_eq!(manifest.dependencies[0].display_name, "requests");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn falls_through_when_higher_precedence_file_yields_zero_deps() {
        let dir = temp_dir("fallthrough");
        // pyproject.toml with no [project] dependencies at all.
        fs::write(
            dir.join("pyproject.toml"),
            "[build-system]\nrequires = []\n",
        )
        .unwrap();
        fs::write(
            dir.join("setup.py"),
            "from setuptools import setup\nsetup(install_requires=['flask'])\n",
        )
        .unwrap();

        let manifest = load(&dir).unwrap();
        assert!(matches!(manifest.source, ManifestSource::SetupPy(_)));
        assert_eq!(manifest.dependencies.len(), 1);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn malformed_pyproject_is_a_hard_error_no_fallthrough() {
        let dir = temp_dir("malformed");
        fs::write(dir.join("pyproject.toml"), "this is not [ valid toml").unwrap();
        fs::write(
            dir.join("setup.py"),
            "from setuptools import setup\nsetup(install_requires=['flask'])\n",
        )
        .unwrap();

        let err = load(&dir).unwrap_err();
        match err {
            ManifestError::Malformed { path, .. } => {
                assert!(path.ends_with("pyproject.toml"));
            }
            other => panic!("expected Malformed, got {other:?}"),
        }

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn dedupes_by_normalized_name_within_one_source() {
        let dir = temp_dir("dedupe");
        fs::write(
            dir.join("pyproject.toml"),
            "[project]\nname = \"x\"\ndependencies = [\"PyYAML\"]\n\n\
             [project.optional-dependencies]\nextra = [\"pyyaml\"]\n",
        )
        .unwrap();

        let manifest = load(&dir).unwrap();
        assert_eq!(manifest.dependencies.len(), 1);
        assert_eq!(manifest.dependencies[0].display_name, "PyYAML");

        fs::remove_dir_all(&dir).unwrap();
    }
}
