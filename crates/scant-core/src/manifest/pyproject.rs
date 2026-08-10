//! PEP 621 `[project.dependencies]` + every group under
//! `[project.optional-dependencies]`. PEP 735 `[dependency-groups]` is not
//! read yet (out of scope for this phase).

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::Deserialize;

use super::{DetectorResult, ManifestError, parse_requirement};

#[derive(Deserialize, Default)]
struct PyProjectToml {
    #[serde(default)]
    project: Option<Project>,
}

#[derive(Deserialize, Default)]
struct Project {
    #[serde(default)]
    dependencies: Vec<String>,
    #[serde(default, rename = "optional-dependencies")]
    optional_dependencies: BTreeMap<String, Vec<String>>,
}

pub(crate) fn load(root: &Path) -> Result<Option<DetectorResult>, ManifestError> {
    let path = root.join("pyproject.toml");
    if !path.is_file() {
        return Ok(None);
    }

    let contents = fs::read_to_string(&path).map_err(|e| ManifestError::Malformed {
        path: path.clone(),
        message: format!("couldn't read the file ({e})"),
    })?;

    let parsed: PyProjectToml =
        toml::from_str(&contents).map_err(|e| ManifestError::Malformed {
            path: path.clone(),
            message: format!("invalid TOML ({e})"),
        })?;

    let mut dependencies = Vec::new();
    let mut warnings = Vec::new();

    if let Some(project) = parsed.project {
        for raw in &project.dependencies {
            push(raw, &path, &mut dependencies, &mut warnings);
        }
        for reqs in project.optional_dependencies.values() {
            for raw in reqs {
                push(raw, &path, &mut dependencies, &mut warnings);
            }
        }
    }

    Ok(Some(DetectorResult {
        path,
        dependencies,
        warnings,
    }))
}

fn push(
    raw: &str,
    path: &Path,
    dependencies: &mut Vec<super::Dependency>,
    warnings: &mut Vec<String>,
) {
    match parse_requirement(raw) {
        Ok(dep) => dependencies.push(dep),
        Err(_) => warnings.push(format!(
            "Couldn't parse a dependency in '{}': '{raw}' -- skipped it.",
            path.display()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("scant-pyproject-{name}-{}", std::process::id()));
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
    fn plain_dependencies() {
        let dir = temp_dir("plain");
        fs::write(
            dir.join("pyproject.toml"),
            "[project]\nname = \"x\"\ndependencies = [\"requests>=2.0\", \"click\"]\n",
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
    fn optional_dependency_groups_are_included() {
        let dir = temp_dir("optional");
        fs::write(
            dir.join("pyproject.toml"),
            "[project]\nname = \"x\"\ndependencies = [\"click\"]\n\n\
             [project.optional-dependencies]\ni18n = [\"babel\"]\n",
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
            std::collections::HashSet::from(["click".to_string(), "babel".to_string()])
        );

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn marker_gated_dependency_is_still_declared() {
        let dir = temp_dir("marker");
        fs::write(
            dir.join("pyproject.toml"),
            "[project]\nname = \"x\"\ndependencies = [\"colorama >=0.4; platform_system == 'Windows'\"]\n",
        )
        .unwrap();

        let result = load(&dir).unwrap().unwrap();
        assert_eq!(result.dependencies.len(), 1);
        assert_eq!(result.dependencies[0].display_name, "colorama");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn underscore_spelled_name_normalizes_like_dashed() {
        let dir = temp_dir("underscore");
        fs::write(
            dir.join("pyproject.toml"),
            "[project]\nname = \"x\"\ndependencies = [\"pyyaml_env_tag\"]\n",
        )
        .unwrap();

        let result = load(&dir).unwrap().unwrap();
        assert_eq!(
            result.dependencies[0].name,
            pep508_rs::PackageName::new("pyyaml-env-tag".to_string()).unwrap()
        );

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn no_project_table_yields_zero_deps_not_an_error() {
        let dir = temp_dir("no-project-table");
        fs::write(
            dir.join("pyproject.toml"),
            "[build-system]\nrequires = []\n",
        )
        .unwrap();

        let result = load(&dir).unwrap().unwrap();
        assert!(result.dependencies.is_empty());

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn malformed_toml_is_an_error() {
        let dir = temp_dir("malformed");
        fs::write(dir.join("pyproject.toml"), "not [ valid toml").unwrap();

        let err = load(&dir).unwrap_err();
        assert!(matches!(err, ManifestError::Malformed { .. }));

        fs::remove_dir_all(&dir).unwrap();
    }
}
