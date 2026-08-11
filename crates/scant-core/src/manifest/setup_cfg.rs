//! `[options] install_requires` (multi-line, one requirement per line) +
//! `[options.extras_require]` (one multi-line block per extra name), via
//! `configparser` with `multiline = true` -- generic INI parsers don't join
//! indented continuation-line values by default, and setup.cfg's
//! `install_requires =` block is exactly that shape. Purely declarative
//! INI, no execution risk.

use std::path::Path;

use configparser::ini::Ini;

use super::{Dependency, DetectorResult, ManifestError, parse_requirement};

pub(crate) fn load(root: &Path) -> Result<Option<DetectorResult>, ManifestError> {
    let path = root.join("setup.cfg");
    if !path.is_file() {
        return Ok(None);
    }

    let contents = std::fs::read_to_string(&path).map_err(|e| ManifestError::Malformed {
        path: path.clone(),
        message: format!("couldn't read the file ({e})"),
    })?;

    let mut ini = Ini::new();
    ini.set_multiline(true);
    let map = ini.read(contents).map_err(|e| ManifestError::Malformed {
        path: path.clone(),
        message: format!("invalid setup.cfg ({e})"),
    })?;

    let mut dependencies = Vec::new();
    let mut warnings = Vec::new();

    if let Some(options) = map.get("options")
        && let Some(Some(value)) = options.get("install_requires")
    {
        extract_requirements(value, &path, &mut dependencies, &mut warnings);
    }

    if let Some(extras) = map.get("options.extras_require") {
        for value in extras.values().flatten() {
            extract_requirements(value, &path, &mut dependencies, &mut warnings);
        }
    }

    Ok(Some(DetectorResult {
        path,
        dependencies,
        warnings,
    }))
}

fn extract_requirements(
    block: &str,
    path: &Path,
    dependencies: &mut Vec<Dependency>,
    warnings: &mut Vec<String>,
) {
    for line in block.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        match parse_requirement(line) {
            Ok(dep) => dependencies.push(dep),
            Err(_) => warnings.push(format!(
                "Couldn't parse a dependency in '{}': '{line}' -- skipped it.",
                path.display()
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("scant-setup-cfg-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn missing_file_returns_none() {
        let dir = temp_dir("missing");
        assert!(load(&dir).unwrap().is_none());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn single_line_install_requires() {
        let dir = temp_dir("single-line");
        fs::write(
            dir.join("setup.cfg"),
            "[options]\ninstall_requires = requests\n",
        )
        .unwrap();

        let result = load(&dir).unwrap().unwrap();
        assert_eq!(result.dependencies.len(), 1);
        assert_eq!(result.dependencies[0].display_name, "requests");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn multi_line_install_requires() {
        let dir = temp_dir("multi-line");
        fs::write(
            dir.join("setup.cfg"),
            "[options]\ninstall_requires =\n    requests>=2.0\n    click\n",
        )
        .unwrap();

        let result = load(&dir).unwrap().unwrap();
        let names: Vec<_> = result
            .dependencies
            .iter()
            .map(|d| d.display_name.clone())
            .collect();
        assert_eq!(names, vec!["requests", "click"]);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn multiple_extras_require_blocks() {
        let dir = temp_dir("extras");
        fs::write(
            dir.join("setup.cfg"),
            "[options]\ninstall_requires = click\n\n\
             [options.extras_require]\ntest =\n    pytest\n    coverage\n\
             docs =\n    sphinx\n",
        )
        .unwrap();

        let result = load(&dir).unwrap().unwrap();
        let names: std::collections::HashSet<_> = result
            .dependencies
            .iter()
            .map(|d| d.display_name.clone())
            .collect();
        assert_eq!(
            names,
            std::collections::HashSet::from([
                "click".to_string(),
                "pytest".to_string(),
                "coverage".to_string(),
                "sphinx".to_string(),
            ])
        );

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_options_section_is_zero_deps_not_an_error() {
        let dir = temp_dir("no-options");
        fs::write(dir.join("setup.cfg"), "[metadata]\nname = foo\n").unwrap();

        let result = load(&dir).unwrap().unwrap();
        assert!(result.dependencies.is_empty());

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn malformed_ini_is_an_error() {
        let dir = temp_dir("malformed");
        fs::write(dir.join("setup.cfg"), "[options\ninstall_requires = foo\n").unwrap();

        let err = load(&dir).unwrap_err();
        assert!(matches!(err, ManifestError::Malformed { .. }));

        fs::remove_dir_all(&dir).unwrap();
    }
}
