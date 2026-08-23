//! Single-pass extraction of import bindings and usage sites.
//!
//! One `ruff_python_parser::parse_module` call per file, one `Visitor` walk
//! over the resulting AST, collecting both import bindings and usage sites
//! in the same traversal (CLAUDE.md's performance rule: one parse pass per
//! file). See plans/Phase1.md's `parse.rs` section for the interpretation
//! decisions baked into this module.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use ruff_python_ast::visitor::{self, Visitor};
use ruff_python_ast::{self as ast, Expr, Stmt};
use ruff_text_size::{TextRange, TextSize};

/// Per-(file, import-root) usage record. `lines`/`symbols` come strictly
/// from actual `Name`/`Attribute` usage sites, never from the import
/// statement's own line -- that's what makes `imports:1, files:1, lines:1`
/// self-consistent for a single bare usage.
#[derive(Debug, Clone, Default)]
pub struct ImportRecord {
    /// Count of distinct import *statements* that touched this root in this
    /// file (not aliases: `from x import a, b, c` is 1).
    pub import_statements: u32,
    /// Distinct 1-indexed source lines where the bound name was actually used.
    pub lines: BTreeSet<u32>,
    /// Distinct symbols attributed to this root (see module docs for the
    /// module-import vs. from-import attribution rules).
    pub symbols: BTreeSet<String>,
}

/// Everything extracted from one source file.
#[derive(Debug, Clone, Default)]
pub struct FileUsage {
    pub path: PathBuf,
    /// import root -> usage record, for every non-first-party root touched.
    pub records: HashMap<String, ImportRecord>,
    /// Import roots named in a string literal in this file, mapped to the
    /// first line naming them. Django loads apps, middleware and storage
    /// backends by dotted string from `settings.py` and never imports them,
    /// so a string is the only evidence such a dependency is in use.
    pub string_refs: HashMap<String, u32>,
    /// Import roots pulled in via `from x import *` in this file. A wildcard
    /// import creates no bindings (we can't attribute usage to it), so
    /// `analyze.rs` forces `Verdict::Keep` for any dependency in this set
    /// regardless of the (necessarily incomplete) usage signal above.
    pub wildcard: HashSet<String>,
}

/// What a locally-bound name resolves back to.
#[derive(Debug, Clone)]
enum BindingKind {
    /// Bound via `import x` / `import x as y` / `import x.y.z [as w]`.
    /// The symbol isn't known yet -- it's the first attribute accessed off
    /// the bound name, discovered at each usage site.
    Module,
    /// Bound via `from x import y [as z]`. The symbol is fixed: it's `y`,
    /// the imported name, regardless of the local alias `z`.
    FromImport(String),
}

#[derive(Debug, Clone)]
struct Binding {
    root: String,
    kind: BindingKind,
}

/// Maps byte offsets to 1-indexed line numbers without pulling in a whole
/// source-map crate -- we only need this for usage-site line attribution.
struct LineIndex {
    /// Byte offset of the start of each line; `starts[0] == 0`.
    starts: Vec<u32>,
}

impl LineIndex {
    fn new(source: &str) -> Self {
        let mut starts = vec![0u32];
        for (i, b) in source.bytes().enumerate() {
            if b == b'\n' {
                starts.push((i + 1) as u32);
            }
        }
        LineIndex { starts }
    }

    fn line_number(&self, offset: TextSize) -> u32 {
        let offset: u32 = offset.into();
        match self.starts.binary_search(&offset) {
            Ok(i) => (i as u32) + 1,
            Err(i) => i as u32,
        }
    }
}

struct FileVisitor<'a> {
    first_party: &'a HashSet<String>,
    // Passed in rather than collected wholesale: a project has a few hundred declared import roots and tens of thousands of string literals, so matching as we walk keeps this to the roots that could ever matter.
    declared_roots: &'a HashSet<String>,
    string_refs: HashMap<String, u32>,
    bindings: HashMap<String, Binding>,
    records: HashMap<String, ImportRecord>,
    wildcard: HashSet<String>,
    line_index: LineIndex,
}

fn root_segment(dotted: &str) -> &str {
    dotted.split('.').next().unwrap_or(dotted)
}

impl<'a> FileVisitor<'a> {
    fn handle_import(&mut self, stmt: &ast::StmtImport) {
        let mut roots_touched: HashSet<String> = HashSet::new();
        for alias in &stmt.names {
            let full_name = alias.name.as_str();
            let root = root_segment(full_name).to_string();
            if self.first_party.contains(&root) {
                continue;
            }
            roots_touched.insert(root.clone());
            let bound_name = match &alias.asname {
                Some(id) => id.as_str().to_string(),
                // `import a.b.c` (no alias) binds only the top name `a`.
                None => root.clone(),
            };
            self.bindings.insert(
                bound_name,
                Binding {
                    root,
                    kind: BindingKind::Module,
                },
            );
        }
        for root in roots_touched {
            self.records.entry(root).or_default().import_statements += 1;
        }
    }

    fn handle_import_from(&mut self, stmt: &ast::StmtImportFrom) {
        if stmt.level > 0 {
            // Relative import -- always skipped, no resolution attempted.
            return;
        }
        let Some(module) = &stmt.module else {
            return;
        };
        let root = root_segment(module.as_str()).to_string();
        if self.first_party.contains(&root) {
            return;
        }
        self.records
            .entry(root.clone())
            .or_default()
            .import_statements += 1;

        for alias in &stmt.names {
            let imported_name = alias.name.as_str();
            if imported_name == "*" {
                self.wildcard.insert(root.clone());
                continue;
            }
            let bound_name = match &alias.asname {
                Some(id) => id.as_str().to_string(),
                None => imported_name.to_string(),
            };
            self.bindings.insert(
                bound_name,
                Binding {
                    root: root.clone(),
                    kind: BindingKind::FromImport(imported_name.to_string()),
                },
            );
        }
    }

    fn record_usage(&mut self, root: &str, range: TextRange, symbol: Option<&str>) {
        let line = self.line_index.line_number(range.start());
        let record = self.records.entry(root.to_string()).or_default();
        record.lines.insert(line);
        if let Some(symbol) = symbol {
            record.symbols.insert(symbol.to_string());
        }
    }
}

impl<'a> Visitor<'a> for FileVisitor<'a> {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        match stmt {
            Stmt::Import(import_stmt) => self.handle_import(import_stmt),
            Stmt::ImportFrom(import_from_stmt) => self.handle_import_from(import_from_stmt),
            _ => visitor::walk_stmt(self, stmt),
        }
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        match expr {
            // `a.b.c.d` -> symbol is `b`, the first attribute accessed off
            // the bound name `a` -- found by recursing into nested
            // Attribute chains until `.value` is a bare, bound `Name`.
            Expr::Attribute(attr_expr) => {
                let usage = if let Expr::Name(name_expr) = attr_expr.value.as_ref() {
                    self.bindings
                        .get(name_expr.id.as_str())
                        .map(|binding| match &binding.kind {
                            BindingKind::Module => (
                                binding.root.clone(),
                                Some(attr_expr.attr.as_str().to_string()),
                                attr_expr.range,
                            ),
                            BindingKind::FromImport(symbol) => {
                                (binding.root.clone(), Some(symbol.clone()), name_expr.range)
                            }
                        })
                } else {
                    None
                };
                match usage {
                    Some((root, symbol, range)) => {
                        self.record_usage(&root, range, symbol.as_deref());
                    }
                    None => visitor::walk_expr(self, expr),
                }
            }
            // A bare use with no attribute access (`cb = numpy`) counts
            // toward files/lines but contributes no symbol for module
            // imports -- chosen because it never under-counts usage. For
            // from-imports the symbol is already fixed, so it's recorded
            // even on a bare reference.
            Expr::Name(name_expr) => {
                let usage = self
                    .bindings
                    .get(name_expr.id.as_str())
                    .map(|binding| match &binding.kind {
                        BindingKind::Module => (binding.root.clone(), None),
                        BindingKind::FromImport(symbol) => {
                            (binding.root.clone(), Some(symbol.clone()))
                        }
                    });
                if let Some((root, symbol)) = usage {
                    self.record_usage(&root, name_expr.range, symbol.as_deref());
                }
                visitor::walk_expr(self, expr);
            }
            // A string naming a declared dependency's import root: `"django_prometheus"` in INSTALLED_APPS, `"whitenoise.middleware.WhiteNoiseMiddleware"` in MIDDLEWARE. Recorded separately from real usage -- it never counts toward line or symbol totals, it only answers "is this loaded by name somewhere".
            Expr::StringLiteral(string_expr) => {
                let value = string_expr.value.to_str();
                if is_dotted_identifier(value) {
                    let root = root_segment(value);
                    if self.declared_roots.contains(root) {
                        let line = self.line_index.line_number(string_expr.range.start());
                        self.string_refs.entry(root.to_string()).or_insert(line);
                    }
                }
                visitor::walk_expr(self, expr);
            }
            _ => visitor::walk_expr(self, expr),
        }
    }
}

// Only strings shaped like a module path can be one. Keeps prose, URLs and SQL out: "please install django_prometheus" is not a reference to it.
fn is_dotted_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.split('.').all(|segment| {
            !segment.is_empty()
                && !segment.starts_with(|c: char| c.is_ascii_digit())
                && segment
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
}

/// Extracts import/usage data from already-read source text. Split out from
/// [`analyze_file`] so tests can exercise it without touching disk.
pub fn analyze_source(
    path: &Path,
    source: &str,
    first_party: &HashSet<String>,
    declared_roots: &HashSet<String>,
) -> Option<FileUsage> {
    let parsed = ruff_python_parser::parse_module(source).ok()?;

    let mut file_visitor = FileVisitor {
        first_party,
        declared_roots,
        string_refs: HashMap::new(),
        bindings: HashMap::new(),
        records: HashMap::new(),
        wildcard: HashSet::new(),
        line_index: LineIndex::new(source),
    };

    for stmt in parsed.suite() {
        file_visitor.visit_stmt(stmt);
    }

    Some(FileUsage {
        path: path.to_path_buf(),
        records: file_visitor.records,
        string_refs: file_visitor.string_refs,
        wildcard: file_visitor.wildcard,
    })
}

/// Reads and analyzes one file. Returns `None` on a read or parse failure --
/// the caller treats that as a warning, not an error, and keeps going.
pub fn analyze_file(
    path: &Path,
    first_party: &HashSet<String>,
    declared_roots: &HashSet<String>,
) -> Option<FileUsage> {
    let source = std::fs::read_to_string(path).ok()?;
    analyze_source(path, &source, first_party, declared_roots)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_first_party() -> HashSet<String> {
        HashSet::new()
    }

    fn analyze(source: &str) -> FileUsage {
        analyze_source(
            Path::new("test.py"),
            source,
            &no_first_party(),
            &HashSet::new(),
        )
        .expect("valid python should parse")
    }

    fn analyze_with_first_party(source: &str, first_party: &[&str]) -> FileUsage {
        let fp: HashSet<String> = first_party.iter().map(|s| s.to_string()).collect();
        analyze_source(Path::new("test.py"), source, &fp, &HashSet::new())
            .expect("valid python should parse")
    }

    #[test]
    fn plain_import_and_attribute_usage() {
        let usage = analyze("import numpy\nnumpy.linalg.norm(x)\n");
        let record = &usage.records["numpy"];
        assert_eq!(record.import_statements, 1);
        assert_eq!(record.lines.len(), 1);
        assert_eq!(record.symbols, BTreeSet::from(["linalg".to_string()]));
    }

    #[test]
    fn import_with_alias_and_dotted_path() {
        let usage = analyze("import a.b.c as c\nc.frobnicate()\n");
        let record = &usage.records["a"];
        assert_eq!(record.import_statements, 1);
        assert_eq!(record.lines.len(), 1);
        assert_eq!(record.symbols, BTreeSet::from(["frobnicate".to_string()]));
    }

    #[test]
    fn import_without_alias_dotted_binds_root_only() {
        let usage = analyze("import a.b.c\na.b.c.frobnicate()\n");
        // Bound name is `a`; usage `a.b.c.frobnicate()` -> first attribute is `b`.
        let record = &usage.records["a"];
        assert_eq!(record.import_statements, 1);
        assert_eq!(record.symbols, BTreeSet::from(["b".to_string()]));
    }

    #[test]
    fn multiline_from_import() {
        let usage = analyze("from x import (\n    a,\n    b,\n)\na()\nb()\n");
        let record = &usage.records["x"];
        assert_eq!(record.import_statements, 1);
        assert_eq!(
            record.symbols,
            BTreeSet::from(["a".to_string(), "b".to_string()])
        );
        assert_eq!(record.lines.len(), 2);
    }

    #[test]
    fn from_import_partial_usage_counts_only_used_symbols() {
        let usage = analyze("from x import a, b, c\na()\nb()\n");
        let record = &usage.records["x"];
        assert_eq!(record.import_statements, 1);
        assert_eq!(record.symbols.len(), 2);
    }

    #[test]
    fn from_import_symbol_is_original_name_not_alias() {
        let usage = analyze("from x import y as z\nz()\n");
        let record = &usage.records["x"];
        assert_eq!(record.symbols, BTreeSet::from(["y".to_string()]));
    }

    #[test]
    fn relative_import_is_skipped_without_crashing() {
        let usage = analyze("from . import sibling\nfrom ..pkg import other\nsibling.use()\n");
        assert!(usage.records.is_empty());
    }

    #[test]
    fn wildcard_import_records_no_bindings_but_marks_wildcard() {
        let usage = analyze("from x import *\n");
        assert!(usage.wildcard.contains("x"));
        // `import_statements` still counts the statement itself.
        assert_eq!(usage.records["x"].import_statements, 1);
        assert!(usage.records["x"].symbols.is_empty());
    }

    #[test]
    fn conditional_import_in_try_except() {
        let usage = analyze(
            "try:\n    import simplejson as json\nexcept ImportError:\n    import json\njson.dumps({})\n",
        );
        let record = &usage.records["json"];
        assert_eq!(record.import_statements, 1);
        assert_eq!(record.symbols, BTreeSet::from(["dumps".to_string()]));
    }

    #[test]
    fn lazy_import_inside_function() {
        let usage = analyze("def f():\n    import numpy\n    return numpy.array([1])\n");
        let record = &usage.records["numpy"];
        assert_eq!(record.import_statements, 1);
        assert_eq!(record.symbols, BTreeSet::from(["array".to_string()]));
    }

    #[test]
    fn a_string_naming_a_declared_root_is_recorded_with_its_line() {
        // Django's real shape: the app is listed by name and the middleware by dotted path, and neither is ever imported.
        let source = "INSTALLED_APPS = [\n    \"django_prometheus\",\n]\nMIDDLEWARE = [\n    \"whitenoise.middleware.WhiteNoiseMiddleware\",\n]\n";
        let declared = ["django_prometheus", "whitenoise"]
            .into_iter()
            .map(String::from)
            .collect();
        let usage = analyze_source(
            Path::new("settings.py"),
            source,
            &no_first_party(),
            &declared,
        )
        .unwrap();

        assert_eq!(usage.string_refs.get("django_prometheus"), Some(&2));
        assert_eq!(usage.string_refs.get("whitenoise"), Some(&5));
        // A string is not usage: it must never contribute lines or symbols.
        assert!(usage.records.is_empty());
    }

    #[test]
    fn strings_that_are_not_module_paths_are_ignored() {
        let declared = ["requests", "django_prometheus"]
            .into_iter()
            .map(String::from)
            .collect();
        let source = "a = \"please install django_prometheus\"\nb = \"https://requests.example.com/x\"\nc = \"requests \"\n";
        let usage =
            analyze_source(Path::new("t.py"), source, &no_first_party(), &declared).unwrap();

        assert!(usage.string_refs.is_empty());
    }

    #[test]
    fn a_string_naming_an_undeclared_root_is_not_recorded() {
        let usage = analyze("x = \"colorama.init\"\n");

        assert!(usage.string_refs.is_empty());
    }

    #[test]
    fn stdlib_import_is_recorded_like_any_other_root() {
        // parse.rs doesn't know about stdlib; that's analyze.rs's job when
        // it consults the namemap. Here we just confirm nothing special
        // (and nothing crashes) happens for e.g. `os`.
        let usage = analyze("import os\nos.getcwd()\n");
        assert!(usage.records.contains_key("os"));
    }

    #[test]
    fn first_party_import_is_never_bound_even_if_used() {
        let usage = analyze_with_first_party("import myapp\nmyapp.run()\n", &["myapp"]);
        assert!(usage.records.is_empty());
    }

    #[test]
    fn multi_dist_import_in_one_statement() {
        let usage = analyze("import numpy, pandas\nnumpy.array([1])\npandas.DataFrame()\n");
        assert_eq!(usage.records["numpy"].import_statements, 1);
        assert_eq!(usage.records["pandas"].import_statements, 1);
    }

    #[test]
    fn deep_attribute_chain_attributes_first_segment() {
        let usage = analyze("import a\na.b.c.d.e()\n");
        assert_eq!(
            usage.records["a"].symbols,
            BTreeSet::from(["b".to_string()])
        );
    }

    #[test]
    fn same_line_double_usage_deduped() {
        let usage = analyze("import numpy\nnumpy.array([1]); numpy.array([2])\n");
        assert_eq!(usage.records["numpy"].lines.len(), 1);
    }

    #[test]
    fn bare_name_usage_counts_line_but_no_symbol_for_module_import() {
        let usage = analyze("import numpy\ncb = numpy\n");
        let record = &usage.records["numpy"];
        assert_eq!(record.lines.len(), 1);
        assert!(record.symbols.is_empty());
    }

    #[test]
    fn import_statement_line_itself_never_counted_as_usage() {
        let usage = analyze("import numpy\n");
        let record = &usage.records["numpy"];
        assert_eq!(record.import_statements, 1);
        assert!(record.lines.is_empty());
    }

    #[test]
    fn unparseable_source_returns_none() {
        let result = analyze_source(
            Path::new("bad.py"),
            "def broken(:\n",
            &no_first_party(),
            &HashSet::new(),
        );
        assert!(result.is_none());
    }

    #[test]
    fn missing_file_returns_none_without_panicking() {
        let result = analyze_file(
            Path::new("/nonexistent/path/does-not-exist.py"),
            &no_first_party(),
            &HashSet::new(),
        );
        assert!(result.is_none());
    }
}
