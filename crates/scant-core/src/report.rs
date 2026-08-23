//! Renders the dependency report as a single flat table, one row per
//! dependency, grouped by action (drop / inline / keep) with a blank line
//! between groups -- not a bold section header per group, the `ACTION`
//! column already says it, and a single global column header keeps every
//! row on the same grid (which is also what makes a `WHERE` column
//! workable: it wouldn't be if each group had its own differently-shaped
//! header).

use std::fmt::Write as _;

use owo_colors::OwoColorize;

use crate::analyze::{DepReport, Report, UsageBand, Verdict};

const MIN_NAME_WIDTH: usize = 7; // fits the "PACKAGE" column header itself
const MIN_ACTION_WIDTH: usize = 8; // fits the "ACTION" column header plus a 2-space gutter
const WHERE_MAX_WIDTH: usize = 80;

/// `use_color` embeds real ANSI codes when true -- it does not itself
/// decide whether that's appropriate for the current output destination.
/// That policy call (TTY? `NO_COLOR`? piped to a file?) belongs to
/// `scant-cli`, which writes the result through `anstream`'s auto-detecting
/// stream (strips/translates as needed). Tests always pass `false` so
/// snapshots stay plain, readable text.
pub fn render(report: &Report, project_name: &str, use_color: bool) -> String {
    let mut out = String::new();

    let drop: Vec<&DepReport> = report
        .deps
        .iter()
        .filter(|d| d.verdict == Verdict::Drop)
        .collect();
    let inline: Vec<&DepReport> = report
        .deps
        .iter()
        .filter(|d| d.verdict == Verdict::Inline)
        .collect();
    let registered: Vec<&DepReport> = report
        .deps
        .iter()
        .filter(|d| d.verdict == Verdict::Registered)
        .collect();
    let unknown: Vec<&DepReport> = report
        .deps
        .iter()
        .filter(|d| d.verdict == Verdict::Unknown)
        .collect();
    let keep: Vec<&DepReport> = report
        .deps
        .iter()
        .filter(|d| d.verdict == Verdict::Keep)
        .collect();

    let _ = writeln!(
        out,
        "{project_name} -- {deps} packages declared, {files} files read, {elapsed:.1}s",
        deps = report.deps.len(),
        files = report.files_scanned,
        elapsed = report.elapsed.as_secs_f64(),
    );
    // "registered" is omitted at zero -- it's a reassurance category, and naming it on every clean project is noise.
    let registered_note = match registered.len() {
        0 => String::new(),
        n => format!(", registered {n}"),
    };
    let unknown_note = match unknown.len() {
        0 => String::new(),
        n => format!(", unknown {n}"),
    };
    let _ = writeln!(
        out,
        "Plan: drop {drop}, inline {inline}{registered_note}{unknown_note}, keep {keep}.",
        drop = drop.len(),
        inline = inline.len(),
        keep = keep.len(),
    );

    for warning in &report.warnings {
        let _ = writeln!(out, "\nNOTE -- {warning}");
    }

    if report.deps.is_empty() {
        return out;
    }
    out.push('\n');

    // Sized to the widest verdict actually present, the same way name_width is -- "registered" is long and most projects have none, so a fixed width would tax every report for a column they never show.
    let action_width = report
        .deps
        .iter()
        .map(|d| verdict_label(d.verdict).len() + 2)
        .max()
        .unwrap_or(MIN_ACTION_WIDTH)
        .max(MIN_ACTION_WIDTH);

    let name_width = report
        .deps
        .iter()
        .map(|d| d.display_name.len())
        .max()
        .unwrap_or(MIN_NAME_WIDTH)
        .max(MIN_NAME_WIDTH);

    let _ = writeln!(
        out,
        "  {:<action_width$}{:<name_width$}  {:>4}  {:>5}  {:<8}  WHERE",
        "ACTION",
        "PACKAGE",
        "USES",
        "LINES",
        "USE",
        action_width = action_width,
        name_width = name_width
    );

    let mut first_group = true;
    for group in [&drop, &inline, &registered, &unknown, &keep] {
        if group.is_empty() {
            continue;
        }
        if !first_group {
            out.push('\n');
        }
        first_group = false;
        for dep in group {
            render_row(&mut out, dep, action_width, name_width, use_color);
        }
    }

    out
}

fn render_row(
    out: &mut String,
    dep: &DepReport,
    action_width: usize,
    name_width: usize,
    use_color: bool,
) {
    let label = verdict_label(dep.verdict);
    // Pad on the *plain* word first, then color it -- ANSI escape bytes
    // count toward a string's length, so `{:<N}` on an already-colored
    // string would under-pad. Padding as a separate literal sidesteps that.
    let padding = " ".repeat(action_width.saturating_sub(label.len()));
    let action_cell = if use_color {
        match dep.verdict {
            Verdict::Drop => format!("{}{padding}", label.red()),
            Verdict::Inline => format!("{}{padding}", label.yellow()),
            // Registered is informational, never an action -- dim keeps it visibly subordinate to the three real verdicts.
            Verdict::Registered => format!("{}{padding}", label.dimmed()),
            // Unknown is an absence of a verdict, not a soft one -- dim keeps it from reading as an action.
            Verdict::Unknown => format!("{}{padding}", label.dimmed()),
            Verdict::Keep => format!("{}{padding}", label.green()),
        }
    } else {
        format!("{label}{padding}")
    };

    let where_text = where_column(dep);
    let where_cell = if use_color {
        // Both real paths and the "--" elision marker get the same plain
        // "dim" attribute (ANSI 2) -- no separate hue needed for either;
        // dim is the one style attribute that reliably reads correctly
        // across light/dark/Solarized-style terminal themes.
        where_text.dimmed().to_string()
    } else {
        where_text
    };

    let _ = writeln!(
        out,
        "  {action_cell}{:<name_width$}  {:>4}  {:>5}  {:<8}  {where_cell}",
        dep.display_name,
        dep.imports,
        dep.lines,
        usage_label(dep.usage),
        name_width = name_width
    );
}

/// `drop` never has a location (nothing to point at). `inline` shows an
/// exact `path:line` -- usage is small enough that "here's the line" is
/// genuinely actionable. `keep` shows just the first file, folding any
/// additional files into a `+N files` suffix -- usage is spread widely
/// enough that one exact line wouldn't tell the whole story anyway.
fn where_column(dep: &DepReport) -> String {
    // Registered has no usage to point at, so the WHERE column carries the mechanism that loads it instead.
    if dep.verdict == Verdict::Registered {
        let evidence = dep.registration.as_deref().unwrap_or("--");
        return truncate_front(evidence, WHERE_MAX_WIDTH);
    }
    // The marker is the whole reason there's no verdict, so it belongs in the report rather than a footnote.
    if dep.verdict == Verdict::Unknown {
        let reason = dep.unknown_reason.as_deref().unwrap_or("--");
        return truncate_front(reason, WHERE_MAX_WIDTH);
    }
    if dep.locations.is_empty() {
        return "--".to_string();
    }
    let (first_path, first_line) = &dep.locations[0];
    let raw = match dep.verdict {
        Verdict::Drop | Verdict::Registered | Verdict::Unknown => return "--".to_string(),
        Verdict::Inline => format!("{first_path}:{first_line}"),
        Verdict::Keep => {
            if dep.locations.len() == 1 {
                first_path.clone()
            } else {
                let extra = dep.locations.len() - 1;
                let unit = if extra == 1 { "file" } else { "files" };
                format!("{first_path} +{extra} {unit}")
            }
        }
    };
    truncate_front(&raw, WHERE_MAX_WIDTH)
}

/// Truncates from the front, keeping the tail (filename and immediate
/// parent) intact -- that's the most useful part of a path, and a fixed
/// character budget keeps output deterministic (no terminal-width sensing,
/// which would make `insta` snapshots environment-dependent).
fn truncate_front(s: &str, max: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max {
        return s.to_string();
    }
    const ELLIPSIS: &str = "...";
    let keep = max.saturating_sub(ELLIPSIS.chars().count());
    let tail: String = s
        .chars()
        .rev()
        .take(keep)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{ELLIPSIS}{tail}")
}

fn usage_label(band: UsageBand) -> &'static str {
    match band {
        UsageBand::None => "none",
        UsageBand::Trivial => "trivial",
        UsageBand::Light => "light",
        UsageBand::Moderate => "moderate",
        UsageBand::Heavy => "heavy",
    }
}

fn verdict_label(verdict: Verdict) -> &'static str {
    match verdict {
        Verdict::Drop => "drop",
        Verdict::Inline => "inline",
        Verdict::Registered => "registered",
        Verdict::Unknown => "unknown",
        Verdict::Keep => "keep",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pep508_rs::PackageName;
    use std::time::Duration;

    fn dep(name: &str, verdict: Verdict, usage: UsageBand) -> DepReport {
        DepReport {
            display_name: name.to_string(),
            name: PackageName::new(name.to_string()).unwrap(),
            imports: 1,
            files: 1,
            lines: 1,
            symbols: 1,
            verdict,
            usage,
            locations: vec![],
            registration: None,
            unknown_reason: None,
        }
    }

    fn registered_dep(name: &str, evidence: &str) -> DepReport {
        DepReport {
            imports: 0,
            files: 0,
            lines: 0,
            symbols: 0,
            registration: Some(evidence.to_string()),
            ..dep(name, Verdict::Registered, UsageBand::None)
        }
    }

    #[test]
    fn registered_row_shows_the_loading_mechanism_and_widens_the_action_column() {
        let report = Report {
            manifest_source_label: "pyproject.toml",
            files_scanned: 1,
            elapsed: Duration::from_millis(0),
            deps: vec![
                dep("colorama", Verdict::Inline, UsageBand::Trivial),
                registered_dep("sqlalchemy-redshift", "sqlalchemy.dialects"),
            ],
            warnings: vec![],
        };

        let rendered = render(&report, "superset", false);

        // The mechanism goes in WHERE, so the verdict is never a bare assertion.
        assert!(rendered.contains("registered  sqlalchemy-redshift"));
        assert!(rendered.contains("sqlalchemy.dialects"));
        // Present-but-nonzero registered count is named in the plan line.
        assert!(rendered.contains("Plan: drop 0, inline 1, registered 1, keep 0."));
        // Every row shares one grid, so the inline row widens to match.
        assert!(rendered.contains("  inline      colorama"));
    }

    #[test]
    fn registered_count_is_omitted_when_zero() {
        let report = Report {
            manifest_source_label: "pyproject.toml",
            files_scanned: 1,
            elapsed: Duration::from_millis(0),
            deps: vec![dep("requests", Verdict::Keep, UsageBand::Heavy)],
            warnings: vec![],
        };

        let rendered = render(&report, "proj", false);
        assert!(rendered.contains("Plan: drop 0, inline 0, keep 1."));
        assert!(!rendered.contains("registered"));
    }

    #[test]
    fn numpy_example_renders_as_a_table_row() {
        let report = Report {
            manifest_source_label: "pyproject.toml",
            files_scanned: 1,
            elapsed: Duration::from_secs_f64(0.01),
            deps: vec![dep("numpy", Verdict::Inline, UsageBand::Trivial)],
            warnings: vec![],
        };

        let rendered = render(&report, "proj", false);
        assert!(rendered.contains("Plan: drop 0, inline 1, keep 0."));
        assert!(rendered.contains("ACTION"));
        assert!(rendered.contains("PACKAGE"));
        assert!(rendered.contains("USES"));
        assert!(rendered.contains("LINES"));
        assert!(rendered.contains("USE"));
        assert!(rendered.contains("WHERE"));
        assert!(rendered.contains("inline"));
        assert!(rendered.contains("numpy"));
        assert!(rendered.contains("trivial"));
        assert!(!rendered.contains('?'));
    }

    #[test]
    fn empty_report_still_shows_plan_line() {
        let report = Report {
            manifest_source_label: "pyproject.toml",
            files_scanned: 1,
            elapsed: Duration::from_secs_f64(0.0),
            deps: vec![],
            warnings: vec![],
        };

        let rendered = render(&report, "proj", false);
        assert!(rendered.contains("Plan: drop 0, inline 0, keep 0."));
    }

    #[test]
    fn warnings_are_rendered() {
        let report = Report {
            manifest_source_label: "requirements.txt",
            files_scanned: 0,
            elapsed: Duration::from_secs_f64(0.0),
            deps: vec![],
            warnings: vec!["something worth knowing".to_string()],
        };

        let rendered = render(&report, "proj", false);
        assert!(rendered.contains("NOTE -- something worth knowing"));
    }

    #[test]
    fn drop_has_no_location() {
        let report = Report {
            manifest_source_label: "pyproject.toml",
            files_scanned: 1,
            elapsed: Duration::from_secs_f64(0.0),
            deps: vec![dep("bottleneck", Verdict::Drop, UsageBand::None)],
            warnings: vec![],
        };

        let rendered = render(&report, "proj", false);
        assert!(rendered.contains("bottleneck"));
        assert!(rendered.contains("--"));
    }

    #[test]
    fn inline_location_shows_exact_line() {
        let mut d = dep("markupsafe", Verdict::Inline, UsageBand::Trivial);
        d.locations = vec![("mkdocs/utils.py".to_string(), 41)];
        let report = Report {
            manifest_source_label: "pyproject.toml",
            files_scanned: 1,
            elapsed: Duration::from_secs_f64(0.0),
            deps: vec![d],
            warnings: vec![],
        };

        let rendered = render(&report, "proj", false);
        assert!(rendered.contains("mkdocs/utils.py:41"));
    }

    #[test]
    fn keep_with_one_file_shows_bare_path_no_line() {
        let mut d = dep("watchdog", Verdict::Keep, UsageBand::Light);
        d.locations = vec![("mkdocs/livereload/__init__.py".to_string(), 12)];
        let report = Report {
            manifest_source_label: "pyproject.toml",
            files_scanned: 1,
            elapsed: Duration::from_secs_f64(0.0),
            deps: vec![d],
            warnings: vec![],
        };

        let rendered = render(&report, "proj", false);
        assert!(rendered.contains("mkdocs/livereload/__init__.py"));
        assert!(!rendered.contains("__init__.py:12"));
    }

    #[test]
    fn keep_with_multiple_files_folds_the_rest_into_a_count() {
        let mut d = dep("click", Verdict::Keep, UsageBand::Heavy);
        d.locations = vec![
            ("mkdocs/__main__.py".to_string(), 5),
            ("mkdocs/commands/build.py".to_string(), 10),
            ("mkdocs/commands/serve.py".to_string(), 3),
            ("mkdocs/plugins.py".to_string(), 88),
        ];
        let report = Report {
            manifest_source_label: "pyproject.toml",
            files_scanned: 1,
            elapsed: Duration::from_secs_f64(0.0),
            deps: vec![d],
            warnings: vec![],
        };

        let rendered = render(&report, "proj", false);
        assert!(rendered.contains("mkdocs/__main__.py +3 files"));
    }

    #[test]
    fn long_path_is_truncated_from_the_front_deterministically() {
        assert_eq!(
            truncate_front("a/very/deeply/nested/package/subdir/module/file.py:123", 20),
            "...odule/file.py:123"
        );
        assert_eq!(truncate_front("short.py:1", 20), "short.py:1");
    }

    #[test]
    fn deterministic_snapshot_of_a_small_mixed_report() {
        let mut inline_dep = dep("colorama", Verdict::Inline, UsageBand::Trivial);
        inline_dep.locations = vec![("mkdocs/utils.py".to_string(), 7)];
        let mut keep_dep = dep("requests", Verdict::Keep, UsageBand::Heavy);
        keep_dep.locations = vec![
            ("mkdocs/api.py".to_string(), 3),
            ("mkdocs/client.py".to_string(), 40),
        ];

        let report = Report {
            manifest_source_label: "pyproject.toml",
            files_scanned: 39,
            elapsed: Duration::from_secs_f64(0.012),
            deps: vec![
                dep("bottleneck", Verdict::Drop, UsageBand::None),
                inline_dep,
                keep_dep,
            ],
            warnings: vec![],
        };

        let rendered = render(&report, "mkdocs", false);
        insta::assert_snapshot!(rendered);
    }
}
