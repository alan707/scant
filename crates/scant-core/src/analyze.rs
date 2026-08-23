//! Orchestrates manifest + namemap + discover + parse, aggregates per-file
//! results into per-dependency usage, and classifies each dependency as
//! `drop` / `inline` / `keep`.

use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use pep508_rs::PackageName;
use rayon::prelude::*;

use crate::discover;
use crate::manifest::{self, Dependency, ManifestError};
use crate::namemap::{self, EntryPoints, NameMap, NameMapError};
use crate::parse::{self, FileUsage};
use pep508_rs::MarkerEnvironment;

#[derive(Debug, Clone, Copy)]
pub struct Thresholds {
    pub lines: u32,
    pub files: u32,
    pub symbols: u32,
}

impl Default for Thresholds {
    fn default() -> Self {
        Thresholds {
            lines: 3,
            files: 2,
            symbols: 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Drop,
    Inline,
    Registered,
    Unknown,
    Keep,
}

// The non-count inputs to a verdict. Grouped rather than passed as three adjacent booleans, where a transposed argument would silently invert a verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Signals {
    pub is_wildcard: bool,
    pub registers_entry_points: bool,
    // Whether some string literal in the project names this distribution's import root -- Django's INSTALLED_APPS and MIDDLEWARE load by dotted name, never by import.
    pub named_by_string: bool,
    // Whether a distribution the project actually imports declares this one behind an `extra ==` marker -- the database-driver case, where the import lives inside the provider rather than in the project.
    pub provided_as_extra: bool,
    pub installable_here: bool,
    // Whether the distribution was actually found in the environment. `drop` is a destructive recommendation, so it requires positive confirmation the package is there and unimported -- never merely that we failed to find it.
    pub installed: bool,
}

impl Default for Signals {
    fn default() -> Self {
        // Installable unless we positively determine otherwise -- absence of a marker environment must never read as "cannot install".
        Signals {
            is_wildcard: false,
            registers_entry_points: false,
            named_by_string: false,
            provided_as_extra: false,
            installable_here: true,
            installed: true,
        }
    }
}

/// Display-only usage scale -- never flips a verdict on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageBand {
    None,
    Trivial,
    Light,
    Moderate,
    Heavy,
}

#[derive(Debug, Clone)]
pub struct DepReport {
    pub display_name: String,
    pub name: PackageName,
    pub imports: u32,
    pub files: u32,
    pub lines: u32,
    pub symbols: u32,
    pub verdict: Verdict,
    pub usage: UsageBand,
    /// Files with real usage, as paths relative to the scanned root, each
    /// paired with the lowest line number touched in that file -- sorted by
    /// path for determinism. Empty for `drop`. `report.rs` renders this as
    /// a single `path:line` for a one-file dependency, or `path +N files`
    /// when usage is spread across more than one.
    pub locations: Vec<(String, u32)>,
    // How this dependency is loaded when nothing imports it -- the entry-point group, or the console script it installs. `Some` only for Registered.
    pub registration: Option<String>,
    // Why no verdict could be reached. `Some` only for Unknown.
    pub unknown_reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Report {
    pub manifest_source_label: &'static str,
    pub files_scanned: usize,
    pub elapsed: Duration,
    pub deps: Vec<DepReport>,
    pub warnings: Vec<String>,
}

impl Report {
    /// `0` if every dependency's verdict is Keep, `1` if anything is
    /// flagged Drop/Inline -- matches PLAN.md's exit code contract.
    pub fn has_findings(&self) -> bool {
        self.deps
            .iter()
            .any(|d| matches!(d.verdict, Verdict::Drop | Verdict::Inline))
    }
}

#[derive(Debug)]
pub enum AnalyzeError {
    Manifest(ManifestError),
    NameMap(NameMapError),
}

impl fmt::Display for AnalyzeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AnalyzeError::Manifest(e) => write!(f, "{e}"),
            AnalyzeError::NameMap(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for AnalyzeError {}

/// Runs the full pipeline: load the manifest, resolve `python_env` to a
/// site-packages dir and build the name map, walk + parse the project in
/// parallel, aggregate per-dependency usage, and classify.
pub fn analyze(
    root: &Path,
    python_env: &Path,
    thresholds: Thresholds,
) -> Result<Report, AnalyzeError> {
    let start = Instant::now();

    let manifest = manifest::load(root).map_err(AnalyzeError::Manifest)?;
    let site_packages =
        namemap::resolve_site_packages(python_env).map_err(AnalyzeError::NameMap)?;
    let name_map = namemap::build(&site_packages);
    // Asked of the environment being scanned, not the host: a 3.9 venv inspected from a 3.13 machine must judge `python_version < "3.10"` as the venv sees it. `None` means we couldn't ask, which stays "unknown" rather than becoming a verdict.
    let marker_env = namemap::marker_environment(python_env);

    let mut warnings = manifest.warnings.clone();

    // Cold-start trap: warns when zero declared deps resolve, excluding pip/setuptools/wheel since every venv bundles them and a project declaring one (Superset pins `pip`) would otherwise mask a totally empty environment.
    let bootstrap_tooling: HashSet<PackageName> = ["pip", "setuptools", "wheel"]
        .into_iter()
        .map(|s| PackageName::new(s.to_string()).unwrap())
        .collect();
    let resolved_overlap = manifest
        .dependencies
        .iter()
        .filter(|d| !bootstrap_tooling.contains(&d.name) && name_map.contains(&d.name))
        .count();
    if !manifest.dependencies.is_empty() && resolved_overlap == 0 {
        warnings.push(format!(
            "None of your declared dependencies appear to be installed in '{}' -- \
             did you mean to install them first?",
            python_env.display()
        ));
    }

    let first_party = discover::first_party_packages(root);
    // Every import root any declared dependency owns. The parser matches string literals against this as it walks, so a project's tens of thousands of strings never have to be collected.
    let declared_roots: HashSet<String> = manifest
        .dependencies
        .iter()
        .filter_map(|dep| name_map.imports_for(&dep.name))
        .flatten()
        .cloned()
        .collect();
    let files = discover::walk(root);
    let files_scanned = files.len();

    let file_usages: Vec<FileUsage> = files
        .par_iter()
        .filter_map(|path| parse::analyze_file(path, &first_party, &declared_roots))
        .collect();

    let usage = ProjectUsage {
        wildcard_roots: file_usages
            .iter()
            .flat_map(|usage| usage.wildcard.iter().cloned())
            .collect(),
        used_roots: file_usages
            .iter()
            .flat_map(|usage| usage.records.keys().map(String::as_str))
            .collect(),
        files: &file_usages,
    };

    let mut deps: Vec<DepReport> = manifest
        .dependencies
        .iter()
        .map(|dep| {
            build_dep_report(
                dep,
                &name_map,
                &usage,
                &thresholds,
                marker_env.as_ref(),
                root,
            )
        })
        .collect();
    deps.sort_by(|a, b| a.name.cmp(&b.name));

    Ok(Report {
        manifest_source_label: manifest.source.label(),
        files_scanned,
        elapsed: start.elapsed(),
        deps,
        warnings,
    })
}

// What the file scan learned about the project, grouped so classifying one dependency takes a single reference rather than three parallel arguments.
struct ProjectUsage<'a> {
    files: &'a [FileUsage],
    wildcard_roots: HashSet<String>,
    // Import roots the project actually uses, for deciding whether a distribution that provides an optional backend is itself in play.
    used_roots: HashSet<&'a str>,
}

fn build_dep_report(
    dep: &Dependency,
    name_map: &NameMap,
    usage: &ProjectUsage,
    thresholds: &Thresholds,
    marker_env: Option<&MarkerEnvironment>,
    scan_root: &Path,
) -> DepReport {
    let import_roots = name_map.imports_for(&dep.name).cloned().unwrap_or_default();

    let mut imports = 0u32;
    let mut lines_total = 0u32;
    let mut symbols: HashSet<&str> = HashSet::new();
    let mut files_with_usage: HashSet<&PathBuf> = HashSet::new();
    let mut locations: Vec<(String, u32)> = Vec::new();

    for file in usage.files {
        let mut min_line: Option<u32> = None;
        for import_root in &import_roots {
            let Some(record) = file.records.get(import_root) else {
                continue;
            };
            imports += record.import_statements;
            lines_total += record.lines.len() as u32;
            symbols.extend(record.symbols.iter().map(String::as_str));
            if let Some(&first) = record.lines.iter().next() {
                min_line = Some(min_line.map_or(first, |m: u32| m.min(first)));
            }
        }
        if let Some(line) = min_line {
            files_with_usage.insert(&file.path);
            let display_path = file
                .path
                .strip_prefix(scan_root)
                .unwrap_or(&file.path)
                .display()
                .to_string();
            locations.push((display_path, line));
        }
    }
    locations.sort();

    let is_wildcard = import_roots
        .iter()
        .any(|root| usage.wildcard_roots.contains(root));
    let files = files_with_usage.len() as u32;
    let symbol_count = symbols.len() as u32;

    // Only a marker we can actually evaluate counts against installability. With no marker environment to judge against, every dependency stays installable-as-far-as-we-know.
    let installable_here = match marker_env {
        Some(env) => dep.marker.evaluate(env, &[]),
        None => true,
    };

    let installed = name_map.contains(&dep.name);
    let entry_points = name_map.entry_points_for(&dep.name);
    // Where a string literal names this dependency, if one does. Reported with the exact file and line so the reader can judge the claim rather than take it -- see CLAUDE.md non-negotiable #9.
    let named_at = usage.files.iter().find_map(|file| {
        import_roots.iter().find_map(|root| {
            file.string_refs.get(root).map(|line| {
                let display_path = file
                    .path
                    .strip_prefix(scan_root)
                    .unwrap_or(&file.path)
                    .display();
                format!("named as a string in {display_path}:{line}")
            })
        })
    });
    // An unimported package that another *used* distribution declares as an optional extra is a backend selected at runtime, not dead weight: SQLAlchemy declares psycopg2-binary and PyMySQL this way and imports them from inside its own dialects. Requiring the provider to be in use is what keeps this from excusing every optional extra in site-packages.
    let provider = name_map
        .extra_providers_for(&dep.name)
        .and_then(|providers| {
            providers.iter().find(|provider| {
                name_map.imports_for(provider).is_some_and(|roots| {
                    roots
                        .iter()
                        .any(|root| usage.used_roots.contains(root.as_str()))
                })
            })
        });
    let (verdict, usage) = classify(
        imports,
        files,
        lines_total,
        symbol_count,
        Signals {
            is_wildcard,
            registers_entry_points: entry_points.is_some(),
            named_by_string: named_at.is_some(),
            provided_as_extra: provider.is_some(),
            installable_here,
            installed,
        },
        thresholds,
    );
    let registration = (verdict == Verdict::Registered)
        .then(|| {
            entry_points
                .map(EntryPoints::evidence)
                .or_else(|| provider.map(|p| format!("optional dependency of {p}")))
                .or(named_at)
        })
        .flatten();
    // The marker is the most specific explanation when it applies -- it says why the package isn't there, not merely that it isn't. Otherwise distinguish a package that is absent from one that is present but tells us nothing about its import names.
    let unknown_reason = (verdict == Verdict::Unknown).then(|| {
        if let Some(marker) = dep.marker.contents() {
            return format!("only installs when {marker}");
        }
        if name_map.is_present(&dep.name) {
            return "installed, but its import names couldn't be determined".to_string();
        }
        "declared, but not installed in this environment".to_string()
    });

    DepReport {
        display_name: dep.display_name.clone(),
        name: dep.name.clone(),
        imports,
        files,
        lines: lines_total,
        symbols: symbol_count,
        verdict,
        usage,
        locations,
        registration,
        unknown_reason,
    }
}

fn classify(
    imports: u32,
    files: u32,
    lines: u32,
    symbols: u32,
    signals: Signals,
    thresholds: &Thresholds,
) -> (Verdict, UsageBand) {
    // `from x import *` creates no bindings, so we fundamentally can't
    // attribute usage to it -- never let that read as "unused" or "barely
    // used" when it might be exercised extensively via names we can't see.
    if signals.is_wildcard {
        return (Verdict::Keep, usage_band_for_keep(lines, thresholds));
    }

    // Registration only ever overrides Drop. A registered package that IS imported is still a fair inline/keep candidate -- suppressing that would cost the signal this tool exists for.
    if imports == 0 {
        // The package's own declaration of how it gets loaded is the strongest evidence there is, and it explains the absent imports even when nothing else about the package resolves -- a binary-only distribution like `dumb-init` owns no import names at all.
        if signals.registers_entry_points {
            return (Verdict::Registered, UsageBand::None);
        }
        // A dependency gated to another platform cannot be installed here, so zero imports is not evidence of anything. Saying so is the honest answer; "drop" would be a guess dressed as a finding.
        if !signals.installable_here {
            return (Verdict::Unknown, UsageBand::None);
        }
        // Nothing resolved for this name: not installed, or installed with metadata we can't read (legacy `.egg-info`). Either way its import roots are unknown, so zero measured imports means nothing.
        if !signals.installed {
            return (Verdict::Unknown, UsageBand::None);
        }
        // Evidence from somewhere other than the package itself, which is why it ranks below "we couldn't read this one": saying "optional dependency of google-cloud-storage" implies we looked for imports and found none, when in truth we never learned its import names.
        if signals.provided_as_extra || signals.named_by_string {
            return (Verdict::Registered, UsageBand::None);
        }
        return (Verdict::Drop, UsageBand::None);
    }

    let below_all_thresholds =
        lines <= thresholds.lines && files <= thresholds.files && symbols <= thresholds.symbols;
    if below_all_thresholds {
        return (Verdict::Inline, UsageBand::Trivial);
    }

    (Verdict::Keep, usage_band_for_keep(lines, thresholds))
}

/// Cosmetic only, never flips a verdict: multiples of `threshold_lines`.
/// Sanity-checked against `requests`: 287 lines / threshold 3 -> 287 > 30 ->
/// Heavy.
fn usage_band_for_keep(lines: u32, thresholds: &Thresholds) -> UsageBand {
    if thresholds.lines == 0 {
        return if lines == 0 {
            UsageBand::Trivial
        } else {
            UsageBand::Heavy
        };
    }
    if lines <= thresholds.lines.saturating_mul(3) {
        UsageBand::Light
    } else if lines <= thresholds.lines.saturating_mul(10) {
        UsageBand::Moderate
    } else {
        UsageBand::Heavy
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("scant-analyze-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn default_thresholds() -> Thresholds {
        Thresholds::default()
    }

    #[test]
    fn classify_truth_table() {
        let t = default_thresholds();

        // imports == 0 -> Drop, regardless of anything else.
        assert_eq!(
            classify(0, 0, 0, 0, Signals::default(), &t),
            (Verdict::Drop, UsageBand::None)
        );

        // Below all thresholds -> Inline/Trivial.
        assert_eq!(
            classify(1, 1, 1, 1, Signals::default(), &t),
            (Verdict::Inline, UsageBand::Trivial)
        );

        // Exactly at the threshold boundary still counts as "below" (<=).
        assert_eq!(
            classify(1, t.files, t.lines, t.symbols, Signals::default(), &t),
            (Verdict::Inline, UsageBand::Trivial)
        );

        // One line over any threshold -> Keep.
        assert_eq!(
            classify(1, t.files, t.lines + 1, t.symbols, Signals::default(), &t).0,
            Verdict::Keep
        );

        // Usage band boundaries (threshold_lines = 3): <=9 Light, <=30 Moderate, else Heavy.
        assert_eq!(
            classify(1, 100, 9, 100, Signals::default(), &t),
            (Verdict::Keep, UsageBand::Light)
        );
        assert_eq!(
            classify(1, 100, 30, 100, Signals::default(), &t),
            (Verdict::Keep, UsageBand::Moderate)
        );
        assert_eq!(
            classify(1, 100, 287, 100, Signals::default(), &t),
            (Verdict::Keep, UsageBand::Heavy)
        );
    }

    #[test]
    fn being_named_in_a_string_overrides_drop_only() {
        let t = default_thresholds();

        // authentik's real shape: django-prometheus appears only in INSTALLED_APPS and MIDDLEWARE, as strings.
        assert_eq!(
            classify(
                0,
                0,
                0,
                0,
                Signals {
                    named_by_string: true,
                    ..Default::default()
                },
                &t
            ),
            (Verdict::Registered, UsageBand::None)
        );

        // A package the project imports is still judged on its usage, string or no string.
        assert_eq!(
            classify(
                1,
                1,
                1,
                1,
                Signals {
                    named_by_string: true,
                    ..Default::default()
                },
                &t
            )
            .0,
            Verdict::Inline
        );
    }

    #[test]
    fn optional_backend_of_a_used_dependency_overrides_drop_only() {
        let t = default_thresholds();

        // mlflow's real shape: psycopg2-binary is never imported and registers no entry points, and is reached only through SQLAlchemy's built-in postgresql dialect.
        assert_eq!(
            classify(
                0,
                0,
                0,
                0,
                Signals {
                    provided_as_extra: true,
                    ..Default::default()
                },
                &t
            ),
            (Verdict::Registered, UsageBand::None)
        );

        // Being another package's optional backend never rescues one the project does import.
        assert_eq!(
            classify(
                1,
                1,
                1,
                1,
                Signals {
                    provided_as_extra: true,
                    ..Default::default()
                },
                &t
            )
            .0,
            Verdict::Inline
        );
    }

    #[test]
    fn entry_point_registration_overrides_drop_but_never_inline_or_keep() {
        let t = default_thresholds();

        // Zero imports + registration -> Registered instead of Drop. This is the Superset case: ~40 SQLAlchemy dialects that are loaded by connection string, never imported.
        assert_eq!(
            classify(
                0,
                0,
                0,
                0,
                Signals {
                    registers_entry_points: true,
                    ..Default::default()
                },
                &t
            ),
            (Verdict::Registered, UsageBand::None)
        );

        // Registration must NOT rescue a package that IS imported from an inline verdict -- that would suppress the signal this tool exists for.
        assert_eq!(
            classify(
                1,
                1,
                1,
                1,
                Signals {
                    registers_entry_points: true,
                    ..Default::default()
                },
                &t
            )
            .0,
            Verdict::Inline
        );
        assert_eq!(
            classify(
                1,
                100,
                287,
                100,
                Signals {
                    registers_entry_points: true,
                    ..Default::default()
                },
                &t
            )
            .0,
            Verdict::Keep
        );
    }

    #[test]
    fn marker_gated_dependency_is_unknown_not_dropped() {
        let t = default_thresholds();

        // Zero imports + cannot install here -> Unknown. Superset declares `waitress; sys_platform == "win32"`, which can never resolve on Linux, so calling it unused would be a guess.
        assert_eq!(
            classify(
                0,
                0,
                0,
                0,
                Signals {
                    installable_here: false,
                    ..Default::default()
                },
                &t
            ),
            (Verdict::Unknown, UsageBand::None)
        );

        // Registration still wins -- it is positive evidence, where a false marker is only absence of evidence.
        assert_eq!(
            classify(
                0,
                0,
                0,
                0,
                Signals {
                    registers_entry_points: true,
                    installable_here: false,
                    ..Default::default()
                },
                &t
            )
            .0,
            Verdict::Registered
        );

        // Platform-guarded code that DOES import it classifies normally: the marker only ever overrides Drop.
        assert_eq!(
            classify(
                1,
                1,
                1,
                1,
                Signals {
                    installable_here: false,
                    ..Default::default()
                },
                &t
            )
            .0,
            Verdict::Inline
        );
    }

    #[test]
    fn marker_for_another_platform_reports_the_marker_as_the_reason() {
        let root = temp_dir("marker-gated");
        fs::write(
            root.join("pyproject.toml"),
            "[project]\nname = \"proj\"\ndependencies = [\"pywin32; sys_platform == 'win32'\"]\n",
        )
        .unwrap();
        let site_packages = root.join(".venv-fake");
        // pywin32 must actually be installed here, so the only thing in question is whether its marker can be evaluated.
        let installed = site_packages.join("pywin32-306.dist-info");
        fs::create_dir_all(&installed).unwrap();
        fs::write(installed.join("top_level.txt"), "win32api").unwrap();
        fs::write(root.join("main.py"), "print('nothing')\n").unwrap();

        let report = analyze(&root, &site_packages, Thresholds::default()).unwrap();
        let dep = report
            .deps
            .iter()
            .find(|d| d.display_name == "pywin32")
            .unwrap();

        // A bare site-packages dir has no interpreter to ask, so there is no marker environment and nothing may be concluded -- it must stay Drop rather than becoming a guess in the other direction.
        assert_eq!(dep.verdict, Verdict::Drop);
        assert!(dep.unknown_reason.is_none());

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_dependency_that_resolves_to_nothing_is_unknown_not_dropped() {
        let root = temp_dir("unresolvable");
        fs::write(
            root.join("pyproject.toml"),
            "[project]\nname = \"proj\"\ndependencies = [\"feedgen\", \"click\"]\n",
        )
        .unwrap();

        let site_packages = root.join(".venv-fake");
        // Only click resolves. feedgen stands in for the two ways a declared dependency can go missing: never installed, or installed with metadata scant cannot read (legacy `.egg-info`).
        let click = site_packages.join("click-8.1.7.dist-info");
        fs::create_dir_all(&click).unwrap();
        fs::write(click.join("top_level.txt"), "click").unwrap();
        // Used heavily so it lands on `keep`, leaving the unknown verdict as the only thing that could make this a finding.
        let heavy_use: String = std::iter::once("import click\n".to_string())
            .chain((0..12).map(|i| format!("click.echo('{i}')\n")))
            .collect();
        fs::write(root.join("main.py"), heavy_use).unwrap();

        let report = analyze(&root, &site_packages, Thresholds::default()).unwrap();
        let feedgen = report
            .deps
            .iter()
            .find(|d| d.display_name == "feedgen")
            .unwrap();

        // Telling someone to delete a package we never even found is the most damaging thing this tool can do.
        assert_eq!(feedgen.verdict, Verdict::Unknown);
        assert_eq!(
            feedgen.unknown_reason.as_deref(),
            Some("declared, but not installed in this environment")
        );
        assert!(!report.has_findings());

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_present_dependency_with_no_import_names_says_so() {
        let root = temp_dir("present-no-imports");
        fs::write(
            root.join("pyproject.toml"),
            "[project]\nname = \"proj\"\ndependencies = [\"fastmcp\"]\n",
        )
        .unwrap();

        // fastmcp's real shape: a metapackage whose dist-info is present but which ships no importable modules at all.
        let site_packages = root.join(".venv-fake");
        let dist_info = site_packages.join("fastmcp-3.4.7.dist-info");
        fs::create_dir_all(&dist_info).unwrap();
        fs::write(
            dist_info.join("RECORD"),
            "fastmcp-3.4.7.dist-info/METADATA,,\n",
        )
        .unwrap();
        fs::write(root.join("main.py"), "print('nothing')\n").unwrap();

        let dep = &analyze(&root, &site_packages, Thresholds::default())
            .unwrap()
            .deps[0];

        assert_eq!(dep.verdict, Verdict::Unknown);
        // Claiming it isn't installed would be false -- it is, it just tells us nothing.
        assert_eq!(
            dep.unknown_reason.as_deref(),
            Some("installed, but its import names couldn't be determined")
        );

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn egg_info_metadata_counts_as_present() {
        let root = temp_dir("egg-info-present");
        fs::write(
            root.join("pyproject.toml"),
            "[project]\nname = \"proj\"\ndependencies = [\"feedgen\"]\n",
        )
        .unwrap();

        // Legacy metadata scant cannot yet read (see #40) -- but it is proof the package is installed, so the reason must not claim otherwise.
        let site_packages = root.join(".venv-fake");
        fs::create_dir_all(site_packages.join("feedgen-1.0.0.egg-info")).unwrap();
        // A directory holding only `.egg-info` isn't recognized as site-packages at all, so give it one ordinary dist.
        let click = site_packages.join("click-8.1.7.dist-info");
        fs::create_dir_all(&click).unwrap();
        fs::write(click.join("top_level.txt"), "click").unwrap();
        fs::write(root.join("main.py"), "print('nothing')\n").unwrap();

        let report = analyze(&root, &site_packages, Thresholds::default()).unwrap();
        let dep = report
            .deps
            .iter()
            .find(|d| d.display_name == "feedgen")
            .unwrap();

        assert_eq!(dep.verdict, Verdict::Unknown);
        assert_eq!(
            dep.unknown_reason.as_deref(),
            Some("installed, but its import names couldn't be determined")
        );

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn drop_requires_confirming_the_package_is_installed() {
        let t = default_thresholds();

        // Installed and unimported is the only shape that earns a drop.
        assert_eq!(
            classify(0, 0, 0, 0, Signals::default(), &t).0,
            Verdict::Drop
        );
        assert_eq!(
            classify(
                0,
                0,
                0,
                0,
                Signals {
                    installed: false,
                    ..Default::default()
                },
                &t
            )
            .0,
            Verdict::Unknown
        );
    }

    #[test]
    fn registered_dependencies_are_not_findings() {
        let root = temp_dir("registered-exit-code");
        fs::write(
            root.join("pyproject.toml"),
            "[project]\nname = \"proj\"\ndependencies = [\"sqlalchemy_redshift\"]\n",
        )
        .unwrap();

        let site_packages = root.join(".venv-fake");
        let dist_info = site_packages.join("sqlalchemy_redshift-0.8.14.dist-info");
        fs::create_dir_all(&dist_info).unwrap();
        fs::write(dist_info.join("top_level.txt"), "sqlalchemy_redshift").unwrap();
        fs::write(
            dist_info.join("entry_points.txt"),
            "[sqlalchemy.dialects]\nredshift = sqlalchemy_redshift.dialect:RedshiftDialect\n",
        )
        .unwrap();
        fs::write(root.join("main.py"), "print('no imports here')\n").unwrap();

        let report = analyze(&root, &site_packages, Thresholds::default()).unwrap();
        let dep = &report.deps[0];

        assert_eq!(dep.verdict, Verdict::Registered);
        assert_eq!(dep.registration.as_deref(), Some("sqlalchemy.dialects"));
        // Exit code 1 means "we found something you should act on" -- a registered dependency is working as designed.
        assert!(!report.has_findings());

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn wildcard_import_forces_keep_even_with_zero_measured_usage() {
        let t = default_thresholds();
        // Without the override this would read as Inline (imports=1, but
        // files/lines/symbols all 0, which is <= every threshold).
        let (verdict, _) = classify(
            1,
            0,
            0,
            0,
            Signals {
                is_wildcard: true,
                ..Default::default()
            },
            &t,
        );
        assert_eq!(verdict, Verdict::Keep);
    }

    /// A small synthetic end-to-end run reproducing the shape of the mkdocs
    /// case (drop / light / heavy) without needing real pip.
    #[test]
    fn end_to_end_synthetic_project() {
        let root = temp_dir("e2e");
        fs::write(
            root.join("pyproject.toml"),
            "[project]\nname = \"proj\"\ndependencies = [\"unused_dep\", \"barely_used\", \"heavily_used\"]\n",
        )
        .unwrap();

        let site_packages = root.join(".venv-fake");
        fs::create_dir_all(&site_packages).unwrap();
        for (dist, import_root) in [
            ("unused_dep", "unused_dep"),
            ("barely_used", "barely_used"),
            ("heavily_used", "heavily_used"),
        ] {
            let dist_info = site_packages.join(format!("{dist}-1.0.0.dist-info"));
            fs::create_dir_all(&dist_info).unwrap();
            fs::write(dist_info.join("top_level.txt"), import_root).unwrap();
        }

        fs::write(
            root.join("main.py"),
            "import barely_used\nimport heavily_used\n\n\
             barely_used.helper()\n\n\
             heavily_used.a()\nheavily_used.b()\nheavily_used.c()\nheavily_used.d()\n\
             heavily_used.e()\nheavily_used.f()\nheavily_used.g()\nheavily_used.h()\n\
             heavily_used.i()\nheavily_used.j()\nheavily_used.k()\nheavily_used.l()\n",
        )
        .unwrap();

        let report = analyze(&root, &site_packages, Thresholds::default()).unwrap();
        assert_eq!(report.deps.len(), 3);

        let by_name = |name: &str| report.deps.iter().find(|d| d.display_name == name).unwrap();

        assert_eq!(by_name("unused_dep").verdict, Verdict::Drop);
        assert_eq!(by_name("barely_used").verdict, Verdict::Inline);
        assert_eq!(by_name("heavily_used").verdict, Verdict::Keep);

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn cold_start_zero_overlap_warns() {
        let root = temp_dir("cold-start");
        fs::write(
            root.join("pyproject.toml"),
            "[project]\nname = \"proj\"\ndependencies = [\"requests\"]\n",
        )
        .unwrap();
        // A real venv layout, but with nothing installed in site-packages.
        let python_env = root.join(".venv-empty");
        let site_packages = python_env
            .join("lib")
            .join("python3.11")
            .join("site-packages");
        fs::create_dir_all(&site_packages).unwrap();

        let report = analyze(&root, &python_env, Thresholds::default()).unwrap();
        assert!(
            report
                .warnings
                .iter()
                .any(|w| w.contains("did you mean to install them first"))
        );

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn cold_start_warns_even_when_only_bootstrap_tooling_resolves() {
        let root = temp_dir("cold-start-pip-only");
        fs::write(
            root.join("pyproject.toml"),
            "[project]\nname = \"proj\"\ndependencies = [\"requests\", \"pip\"]\n",
        )
        .unwrap();
        // pip is bundled in every venv by default, but nothing else was ever installed -- the warning must still fire.
        let python_env = root.join(".venv-pip-only");
        let site_packages = python_env
            .join("lib")
            .join("python3.11")
            .join("site-packages");
        let pip_dist_info = site_packages.join("pip-26.2.1.dist-info");
        fs::create_dir_all(&pip_dist_info).unwrap();
        fs::write(pip_dist_info.join("top_level.txt"), "pip").unwrap();

        let report = analyze(&root, &python_env, Thresholds::default()).unwrap();
        assert!(
            report
                .warnings
                .iter()
                .any(|w| w.contains("did you mean to install them first"))
        );

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn missing_manifest_is_an_error() {
        let root = temp_dir("no-manifest");
        let site_packages = root.join(".venv");
        fs::create_dir_all(&site_packages).unwrap();

        let err = analyze(&root, &site_packages, Thresholds::default()).unwrap_err();
        assert!(matches!(err, AnalyzeError::Manifest(_)));

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn missing_python_env_is_an_error() {
        let root = temp_dir("no-python-env");
        fs::write(
            root.join("pyproject.toml"),
            "[project]\nname = \"proj\"\ndependencies = [\"requests\"]\n",
        )
        .unwrap();
        let missing_env = root.join("does-not-exist");

        let err = analyze(&root, &missing_env, Thresholds::default()).unwrap_err();
        assert!(matches!(err, AnalyzeError::NameMap(_)));

        fs::remove_dir_all(&root).unwrap();
    }
}
