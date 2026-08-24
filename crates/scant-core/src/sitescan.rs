//! Answers one question about installed packages: does a distribution the
//! project actually imports, itself import a distribution the project does
//! not? Django's `django/db/backends/postgresql/base.py` does exactly that
//! with `import psycopg as Database`, which is the only on-disk record that
//! an unimported `psycopg` is a live database driver rather than dead weight
//! -- Django declares it nowhere, so no metadata can say so.
//!
//! Opt-in via `--safe-to-scan-site-packages`, and deliberately so: CLAUDE.md
//! non-negotiable #1 forbids scanning site-packages, because doing so reads a
//! dependency's own internal imports as *project* usage. Nothing here does
//! that. A hit produces a `registered` verdict naming the provider's file, and
//! contributes no imports, lines or symbols to any usage total. The flag exists
//! because that distinction is ours to argue and the user's to accept.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use pep508_rs::PackageName;
use rayon::prelude::*;

use crate::parse;

// Skips generated bindings and vendored blobs: a file this large is not where a driver import lives, and reading it would dominate the scan.
const MAX_FILE_BYTES: u64 = 1_000_000;

/// For each suspect import root, evidence that some provider imports it.
/// Runs only over the suspect set and only inside distributions the project
/// already uses, so it costs nothing when there is nothing to explain.
pub fn find_importers(
    site_packages: &Path,
    suspects: &BTreeMap<String, PackageName>,
    provider_roots: &BTreeSet<String>,
) -> BTreeMap<PackageName, String> {
    if suspects.is_empty() || provider_roots.is_empty() {
        return BTreeMap::new();
    }

    let files: Vec<PathBuf> = provider_roots
        .iter()
        .flat_map(|root| python_files(&site_packages.join(root)))
        .collect();

    let mut hits: Vec<(PackageName, String)> = files
        .par_iter()
        .filter_map(|path| {
            let source = std::fs::read_to_string(path).ok()?;
            // Substring first, parse second. The substring is only a filter -- it decides which files are worth parsing, never what counts as an import. See CLAUDE.md "don't use regex to find imports".
            if !suspects.keys().any(|root| source.contains(root.as_str())) {
                return None;
            }
            let usage = parse::analyze_source(path, &source, &HashSet::new(), &HashSet::new())?;
            let relative = path
                .strip_prefix(site_packages)
                .unwrap_or(path)
                .display()
                .to_string();
            let found: Vec<(PackageName, String)> = suspects
                .iter()
                .filter_map(|(root, dist)| {
                    let record = usage.records.get(root)?;
                    // The import statement itself records no line, only usage sites do -- so name the file alone when the provider imports something it never dereferences.
                    let evidence = match record.lines.iter().next() {
                        Some(line) => format!("imported by {relative}:{line}"),
                        None => format!("imported by {relative}"),
                    };
                    Some((dist.clone(), evidence))
                })
                .collect();
            (!found.is_empty()).then_some(found)
        })
        .flatten()
        .collect();

    // Sorted so the reported file is the same on every run regardless of walk order.
    hits.sort();
    let mut earliest = BTreeMap::new();
    for (dist, evidence) in hits {
        earliest.entry(dist).or_insert(evidence);
    }
    earliest
}

fn python_files(root: &Path) -> Vec<PathBuf> {
    ignore::WalkBuilder::new(root)
        .standard_filters(false)
        .build()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry.path().extension().is_some_and(|ext| ext == "py")
                && entry
                    .metadata()
                    .is_ok_and(|meta| meta.is_file() && meta.len() <= MAX_FILE_BYTES)
        })
        .map(|entry| entry.into_path())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("scant-sitescan-{name}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn suspect(root: &str, dist: &str) -> BTreeMap<String, PackageName> {
        BTreeMap::from([(
            root.to_string(),
            PackageName::new(dist.to_string()).unwrap(),
        )])
    }

    #[test]
    fn a_provider_importing_the_suspect_is_reported_with_its_file() {
        let site = temp_dir("importer");
        let backend = site.join("django/db/backends/postgresql");
        fs::create_dir_all(&backend).unwrap();
        // Django's real shape: the driver is imported under an alias inside a try/except, and Django declares it in no metadata anywhere.
        fs::write(
            backend.join("base.py"),
            "try:\n    import psycopg as Database\nexcept ImportError:\n    pass\n\nx = Database.connect()\n",
        )
        .unwrap();

        let found = find_importers(
            &site,
            &suspect("psycopg", "psycopg"),
            &BTreeSet::from(["django".to_string()]),
        );

        // Built from a Path rather than written out: the evidence carries native separators, the same as every other path in the report.
        let expected = format!(
            "imported by {}:6",
            Path::new("django")
                .join("db")
                .join("backends")
                .join("postgresql")
                .join("base.py")
                .display()
        );
        assert_eq!(
            found.get(&PackageName::new("psycopg".to_string()).unwrap()),
            Some(&expected)
        );
        fs::remove_dir_all(&site).unwrap();
    }

    #[test]
    fn a_bare_mention_in_a_string_or_comment_is_not_an_import() {
        let site = temp_dir("mention");
        fs::create_dir_all(site.join("django")).unwrap();
        fs::write(
            site.join("django/base.py"),
            "# psycopg is required for postgres\nENGINE = \"psycopg\"\n",
        )
        .unwrap();

        let found = find_importers(
            &site,
            &suspect("psycopg", "psycopg"),
            &BTreeSet::from(["django".to_string()]),
        );

        assert!(found.is_empty());
        fs::remove_dir_all(&site).unwrap();
    }

    #[test]
    fn providers_the_project_does_not_use_are_never_opened() {
        let site = temp_dir("unused-provider");
        fs::create_dir_all(site.join("celery")).unwrap();
        fs::write(site.join("celery/app.py"), "import psycopg\n").unwrap();

        let found = find_importers(
            &site,
            &suspect("psycopg", "psycopg"),
            &BTreeSet::from(["django".to_string()]),
        );

        assert!(found.is_empty());
        fs::remove_dir_all(&site).unwrap();
    }
}
