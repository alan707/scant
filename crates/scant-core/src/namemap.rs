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
    dist_entry_points: HashMap<PackageName, EntryPoints>,
}

// Entry points a distribution registers in `entry_points.txt`. Anything registering one is loaded by name at runtime -- a framework resolving a plugin, or a shell running a command -- so zero imports is expected, not unused. See CLAUDE.md non-negotiable #4.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct EntryPoints {
    pub plugin_groups: BTreeSet<String>,
    pub console_scripts: BTreeSet<String>,
}

impl EntryPoints {
    pub fn is_empty(&self) -> bool {
        self.plugin_groups.is_empty() && self.console_scripts.is_empty()
    }

    // Short evidence for the report's WHERE column -- names the mechanism that loads it, never a bare assertion. See CLAUDE.md non-negotiable #9.
    pub fn evidence(&self) -> String {
        if let Some(first) = self.plugin_groups.iter().next() {
            return match self.plugin_groups.len() - 1 {
                0 => first.clone(),
                extra => format!("{first} +{extra} more"),
            };
        }
        match self.console_scripts.iter().next() {
            Some(first) => match self.console_scripts.len() - 1 {
                0 => format!("console_scripts: {first}"),
                extra => format!("console_scripts: {first} +{extra} more"),
            },
            None => String::new(),
        }
    }
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

    // Entry points this distribution registers, if it registers any.
    pub fn entry_points_for(&self, dist: &PackageName) -> Option<&EntryPoints> {
        self.dist_entry_points.get(dist)
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
    NoSitePackages {
        python_path: PathBuf,
    },
    InterpreterFailed {
        interpreter: PathBuf,
        message: String,
    },
}

impl fmt::Display for NameMapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NameMapError::NoSitePackages { python_path } => write!(
                f,
                "Couldn't find installed packages under '{}'. scant looks for a \
                 site-packages directory there (either directly, under \
                 lib/python3.*/site-packages or Lib/site-packages, or via a \
                 bin/python interpreter it can ask directly). Check that --env \
                 points at a virtualenv, interpreter, or Python install with \
                 packages already installed.",
                python_path.display()
            ),
            NameMapError::InterpreterFailed {
                interpreter,
                message,
            } => write!(
                f,
                "Tried to ask the Python interpreter at '{}' where its packages are \
                 installed, but that failed: {message}. Make sure the path points at \
                 a real Python interpreter, or point --env at the environment \
                 directory instead.",
                interpreter.display()
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

#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.is_file()
        && std::fs::metadata(path)
            .map(|m| m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
}

#[cfg(windows)]
fn is_executable_file(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"))
}

/// Looks for a venv-shaped interpreter under a directory, so a directory
/// path can be resolved via the same `sysconfig` ask as a direct interpreter
/// path -- avoids hand-guessing the `lib/pythonX.Y` layout when we don't
/// have to.
fn find_interpreter_in(dir: &Path) -> Option<PathBuf> {
    for candidate in [
        "bin/python3",
        "bin/python",
        "Scripts/python.exe",
        "Scripts/python3.exe",
    ] {
        let path = dir.join(candidate);
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

/// Asks a Python interpreter directly where it installs packages, rather
/// than guessing a `lib/pythonX.Y/site-packages`-shaped path -- works for
/// any Python version or layout without pattern-matching on directory names.
fn site_packages_via_sysconfig(interpreter: &Path) -> Result<PathBuf, NameMapError> {
    let fail = |message: String| NameMapError::InterpreterFailed {
        interpreter: interpreter.to_path_buf(),
        message,
    };

    let output = std::process::Command::new(interpreter)
        .args([
            "-c",
            "import sysconfig; print(sysconfig.get_path('purelib'))",
        ])
        .output()
        .map_err(|e| fail(e.to_string()))?;

    if !output.status.success() {
        return Err(fail(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }

    let reported = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    if reported.is_dir() {
        Ok(reported)
    } else {
        Err(fail(format!(
            "reported a site-packages directory that doesn't exist: '{}'",
            reported.display()
        )))
    }
}

/// Resolves a `--env` path (a venv root, conda prefix, bare site-packages
/// dir, or a direct interpreter path) to an actual site-packages directory.
pub fn resolve_site_packages(python_path: &Path) -> Result<PathBuf, NameMapError> {
    if has_dist_info(python_path) {
        return Ok(python_path.to_path_buf());
    }

    if is_executable_file(python_path) {
        return site_packages_via_sysconfig(python_path);
    }

    if let Some(interpreter) = find_interpreter_in(python_path) {
        return site_packages_via_sysconfig(&interpreter);
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
    let mut dist_entry_points: HashMap<PackageName, EntryPoints> = HashMap::new();

    let Ok(entries) = std::fs::read_dir(site_packages) else {
        return NameMap {
            dist_imports,
            dist_entry_points,
        };
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
                .entry(package_name.clone())
                .or_default()
                .extend(import_roots);
        }

        let entry_points = read_entry_points(&entry.path());
        if !entry_points.is_empty() {
            dist_entry_points.insert(package_name, entry_points);
        }
    }

    NameMap {
        dist_imports,
        dist_entry_points,
    }
}

// `entry_points.txt` is INI-shaped, but hand-rolled here for the same reason RECORD is: the format is trivial, and configparser would case-fold the group and script names we want to show verbatim.
fn read_entry_points(dist_info_dir: &Path) -> EntryPoints {
    let mut found = EntryPoints::default();
    let Ok(contents) = std::fs::read_to_string(dist_info_dir.join("entry_points.txt")) else {
        return found;
    };

    let mut group: Option<String> = None;
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            group = Some(name.trim().to_string());
            continue;
        }
        let Some(current) = group.as_deref() else {
            continue;
        };
        // A group with no entries registers nothing, so only a real `name = target` line counts.
        let Some((name, _target)) = line.split_once('=') else {
            continue;
        };
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        if current == "console_scripts" || current == "gui_scripts" {
            found.console_scripts.insert(name.to_string());
        } else {
            found.plugin_groups.insert(current.to_string());
        }
    }

    found
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

/// Outcome of looking for a Python environment when `--env` wasn't given:
/// exactly one candidate is used automatically; more than one is a genuine
/// "which one did you mean" that scant refuses to guess at; zero is the
/// friendly "here's what to do" case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PythonEnvDetection {
    Found {
        path: PathBuf,
        source: PythonEnvSource,
    },
    Ambiguous(Vec<PathBuf>),
    NotFound,
}

/// Resolves the Python environment to read installed metadata from:
/// `--env` if given, else `$VIRTUAL_ENV`, else `$CONDA_PREFIX`, else any
/// directory directly under the scanned root that carries a `pyvenv.cfg`
/// (the actual PEP 405 marker for "this is a venv" -- catches `env/`,
/// `venv311/`, etc., not just the literal names `.venv`/`venv`), falling
/// back to those literal names for a venv that somehow lacks the marker.
pub fn detect_python_env(explicit: Option<&Path>, root: &Path) -> PythonEnvDetection {
    if let Some(path) = explicit {
        return PythonEnvDetection::Found {
            path: path.to_path_buf(),
            source: PythonEnvSource::Explicit,
        };
    }
    if let Some(venv) = non_empty_env_var("VIRTUAL_ENV") {
        return PythonEnvDetection::Found {
            path: PathBuf::from(venv),
            source: PythonEnvSource::VirtualEnv,
        };
    }
    if let Some(conda) = non_empty_env_var("CONDA_PREFIX") {
        return PythonEnvDetection::Found {
            path: PathBuf::from(conda),
            source: PythonEnvSource::CondaPrefix,
        };
    }

    let pyvenv_candidates = find_pyvenv_candidates(root);
    match pyvenv_candidates.len() {
        1 => {
            return PythonEnvDetection::Found {
                path: pyvenv_candidates.into_iter().next().unwrap(),
                source: PythonEnvSource::LocalVenv,
            };
        }
        n if n > 1 => return PythonEnvDetection::Ambiguous(pyvenv_candidates),
        _ => {}
    }

    for name in [".venv", "venv"] {
        let candidate = root.join(name);
        if candidate.is_dir() {
            return PythonEnvDetection::Found {
                path: candidate,
                source: PythonEnvSource::LocalVenv,
            };
        }
    }
    PythonEnvDetection::NotFound
}

/// Directories directly under `root` carrying a `pyvenv.cfg` marker,
/// sorted for determinism. Deliberately shallow (not recursive) -- a full
/// tree walk just to find a venv would be slow on a large repo and risks
/// matching an unrelated nested venv (e.g. one vendored inside a fixture).
fn find_pyvenv_candidates(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut candidates: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir() && p.join("pyvenv.cfg").is_file())
        .collect();
    candidates.sort();
    candidates
}

fn non_empty_env_var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// Looks for a `python3`/`python` interpreter on `$PATH`. Never used for
/// auto-detection -- a system interpreter isn't tied to this project the way
/// an activated venv or a local `pyvenv.cfg` folder is, and could have
/// unrelated packages installed that look like a false match (the same class
/// of bug the pip/setuptools cold-start fix addressed). Worth *suggesting*
/// though: installing straight into a container's system Python (no venv at
/// all) is a common, real setup, and scant should point that out rather than
/// leave the user to already know the right `--env` value for it.
fn find_system_python() -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        for name in ["python3", "python", "python3.exe", "python.exe"] {
            let candidate = dir.join(name);
            if is_executable_file(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

fn render_tree(lines: &[String]) -> String {
    let mut out = String::new();
    for (i, line) in lines.iter().enumerate() {
        let branch = if i + 1 == lines.len() {
            "└──"
        } else {
            "├──"
        };
        out.push_str(&format!("  {branch} {line}\n"));
    }
    out
}

/// Friendly, "what happened / why / what to do next" message for when no
/// Python environment could be found at all.
pub fn format_not_found_error(root: &Path) -> String {
    let virtual_env = non_empty_env_var("VIRTUAL_ENV");
    let conda_prefix = non_empty_env_var("CONDA_PREFIX");
    let system_python = find_system_python();
    let root_display = root.display();

    let mut checked = vec![
        format!(
            "$VIRTUAL_ENV   {}",
            virtual_env.map_or_else(
                || "not set".to_string(),
                |v| format!("set to '{v}', but that didn't resolve")
            )
        ),
        format!(
            "$CONDA_PREFIX  {}",
            conda_prefix.map_or_else(
                || "not set".to_string(),
                |v| format!("set to '{v}', but that didn't resolve")
            )
        ),
        format!("{root_display}/   no .venv, venv, or other pyvenv.cfg-marked folder"),
    ];
    if let Some(p) = &system_python {
        checked.push(format!(
            "system python  found at {} (not used automatically -- see option 4 below)",
            p.display()
        ));
    }

    let mut steps = vec![
        "Activate a virtualenv with your dependencies installed, then re-run scant".to_string(),
        format!("Or point at one directly:    scant {root_display} --env path/to/venv"),
        format!("Or point at an interpreter:  scant {root_display} --env path/to/venv/bin/python"),
    ];
    if let Some(p) = &system_python {
        steps.push(format!(
            "Or use the system python found above:  scant {root_display} --env {}",
            p.display()
        ));
    }
    let steps_text = steps
        .iter()
        .enumerate()
        .map(|(i, s)| format!("  {}. {s}", i + 1))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        "Couldn't find a Python environment for this project.\n\n\
         scant needs to read your dependencies' installed metadata to map declared \
         names to imports (e.g. \"PyYAML\" -> \"yaml\") -- there's no reliable way to \
         do that without them actually being installed somewhere.\n\n\
         Checked:\n{}\n\
         To fix this:\n{steps_text}",
        render_tree(&checked),
    )
}

/// Friendly message for when more than one directory looks like a venv and
/// scant refuses to guess which one is meant.
pub fn format_ambiguous_error(root: &Path, candidates: &[PathBuf]) -> String {
    let tree_lines: Vec<String> = candidates
        .iter()
        .map(|c| {
            let name = c
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| c.display().to_string());
            format!("{name}/   has pyvenv.cfg")
        })
        .collect();
    let root_display = root.display();

    let first_name = candidates[0]
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| candidates[0].display().to_string());

    let system_python_note = match find_system_python() {
        Some(p) => format!(
            "\nA system python is also available, if neither of those is it: \
             scant {root_display} --env {}\n",
            p.display()
        ),
        None => String::new(),
    };

    format!(
        "Found more than one Python environment here -- not sure which one to use.\n\n\
         {root_display}/\n\
         {}{system_python_note}\n\
         Pick one:   scant {root_display} --env {first_name}",
        render_tree(&tree_lines),
    )
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
    fn entry_points_txt_separates_plugin_groups_from_console_scripts() {
        let dir = temp_dir("entry-points");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("entry_points.txt"),
            "[console_scripts]\ngunicorn = gunicorn.app.wsgiapp:run\n\n[sqlalchemy.dialects]\nredshift = sqlalchemy_redshift.dialect:RedshiftDialect\n",
        )
        .unwrap();

        let found = read_entry_points(&dir);
        assert_eq!(
            found.plugin_groups,
            BTreeSet::from(["sqlalchemy.dialects".to_string()])
        );
        assert_eq!(
            found.console_scripts,
            BTreeSet::from(["gunicorn".to_string()])
        );
        assert_eq!(found.evidence(), "sqlalchemy.dialects");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn entry_points_group_header_with_no_entries_registers_nothing() {
        let dir = temp_dir("entry-points-empty-group");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("entry_points.txt"), "[console_scripts]\n\n").unwrap();

        assert!(read_entry_points(&dir).is_empty());

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_entry_points_txt_is_not_an_error() {
        let dir = temp_dir("entry-points-absent");
        fs::create_dir_all(&dir).unwrap();

        assert!(read_entry_points(&dir).is_empty());

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn console_scripts_only_evidence_names_the_command() {
        let dir = temp_dir("entry-points-console-only");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("entry_points.txt"),
            "[console_scripts]\npytest = pytest:console_main\npy.test = pytest:console_main\n",
        )
        .unwrap();

        assert_eq!(
            read_entry_points(&dir).evidence(),
            "console_scripts: py.test +1 more"
        );

        fs::remove_dir_all(&dir).unwrap();
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

    /// SAFETY: test-only env var manipulation. Rust runs unit tests on multiple
    /// threads within one process, and env vars are process-global, so tests
    /// that touch VIRTUAL_ENV/CONDA_PREFIX could race against each other if run
    /// concurrently. Every such test clears both vars first and doesn't set
    /// them to anything another test would observe, which keeps this safe in
    /// practice even without a lock -- worth revisiting if that ever changes.
    fn clear_env_vars() {
        unsafe {
            std::env::remove_var("VIRTUAL_ENV");
            std::env::remove_var("CONDA_PREFIX");
        }
    }

    #[test]
    fn detect_python_env_prefers_explicit_over_env_vars() {
        let root = temp_dir("detect-explicit");
        let explicit = PathBuf::from("/explicit/path");
        match detect_python_env(Some(&explicit), &root) {
            PythonEnvDetection::Found { path, source } => {
                assert_eq!(path, explicit);
                assert_eq!(source, PythonEnvSource::Explicit);
            }
            other => panic!("expected Found, got {other:?}"),
        }
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn detect_python_env_falls_back_to_local_venv_dir() {
        let root = temp_dir("detect-local-venv");
        fs::create_dir_all(root.join(".venv")).unwrap();
        clear_env_vars();

        match detect_python_env(None, &root) {
            PythonEnvDetection::Found { path, source } => {
                assert_eq!(path, root.join(".venv"));
                assert_eq!(source, PythonEnvSource::LocalVenv);
            }
            other => panic!("expected Found, got {other:?}"),
        }

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn detect_python_env_not_found_when_nothing_resolves() {
        let root = temp_dir("detect-none");
        clear_env_vars();
        assert_eq!(detect_python_env(None, &root), PythonEnvDetection::NotFound);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn detect_python_env_finds_a_pyvenv_cfg_folder_by_any_name() {
        let root = temp_dir("detect-pyvenv-cfg");
        // Not named .venv/venv -- only discoverable via the pyvenv.cfg marker.
        fs::create_dir_all(root.join("env311")).unwrap();
        fs::write(root.join("env311").join("pyvenv.cfg"), "").unwrap();
        clear_env_vars();

        match detect_python_env(None, &root) {
            PythonEnvDetection::Found { path, source } => {
                assert_eq!(path, root.join("env311"));
                assert_eq!(source, PythonEnvSource::LocalVenv);
            }
            other => panic!("expected Found, got {other:?}"),
        }

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn detect_python_env_ambiguous_with_multiple_pyvenv_cfg_folders() {
        let root = temp_dir("detect-ambiguous");
        for name in [".venv", "env"] {
            fs::create_dir_all(root.join(name)).unwrap();
            fs::write(root.join(name).join("pyvenv.cfg"), "").unwrap();
        }
        clear_env_vars();

        match detect_python_env(None, &root) {
            PythonEnvDetection::Ambiguous(candidates) => {
                assert_eq!(candidates, vec![root.join(".venv"), root.join("env")]);
            }
            other => panic!("expected Ambiguous, got {other:?}"),
        }

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn format_not_found_error_mentions_env_flag_and_what_was_checked() {
        let root = PathBuf::from("/my/project");
        let msg = format_not_found_error(&root);
        assert!(msg.contains("Couldn't find a Python environment"));
        assert!(msg.contains("--env path/to/venv"));
        assert!(msg.contains("--env path/to/venv/bin/python"));
    }

    #[test]
    fn format_ambiguous_error_lists_every_candidate() {
        let root = PathBuf::from("/my/project");
        let candidates = vec![root.join(".venv"), root.join("env")];
        let msg = format_ambiguous_error(&root, &candidates);
        assert!(msg.contains(".venv/"));
        assert!(msg.contains("env/"));
        assert!(msg.contains("--env .venv"));
    }

    #[cfg(unix)]
    fn write_fake_interpreter(path: &Path, script: &str) {
        use std::os::unix::fs::PermissionsExt;
        fs::write(path, format!("#!/bin/sh\n{script}\n")).unwrap();
        let mut perms = fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn resolve_site_packages_via_direct_interpreter_path() {
        let dir = temp_dir("fake-interpreter-direct");
        let site_packages = dir.join("fake-site-packages");
        fs::create_dir_all(&site_packages).unwrap();
        let interpreter = dir.join("fake-python");
        write_fake_interpreter(&interpreter, &format!("echo '{}'", site_packages.display()));

        let resolved = resolve_site_packages(&interpreter).unwrap();
        assert_eq!(resolved, site_packages);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn resolve_site_packages_finds_interpreter_inside_venv_bin() {
        let venv = temp_dir("fake-interpreter-venv-bin");
        let site_packages = venv.join("fake-site-packages");
        fs::create_dir_all(&site_packages).unwrap();
        fs::create_dir_all(venv.join("bin")).unwrap();
        write_fake_interpreter(
            &venv.join("bin").join("python3"),
            &format!("echo '{}'", site_packages.display()),
        );

        let resolved = resolve_site_packages(&venv).unwrap();
        assert_eq!(resolved, site_packages);

        fs::remove_dir_all(&venv).unwrap();
    }

    /// SAFETY: same category as `clear_env_vars` above -- test-only,
    /// process-global PATH mutation, restored immediately after use. No
    /// other test in this module resolves an interpreter by searching PATH
    /// (they all use absolute paths), so this can't race against them even
    /// under Rust's default multi-threaded test execution.
    #[cfg(unix)]
    fn with_path_override<T>(dir: &Path, f: impl FnOnce() -> T) -> T {
        let original = std::env::var_os("PATH");
        unsafe {
            std::env::set_var("PATH", dir);
        }
        let result = f();
        unsafe {
            match &original {
                Some(p) => std::env::set_var("PATH", p),
                None => std::env::remove_var("PATH"),
            }
        }
        result
    }

    #[cfg(unix)]
    #[test]
    fn find_system_python_locates_an_executable_on_path() {
        let dir = temp_dir("system-python-path");
        write_fake_interpreter(&dir.join("python3"), "true");

        let found = with_path_override(&dir, find_system_python);

        assert_eq!(found, Some(dir.join("python3")));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn find_system_python_none_when_path_has_no_interpreter() {
        let dir = temp_dir("system-python-empty-path");
        let found = with_path_override(&dir, find_system_python);
        assert_eq!(found, None);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn format_not_found_error_suggests_system_python_when_present() {
        let dir = temp_dir("not-found-suggests-system-python");
        write_fake_interpreter(&dir.join("python3"), "true");

        let msg = with_path_override(&dir, || {
            format_not_found_error(&PathBuf::from("/my/project"))
        });

        assert!(msg.contains("system python"));
        assert!(msg.contains(&format!("--env {}", dir.join("python3").display())));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn format_ambiguous_error_suggests_system_python_when_present() {
        let dir = temp_dir("ambiguous-suggests-system-python");
        write_fake_interpreter(&dir.join("python3"), "true");
        let root = PathBuf::from("/my/project");
        let candidates = vec![root.join(".venv"), root.join("env")];

        let msg = with_path_override(&dir, || format_ambiguous_error(&root, &candidates));

        assert!(msg.contains("system python"));
        assert!(msg.contains(&format!("--env {}", dir.join("python3").display())));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn resolve_site_packages_interpreter_failure_is_friendly() {
        let dir = temp_dir("fake-interpreter-broken");
        let interpreter = dir.join("broken-python");
        write_fake_interpreter(&interpreter, "echo 'not a real interpreter' >&2; exit 1");

        let err = resolve_site_packages(&interpreter).unwrap_err();
        assert!(
            err.to_string()
                .contains("Tried to ask the Python interpreter")
        );
        assert!(err.to_string().contains("not a real interpreter"));

        fs::remove_dir_all(&dir).unwrap();
    }
}
