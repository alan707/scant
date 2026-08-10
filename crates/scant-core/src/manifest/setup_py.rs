//! Best-effort `setup.py` parsing -- **never executes the file.** Parses via
//! `ruff_python_parser`, walks the AST looking for a call whose callee is
//! `setup` or `X.setup` (`setuptools.setup`) with an `install_requires=`
//! keyword argument. If that argument's value is a literal list/tuple of
//! string literals, extract them as PEP 508 specs.
//!
//! If it's anything else (a variable, `open('requirements.txt').read()...`,
//! a conditional expression -- all real, observed patterns), or if the file
//! doesn't even parse as valid Python, we skip gracefully and record a
//! warning rather than silently reporting zero/partial deps as if that were
//! the whole truth. This warning-not-silent-gap behavior is load-bearing:
//! per plans/Phase1.md judgment call #5, both a hard parse failure and a
//! non-literal `install_requires` degrade to "warn, contribute zero deps,
//! keep going" -- never a crash, never a [`super::ManifestError`].

use std::path::Path;

use ruff_python_ast::visitor::{self, Visitor};
use ruff_python_ast::{self as ast, Expr};

use super::{DetectorResult, parse_requirement};

pub(crate) fn load(root: &Path) -> Option<DetectorResult> {
    let path = root.join("setup.py");
    if !path.is_file() {
        return None;
    }

    let mut warnings = Vec::new();

    let Ok(source) = std::fs::read_to_string(&path) else {
        warnings.push(format!("Couldn't read '{}'.", path.display()));
        return Some(DetectorResult {
            path,
            dependencies: Vec::new(),
            warnings,
        });
    };

    let Ok(parsed) = ruff_python_parser::parse_module(&source) else {
        warnings.push(format!(
            "Found setup.py but couldn't parse it as valid Python -- dependencies \
             declared there may be missing from this report: {}",
            path.display()
        ));
        return Some(DetectorResult {
            path,
            dependencies: Vec::new(),
            warnings,
        });
    };

    let mut finder = SetupCallFinder {
        found_call: false,
        install_requires: None,
    };
    for stmt in parsed.suite() {
        finder.visit_stmt(stmt);
    }

    let mut dependencies = Vec::new();
    match finder.install_requires {
        Some(items) => {
            for raw in items {
                match parse_requirement(&raw) {
                    Ok(dep) => dependencies.push(dep),
                    Err(_) => warnings.push(format!(
                        "Couldn't parse a dependency in '{}': '{raw}' -- skipped it.",
                        path.display()
                    )),
                }
            }
        }
        None if finder.found_call => {
            warnings.push(format!(
                "Found setup.py but couldn't statically determine install_requires \
                 (it isn't a literal list) -- dependencies declared there may be \
                 missing from this report: {}",
                path.display()
            ));
        }
        None => {
            // No setup()/X.setup() call found at all -- not a setuptools-style
            // setup.py scant can read; nothing to warn about beyond zero deps.
        }
    }

    Some(DetectorResult {
        path,
        dependencies,
        warnings,
    })
}

struct SetupCallFinder {
    found_call: bool,
    install_requires: Option<Vec<String>>,
}

impl<'a> Visitor<'a> for SetupCallFinder {
    fn visit_expr(&mut self, expr: &'a Expr) {
        if let Expr::Call(call) = expr
            && is_setup_call(&call.func)
        {
            self.found_call = true;
            if let Some(keyword) = find_keyword(&call.arguments, "install_requires") {
                self.install_requires = extract_string_list(&keyword.value);
            }
        }
        visitor::walk_expr(self, expr);
    }
}

fn is_setup_call(func: &Expr) -> bool {
    match func {
        Expr::Name(name) => name.id.as_str() == "setup",
        Expr::Attribute(attr) => attr.attr.as_str() == "setup",
        _ => false,
    }
}

fn find_keyword<'a>(arguments: &'a ast::Arguments, name: &str) -> Option<&'a ast::Keyword> {
    arguments
        .keywords
        .iter()
        .find(|kw| kw.arg.as_ref().is_some_and(|arg| arg.as_str() == name))
}

/// Extracts a literal `[...]`/`(...)` of plain string literals. Returns
/// `None` (never partial/best-guess data) for anything else -- a variable
/// reference, a function call, a comprehension, or a list containing even
/// one non-literal element.
fn extract_string_list(expr: &Expr) -> Option<Vec<String>> {
    let elts: &[Expr] = match expr {
        Expr::List(list) => &list.elts,
        Expr::Tuple(tuple) => &tuple.elts,
        _ => return None,
    };

    let mut items = Vec::with_capacity(elts.len());
    for elt in elts {
        match elt {
            Expr::StringLiteral(s) => items.push(s.value.to_string()),
            _ => return None,
        }
    }
    Some(items)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("scant-setup-py-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn missing_file_returns_none() {
        let dir = temp_dir("missing");
        assert!(load(&dir).is_none());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn literal_install_requires_extracted() {
        let dir = temp_dir("literal");
        fs::write(
            dir.join("setup.py"),
            "from setuptools import setup\n\nsetup(\n    name='x',\n    install_requires=['requests>=2.0', 'click'],\n)\n",
        )
        .unwrap();

        let result = load(&dir).unwrap();
        let names: Vec<_> = result
            .dependencies
            .iter()
            .map(|d| d.display_name.clone())
            .collect();
        assert_eq!(names, vec!["requests", "click"]);
        assert!(result.warnings.is_empty());

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn setuptools_attribute_form_call_recognized() {
        let dir = temp_dir("attribute-form");
        fs::write(
            dir.join("setup.py"),
            "import setuptools\n\nsetuptools.setup(\n    install_requires=['flask'],\n)\n",
        )
        .unwrap();

        let result = load(&dir).unwrap();
        assert_eq!(result.dependencies.len(), 1);
        assert_eq!(result.dependencies[0].display_name, "flask");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn call_nested_under_main_guard_is_still_found() {
        let dir = temp_dir("main-guard");
        fs::write(
            dir.join("setup.py"),
            "from setuptools import setup\n\nif __name__ == '__main__':\n    setup(install_requires=['requests'])\n",
        )
        .unwrap();

        let result = load(&dir).unwrap();
        assert_eq!(result.dependencies.len(), 1);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn non_literal_install_requires_warns_and_yields_zero_deps() {
        let dir = temp_dir("non-literal");
        fs::write(
            dir.join("setup.py"),
            "from setuptools import setup\n\nreqs = open('requirements.txt').read().splitlines()\nsetup(install_requires=reqs)\n",
        )
        .unwrap();

        let result = load(&dir).unwrap();
        assert!(result.dependencies.is_empty());
        assert_eq!(result.warnings.len(), 1);
        assert!(result.warnings[0].contains("couldn't statically determine"));

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn hard_parse_failure_warns_never_crashes() {
        let dir = temp_dir("parse-failure");
        fs::write(dir.join("setup.py"), "def broken(:\n").unwrap();

        let result = load(&dir).unwrap();
        assert!(result.dependencies.is_empty());
        assert_eq!(result.warnings.len(), 1);
        assert!(result.warnings[0].contains("couldn't parse it"));

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn no_setup_call_at_all_is_zero_deps_no_warning() {
        let dir = temp_dir("no-call");
        fs::write(dir.join("setup.py"), "print('hello')\n").unwrap();

        let result = load(&dir).unwrap();
        assert!(result.dependencies.is_empty());
        assert!(result.warnings.is_empty());

        fs::remove_dir_all(&dir).unwrap();
    }
}
