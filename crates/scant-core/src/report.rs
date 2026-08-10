//! Renders the flat per-dependency report format locked into README.md:
//! `imports:`/`files:`/`lines:`/`usage:`/`verdict:`, 2-space indent, values
//! column-aligned. Dependencies are grouped by what to do (DROP? / INLINE? /
//! KEEP), not alphabetically, per CLAUDE.md's output rule -- sorted
//! alphabetically only within each group (already the case coming out of
//! `analyze::analyze`, which sorts by normalized name).

use std::fmt::Write as _;

use crate::analyze::{DepReport, Report, UsageBand, Verdict};

const RULE_WIDTH: usize = 66;

pub fn render(report: &Report, project_name: &str) -> String {
    let mut out = String::new();

    let _ = writeln!(
        out,
        "  scant · {project_name}          {deps} deps · {files} files · {elapsed:.2}s",
        deps = report.deps.len(),
        files = report.files_scanned,
        elapsed = report.elapsed.as_secs_f64(),
    );
    let _ = writeln!(out, "  {}", "─".repeat(RULE_WIDTH));

    for warning in &report.warnings {
        let _ = writeln!(out, "\n  NOTE -- {warning}");
    }

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
    let keep: Vec<&DepReport> = report
        .deps
        .iter()
        .filter(|d| d.verdict == Verdict::Keep)
        .collect();

    render_section(&mut out, "DROP?", "never imported", &drop);
    render_section(&mut out, "INLINE?", "one symbol, a few lines", &inline);
    render_section(&mut out, "KEEP", "earning their place", &keep);

    out
}

fn render_section(out: &mut String, header: &str, subtitle: &str, deps: &[&DepReport]) {
    if deps.is_empty() {
        return;
    }
    let _ = writeln!(
        out,
        "\n  {header:<15}{subtitle:<40}{count:>4}",
        count = deps.len()
    );
    for dep in deps {
        render_dep(out, dep);
    }
}

fn render_dep(out: &mut String, dep: &DepReport) {
    let _ = writeln!(out, "\n    {}", dep.display_name);
    let _ = writeln!(out, "      {:<12}{}", "imports:", dep.imports);
    let _ = writeln!(out, "      {:<12}{}", "files:", dep.files);
    let _ = writeln!(out, "      {:<12}{}", "lines:", dep.lines);
    let _ = writeln!(out, "      {:<12}{}", "usage:", usage_label(dep.usage));
    let _ = writeln!(
        out,
        "      {:<12}{}",
        "verdict:",
        verdict_label(dep.verdict)
    );
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
        }
    }

    #[test]
    fn numpy_example_matches_readme_field_alignment() {
        let report = Report {
            manifest_source_label: "pyproject.toml",
            files_scanned: 1,
            elapsed: Duration::from_secs_f64(0.01),
            deps: vec![dep("numpy", Verdict::Inline, UsageBand::Trivial)],
            warnings: vec![],
        };

        let rendered = render(&report, "proj");
        assert!(rendered.contains("    numpy\n"));
        assert!(rendered.contains("      imports:    1\n"));
        assert!(rendered.contains("      files:      1\n"));
        assert!(rendered.contains("      lines:      1\n"));
        assert!(rendered.contains("      usage:      trivial\n"));
        assert!(rendered.contains("      verdict:    inline\n"));
    }

    #[test]
    fn empty_sections_are_omitted() {
        let report = Report {
            manifest_source_label: "pyproject.toml",
            files_scanned: 1,
            elapsed: Duration::from_secs_f64(0.0),
            deps: vec![dep("kept_dep", Verdict::Keep, UsageBand::Heavy)],
            warnings: vec![],
        };

        let rendered = render(&report, "proj");
        assert!(!rendered.contains("DROP?"));
        assert!(!rendered.contains("INLINE?"));
        assert!(rendered.contains("KEEP"));
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

        let rendered = render(&report, "proj");
        assert!(rendered.contains("NOTE -- something worth knowing"));
    }

    #[test]
    fn drop_verdict_shows_usage_none() {
        let report = Report {
            manifest_source_label: "pyproject.toml",
            files_scanned: 1,
            elapsed: Duration::from_secs_f64(0.0),
            deps: vec![DepReport {
                display_name: "bottleneck".to_string(),
                name: PackageName::new("bottleneck".to_string()).unwrap(),
                imports: 0,
                files: 0,
                lines: 0,
                symbols: 0,
                verdict: Verdict::Drop,
                usage: UsageBand::None,
            }],
            warnings: vec![],
        };

        let rendered = render(&report, "proj");
        assert!(rendered.contains("      usage:      none\n"));
        assert!(rendered.contains("      verdict:    drop\n"));
    }

    #[test]
    fn deterministic_snapshot_of_a_small_mixed_report() {
        let report = Report {
            manifest_source_label: "pyproject.toml",
            files_scanned: 39,
            elapsed: Duration::from_secs_f64(0.012),
            deps: vec![
                dep("bottleneck", Verdict::Drop, UsageBand::None),
                dep("colorama", Verdict::Inline, UsageBand::Trivial),
                dep("requests", Verdict::Keep, UsageBand::Heavy),
            ],
            warnings: vec![],
        };

        let rendered = render(&report, "mkdocs");
        insta::assert_snapshot!(rendered);
    }
}
