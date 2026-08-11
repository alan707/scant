# CLAUDE.md

Guidance for Claude Code working in this repository.

## What this is

`scant` is a fast Rust CLI that scans a Python project and finds:
1. **Unused** dependencies — declared but never imported.
2. **Lightly used** dependencies — used so little (one symbol, a couple of lines) that inlining is probably better than carrying the dependency.

Signal 2 is the differentiator. Existing tools (deptry, fawltydeps, pip-check-reqs) do signal 1. **Protect the lightly-used feature** — it is why this project exists.

**Read `plans/PLAN.md` before doing anything.** It is the full specification: phases, acceptance criteria, output design, error catalog, test strategy. This file is the short version plus working rules.

## Current status

Pre-alpha. No code yet. **We are on Phase 0** (scaffold). Do not skip ahead — each phase in `plans/PLAN.md` has acceptance criteria that must pass before the next begins.

## Working rules

- **Work phase by phase.** Stop at the end of each phase and confirm acceptance criteria before continuing.
- **Prefer the narrow correct version over the broad clever one.** Phase 1 supports exactly one manifest format and one repo. That is deliberate.
- **Don't relitigate settled decisions.** The "Non-negotiables" below were decided with evidence (see `plans/PLAN.md` §14). If you think one is wrong, say so explicitly and wait — don't silently do something else.
- **All logic goes in `scant-core`**, testable without a terminal. `scant-cli` only parses args, loads config, calls core, prints, sets exit codes.
- **Never panic on user input.** Panics are for internal bugs only, and should tell the user to file an issue.
- **Don't add `Co-Authored-By: Claude` (or similar) trailers to commit messages.** Author commits as the user only.
- **Commit messages are a single line.** No multi-paragraph bodies.
- **Code comments are a single line.** No multi-line comment blocks or docstrings.

## General working rules

Adapted from [multica-ai/andrej-karpathy-skills](https://github.com/multica-ai/andrej-karpathy-skills/blob/main/CLAUDE.md). These bias toward caution over speed. For trivial tasks, use judgment.

### 1. Think before coding

Don't assume. Don't hide confusion. Surface tradeoffs.

Before implementing:
- State your assumptions explicitly. If uncertain, ask.
- If multiple interpretations exist, present them — don't pick silently.
- If a simpler approach exists, say so. Push back when warranted.
- If something is unclear, stop. Name what's confusing. Ask.

### 2. Simplicity first

Minimum code that solves the problem. Nothing speculative.

- No features beyond what was asked.
- No abstractions for single-use code.
- No "flexibility" or "configurability" that wasn't requested.
- No error handling for impossible scenarios.
- If you write 200 lines and it could be 50, rewrite it.

Ask yourself: "Would a senior engineer say this is overcomplicated?" If yes, simplify.

### 3. Surgical changes

Touch only what you must. Clean up only your own mess.

When editing existing code:
- Don't "improve" adjacent code, comments, or formatting.
- Don't refactor things that aren't broken.
- Match existing style, even if you'd do it differently.
- If you notice unrelated dead code, mention it — don't delete it.

When your changes create orphans:
- Remove imports/variables/functions that YOUR changes made unused.
- Don't remove pre-existing dead code unless asked.

The test: every changed line should trace directly to the user's request.

### 4. Goal-driven execution

Define success criteria. Loop until verified.

Transform tasks into verifiable goals:
- "Add validation" → "Write tests for invalid inputs, then make them pass"
- "Fix the bug" → "Write a test that reproduces it, then make it pass"
- "Refactor X" → "Ensure tests pass before and after"

For multi-step tasks, state a brief plan:
```
1. [Step] → verify: [check]
2. [Step] → verify: [check]
3. [Step] → verify: [check]
```

Strong success criteria let you loop independently. Weak criteria ("make it work") require constant clarification.

**These guidelines are working if:** fewer unnecessary changes in diffs, fewer rewrites due to overcomplication, and clarifying questions come before implementation rather than after mistakes.

## Non-negotiables (decided, with reasons)

1. **Never scan `.venv`/`site-packages`/vendored dirs.** Scanning them reads a dependency's *own internal imports* as project usage. Most damaging bug class in this tool. Hard-prune at the walk level.
2. **Scan tests BY DEFAULT.** Verified on beets: excluding `test/` made `discogs-client` and `reflink` look unused when their only imports live there. Test usage is real usage. Users may exclude tests; we must not do it for them.
3. **Distribution name ≠ import name.** `Pillow`→`PIL`, `PyYAML`→`yaml`, `python-socketio`→`socketio`, `pywin32`→`win32api`. There is no derivation rule — it is data in installed metadata. Read `.dist-info/RECORD` (primary), `top_level.txt` (fast path), `entry_points.txt` (see 4). Heuristic guessing produced a ~20% false-positive rate in testing.
4. **Entry-point deps are NOT unused.** Superset never imports its ~40 SQLAlchemy dialects — they load by connection string via setuptools entry points. Same for pytest/ruff/pre-commit/gunicorn. Read `entry_points.txt`; report these in a separate "registered, not imported" category. A tool that says "delete your Redshift driver" gets uninstalled.
5. **Detect pip-compile output.** A `requirements.txt` may secretly be a lockfile. edx-platform's `base.txt` has 301 entries of which ~240 are transitive. Detect via the `# autogenerated by pip-compile` header and per-entry `# via` annotations (`# via -r foo.in` = direct; `# via somepackage` = transitive). Prefer the sibling `.in` file. Never diff against `poetry.lock`/`uv.lock`/`pylock.toml`.
6. **Only ever flag DIRECTLY DECLARED deps as unused.** Never transitive ones.
7. **Version-keyed stdlib sets for 3.9–3.14.** The stdlib shrank: `distutils`/`imp`/`asynchat` gone in 3.12; `telnetlib`/`cgi`/`crypt` gone in 3.13. One list only ⇒ a 3.9 repo importing `distutils` gets reported as an undeclared dependency. Ship one set per version + `--target-version`.
8. **Excludes must never silently flip a verdict.** If a user exclusion is the *sole* reason a dep reads unused/light, say so and downgrade confidence. Implement by re-scanning excluded paths *only for already-suspect deps* (cheap).
9. **Never show a bare 0–100 score.** Always show the inputs that produced a verdict.

## Performance rules

Target: Superset (1,501 files) in **0.1–0.2s**. The Python prototype does it in 3.0s; we should be 15–30× faster.

- **One parse pass per file.** Extract imports *and* usage sites in the same traversal. This is Ruff's core trick.
- **Parallelize across files** with `rayon`.
- **Prune excluded dirs at the walk level** (`filter_entry`), don't filter after.
- **Expensive-but-kind work runs on the suspect set, not the tree.** Verification, caveats, entry-point lookups touch ~20 flagged deps out of 165 — milliseconds on top of the fast path. This is how we get friendly *and* fast.
- **Don't add Salsa.** It's for keystroke-level incremental recomputation (ty/LSP). A one-shot CLI reads everything once anyway. Revisit only for `--watch`.

## Stack

`ruff_python_parser` + `ruff_python_ast` (git-pinned; not on crates.io as a stable API) · `ignore` (walking) · `rayon` · `clap` · `toml` · `pep508_rs` · `miette` (errors) · `anstream`/`owo-colors` · `insta` (snapshots) · `assert_cmd`/`trycmd` · `criterion`.

Distributed as **maturin-built wheels** so users `pipx install scant` with no Rust toolchain.

## Commands

```bash
cargo build
cargo test
cargo clippy -- -D warnings     # must be clean
cargo fmt --check               # must be clean
cargo insta review              # after intentional output changes
cargo run -- <path>             # run the CLI
```

## Releasing

`CHANGELOG.md` follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions are [PEP 440](https://peps.python.org/pep-0440/).

- Add entries under `## [Unreleased]` as you make changes.
- To cut a release, add a new `## [X.Y.ZaN]` heading directly below `[Unreleased]` (see the `[0.0.1a1]` entry for the pattern) with the relevant changes moved under it. Do this in a normal PR.
- The version fields committed in `Cargo.toml`/`pyproject.toml` are **not** tied to this and are **not** auto-advanced by `release.yml` — bump them separately, in a normal PR, whenever you like. `release.yml` is triggered manually (`workflow_dispatch`, against `main` only) with its own PEP 440 `version` input, and patches those files itself for the build/tag/publish. It never commits back to `main` (the branch ruleset requires every change to go through a passing PR).
- `dry_run` defaults `true` — a real PyPI publish is one-time and irreversible, so flipping it to `false` is a deliberate, explicit action a human takes from the Actions tab.

## Output & error style

**Output** — plain language, light ASCII rules, generous spacing, no heavy box-drawing. Color is additive, never load-bearing (detect TTY; honor `NO_COLOR`). Sort deterministically so snapshots are stable. Group by *what to do* (DROP / INLINE / REGISTERED / KEEP), not alphabetically — a report of 165 deps that isn't triaged is unreadable. Per-dependency detail belongs in `scant explain <dep>`, not the default report.

**Errors** — every message has three parts: *what happened, why, what to do next*, in words a Python user with no Rust knowledge understands. Never surface a stack trace, errno, or parser internals.

❌ `Error: ENOENT pyproject.toml`
✅ `Couldn't find a dependency file here. scant looks for pyproject.toml or requirements.txt. Run it from your project's top folder, or pass the path: scant path/to/project`

**Warnings vs errors:** one unparseable file is a *warning* and analysis continues. Only project-level problems (no manifest, bad config) are errors. Exit codes: `0` clean, `1` findings, `2` operational error.

## Testing

- Unit-test every import form: `import a.b as c`, multiline `from x import (a,\n b)`, relative, `import *`, conditional in try/except, lazy imports inside functions.
- Fixture projects in `tests/fixtures/`, one trait each — see `plans/PLAN.md` §11b.
- **The false-positive taxonomy in `plans/PLAN.md` §11c is real, observed data.** Each row has a named specimen from a real repo. These are regression tests, not hypotheticals.
- `insta` snapshots lock the output formatting.

## Validation repos

- **Phase 1: mkdocs** — small, clean, hand-verifiable. Must find 3 barely-used (`colorama`, `markupsafe`, `mergedeep`) and must NOT flag `pyyaml-env-tag` (it imports as `yaml_env_tag` — proves RECORD resolution works).
- **Phase 2: Apache Superset** — 165 deps, 2,522 files. The showcase and the false-positive stress test. Headline finding: `prophet` (huge, pulls Stan) used on 2 lines in 1 file.
- **Phase 3: edx-platform** — 4,417 files, compiled-manifest specimen.

## Don't

- Don't use regex to find imports. Multiline imports, aliases, conditional imports, and `importlib.import_module` all defeat it.
- Don't derive import names from distribution names by string munging.
- Don't report findings inside excluded paths.
- Don't add features not in the current phase.
- Don't write to `main` without tests passing.
