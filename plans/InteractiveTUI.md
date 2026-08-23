# Interactive dependency-peeking TUI, dive-style

Tracks [issue #34](https://github.com/alan707/scant/issues/34).

## Context

`scant`'s static report tells you *that* `prophet` is used on 2 lines and *where*, but acting on it means reading a path out of the terminal and opening it yourself. On Superset's 165-row report that friction is the difference between a report you skim and one you act on.

Issue #34 framed this as k9s-style navigation. **[`wagoodman/dive`](https://github.com/wagoodman/dive) (MIT, Go) is the better model**, for a reason beyond layout: dive's organizing idea is *waste*. It puts a size on every layer, lets you peek into what that layer actually contains, and totals up "potential wasted space" at the bottom. That is precisely scant's thesis — a 38 MB dependency earning its keep on one line is wasted space — and scant has never quantified it.

So this adopts dive's structure: **select on the left, peek on the right, totals underneath.** The left column stacks a dependency list (dive's *Layers*) over a details panel (*Layer Details*) over project totals (*Image Details*); the right column is a navigable usage tree (*Current Layer Contents*).

Preserved deliberately from the design discussion: for an `inline` dependency, the details panel shows **the symbol's definition inside the installed package** — the code you'd actually copy — so "inline this" becomes mechanical rather than a research task.

dive is MIT and written in Go, so this is **interaction-model inspiration only**, no code reuse. Rust stack: `ratatui 0.30.2` + `crossterm 0.29.0` (verified current on crates.io).

## Prerequisite state

Written against `main` @ `55ecef3`. Two things landed after this design was first sketched and are now assumed:

- **Entry-point detection** (`225bcc2`) — non-negotiable #4 is implemented. `NameMap` carries `dist_entry_points` with `entry_points_for()`, reading both `entry_points.txt` (what a package *declares*) and RECORD-installed `bin`/`Scripts` commands (what actually got *installed* — the only signal for maturin-built tools like ruff and uv that ship a prebuilt binary and declare no `console_scripts`).
- **`Verdict` is now five-way**: `Drop`, `Inline`, `Registered`, `Unknown`, `Keep`. `DepReport` already carries `registration: Option<String>` (Registered only) and `unknown_reason: Option<String>` (Unknown only). Both are free inputs for the TUI — no core change needed for either.

`report.rs` renders `Registered` and `Unknown` dimmed, deliberately subordinate to the three actionable verdicts. **The TUI must mirror that palette** so the two views read as one product.

## Target UI

```
┌─ Dependencies ─────────────────────────┐┌─ Used here · superset/…/prophet.py ───┐
│ Action     Size    Uses Lines Package  ││   64 │     df = df.rename(columns=…)  │
│ drop       2.1 MB     0     0 babel    ││   65 │                                │
│>inline     38 MB      1     1 prophet  ││   66 │     model = Prophet(**args)  ← │
│ inline     118 kB     1     1 polyline ││   67 │     if weekly_seasonality:     │
│ registered 4.2 MB     0     0 gunicorn │└───────────────────────────────────────┘
│ unknown    —          0     0 pywin32  │┌─ Defined in · prophet/forecaster.py ──┐
│ keep       6.4 MB    14    62 click    ││   38 │ class Prophet(object):         │
└────────────────────────────────────────┘│   39 │     """Prophet forecaster.     │
┌─ Dependency details ───────────────────┐│   40 │                                │
│ prophet 1.1.5 · 38 MB · 1 file, 1 line ││   41 │     def __init__(self, …):     │
│ symbol: Prophet                        ││   42 │         self.growth = growth   │
└────────────────────────────────────────┘│   43 │         …                      │
┌─ Project totals ───────────────────────┐│                                       │
│ 165 declared · 1.4 GB installed        ││                                       │
│ droppable   78 deps · 310 MB           ││                                       │
│ inlineable  23 deps · 195 MB           ││                                       │
└────────────────────────────────────────┘└───────────────────────────────────────┘
 tab switch pane · j/k move · space expand · / filter · ? help · q quit
```

*(Sizes illustrative — not measured.)*

The right column is the heart of it: **the lines you'd delete, directly above the code you'd copy.** That pairing is what turns "inline this dependency" from a research task into a mechanical one.

### Right column, per verdict

Both right panes are **always read-only**. All five verdicts need a design; three have no usage to show, and peeking should answer the question that verdict actually raises.

| Verdict | Top-right | Bottom-right |
|---|---|---|
| `inline` | Usage site source, centered on the line | Definition source in site-packages — the code you'd copy. **The headline case.** |
| `keep` | Usage tree across files (dive-style) | Source of the selected file from that tree |
| `drop` | `METADATA` — name, version, summary | Installed file tree, so "safe to delete?" is answerable in place |
| `registered` | *How it loads without being imported*: entry-point group or console script from `registration`, plus installed commands | Installed file tree |
| `unknown` | `unknown_reason` plus what scant checked — answers "why couldn't you tell me?" | — (single full-height pane) |

`inline` needs no usage tree: an `inline` verdict *means* usage under the thresholds (≤3 lines in ≤2 files by default), so there is essentially always exactly one usage site. The tree only earns its space for `keep`.

## The size dimension (new signal)

**Nearly free.** `RECORD` is `path,hash,size` and `namemap.rs`'s RECORD walk already parses every line — for import roots, and now for installed commands too — while discarding fields 2–3. Summing field 3 per distribution gives installed size. Verified live against a real `requests-2.32.5.dist-info/RECORD`: 24 files, 202,506 bytes.

Three honesty constraints to encode, not paper over:

- The sum is a **close approximation, biased low** — `RECORD` lists wheel contents, so post-install `.pyc` files aren't counted, and `RECORD`'s own row has an empty size field.
- **Editable installs (`pip install -e .`) have essentially no `RECORD` payload**, so size reads as ~0. Show `—`, never a misleading `0 B`.
- **Direct size only, never transitive.** `prophet` drags in Stan via `cmdstanpy`, which likely dwarfs prophet itself — so a bare "38 MB" *understates* the real cost. True transitive weight needs a dependency graph scant doesn't build, and non-negotiable #6 forbids attributing transitive deps anyway. Label the column unambiguously as direct installed size.

**Flagged tension with non-negotiable #9** ("never show a bare 0–100 score; always show the inputs"). dive's headline is an efficiency *percentage*. This plan deliberately ships **no** score — the totals panel reports concrete counts and byte figures only (`droppable 78 deps · 310 MB`), with the per-dependency inputs always visible directly above it. If a single score is wanted later, #9 requires the breakdown ship alongside it.

## Required `scant-core` changes

All additive — no restructuring, no signature changes to `analyze()` or `report::render()`, existing insta snapshots stay valid.

In `crates/scant-core/src/namemap.rs`:

1. **Capture size and file list during the existing RECORD walk.** That walk already runs for import roots and installed commands; have it also sum field 3 and retain the file list. Expose per-distribution installed size and files on `NameMap`, alongside the existing `entry_points_for()`.

In `crates/scant-core/src/analyze.rs` — `build_dep_report` computes two things it then throws away:

2. **Symbol names are discarded.** It collects real names into a `HashSet<&str>` and keeps only the count. The details panel needs the *names* to look up definitions. Add `pub symbol_names: Vec<String>` (sorted); keep the existing `symbols` count so `report.rs` is untouched.
3. **Import roots aren't exposed.** `import_roots` is local but the definition finder needs it to know which directory to search. Add `pub import_roots: Vec<String>`.
4. **Plus** `pub installed_bytes: Option<u64>` on `DepReport`, and `pub site_packages: PathBuf` on `Report` (currently resolved inside `analyze()` and dropped).

`registration` and `unknown_reason` already exist — nothing needed for the Registered/Unknown panels.

## New: `crates/scant-core/src/defsite.rs`

A miniature static "go to definition" scoped to installed packages — the one genuinely novel piece. Lives in core per CLAUDE.md's "all logic in scant-core, testable without a terminal", unit-tested against synthetic site-packages fixtures using the same `temp_dir`/`write_top_level` helper pattern `namemap.rs` tests already use.

```rust
pub struct DefSite { pub path: PathBuf, pub line: u32, pub kind: DefKind }  // Class | Function | Assignment
pub fn find(site_packages: &Path, import_root: &str, symbol: &str) -> Option<DefSite>
```

Reuse the `ruff_python_parser` + `Visitor` pattern already in `parse.rs`:

1. Parse `<site_packages>/<import_root>/__init__.py` (or `<root>.py` for single-module dists).
2. Top-level `ClassDef`/`FunctionDef`/assignment binding `symbol` → done. *(Validated live: `bs4.BeautifulSoup` resolves this way.)*
3. Else follow a re-export and recurse. **Must handle both spellings** — relative (`from .forecaster import Prophet`) *and* absolute self-referential (`from prophet.forecaster import Prophet`), which is equally common and which a relative-only matcher would silently miss. Cap at 3 hops, visited-set cycle guard. *(Validated as necessary: `requests.get` lives in `requests/api.py`, only re-exported from `__init__.py`.)*
4. No hit → `None`; the panel shows a plain "couldn't locate `X`" note. Graceful, never an error.

**Known-unresolvable**, all landing on that fallback rather than a wrong answer: `from .mod import *`, module-level `__getattr__` lazy loading (PEP 562), C extensions with no Python source, dynamically-built `__all__`, namespace packages (a documented Phase 1 gap). Structurally acceptable because fidelity is worst on large compiled libraries — exactly the ones nobody inlines, which classify as `keep`. The packages `inline` actually surfaces (`mergedeep`, `polyline`, `pygeohash`, `six`) sit on the easy path.

**Deliberately static — never imports the package.** Shelling out to `python -c "import pkg; inspect.getsourcefile(...)"` would handle `__all__` and C extensions for free, but executes arbitrary third-party code at import time, can be slow (pandas ≈1s), and crashes on import-time side effects. That's a real posture change from the existing `sysconfig` call (stdlib only) and contradicts the static-only ethos already applied to `setup.py`. Revisit as an opt-in flag only if static resolution proves too lossy.

**Laziness is load-bearing:** resolve only for the selected row, memoized per dependency — never for all 165 up front. Keeps CLAUDE.md's "expensive work runs on the suspect set, not the tree" intact.

## Read-only by design

**scant never modifies anything.** It reads a report, and it reads source files to display them. There is no editing in the panes, no `$EDITOR` handoff, no subprocess spawned to open anything. The tool shows you the lines you'd delete next to the code you'd copy; making the change is your business, in your own editor, in your own window.

This is a scope decision, not a limitation to work around, and it buys a lot:

- **No terminal emulation.** An editable pane means embedding a PTY (`tui-term` over `portable-pty` + `vt100`), which is where this class of project reliably gets stuck: vim drives its own alternate screen (nested alt-screens), `Esc` disambiguation against escape sequences is unsolved in practice and vim leans on `Esc` constantly, plus `SIGWINCH` propagation, bracketed paste, mouse and true-color passthrough. dive doesn't do it; neither does k9s or lazygit.
- **No handoff machinery either.** No per-editor argv translation (every editor spells "open at line N" differently), no `$EDITOR` word-splitting, no suspend/restore dance around the alternate screen and raw mode.
- **The keymap stays global.** An editable pane forces a focus model where `q` can't mean quit and `/` can't mean filter.
- **No footgun on the definition pane.** It points into site-packages; an editable pane there is one keystroke from editing an installed package, a change that "works" locally and silently evaporates on the next `pip install`.
- **The whole UI stays snapshot-testable** via `TestBackend`, which a live PTY would not be.
- **scant keeps its identity** as a linter you can drop into CI, not an IDE.

Staleness follows from this too: if scant can't change your files, a report can only go stale because you edited elsewhere — and that's explicitly **not handled in v1**. No mtime polling, no filesystem watcher, no refresh dialog. Quit and re-run. Revisit only if it proves annoying in practice.

## New: `crates/scant-cli/src/tui/`

`scant-cli` is ~77 lines that parse args and print. A TUI exceeds that remit, so split it to preserve the *intent* of the core/cli rule — everything but the event loop stays terminal-free and testable:

- **`tui/app.rs`** — pure state machine: selected dependency, focused pane, tree expand/collapse, filter, memoized definition lookups. No ratatui, no I/O.
- **`tui/tree.rs`** — pure: flatten `DepReport.locations` paths into a renderable directory tree with collapse state.
- **`tui/render.rs`** — `fn draw(frame: &mut Frame, app: &App)`; pure state → frame, testable headlessly via `TestBackend`.
- **`tui/source.rs`** — pure: read a file and return the window of lines to display around a target, with bounds clamping for files shorter than the window.
- **`tui/mod.rs`** — the only terminal-touching part: setup/teardown and the event loop.

Reuse rather than reimplement: `analyze::{Verdict, UsageBand, DepReport}` for grouping and ordering, and mirror `report.rs`'s palette exactly — red/yellow/green for drop/inline/keep, **dim for registered and unknown**.

## CLI surface

Add `--interactive` to the existing flat `Cli` struct — **no subcommand**; `scant .` must keep working and there's no `Subcommand` derive in the repo. No short alias: CLAUDE.md mandates fully-spelled flags.

Guard: if stdout isn't a TTY (piped, CI), fail fast with a what/why/next-step message rather than garbling or hanging — `std::io::IsTerminal`, std, no new dep. Preserve the documented `0` clean / `1` findings exit contract on quit.

## Reading source files

One gotcha worth stating, since it's the only path resolution in the feature: **`locations` paths are relative to the scan root** (`build_dep_report` strips the prefix, but falls back to the absolute path if stripping fails). Resolve as: absolute → use as-is, else `scan_root.join(rel)`. Don't blindly join.

Definition paths from `defsite.rs` are already absolute (rooted at site-packages) and need no resolution.

Files that can't be read — deleted since the scan, permissions, or not valid UTF-8 — show an inline note in the pane rather than erroring out or panicking. Same posture as the existing "one unparseable file is a warning" rule.

## Terminal lifecycle

With no editor handoff, exactly one failure mode remains: **a panic in raw mode leaves the user's shell unusable.** Install a restoring panic hook (`ratatui::init()`/`restore()` do this in 0.30), and handle Ctrl-C the same way.

## Keybindings (dive-flavored)

`Tab` cycle focused pane — dependencies → usage → definition (dive's core interaction) · `j`/`k`/`↑`/`↓` move or scroll the focused pane · `g`/`G` top/bottom · `Ctrl-d`/`Ctrl-u` half-page · `Space` expand/collapse tree node · `/` filter, `Esc` clears · `?` help · `q`/`Ctrl-C` quit.

Group filters, one per verdict plus all: `0` all · `1` drop · `2` inline · `3` registered · `4` unknown · `5` keep. Numeric switching follows dive/k9s and sidesteps the collision with `k` = up.

`Enter` is deliberately left unbound — there is nothing to open, and binding it to something incidental would invite the expectation that scant edits files.

## Testing

`scant-cli` has **zero** tests today and no `tests/` dir — everything is inline `#[cfg(test)]` in core. This adds the first, following that convention; needs `insta` in `scant-cli`'s `[dev-dependencies]`.

- `defsite.rs` — synthetic fixtures: direct class def, direct function def, relative one-hop re-export, **absolute self-referential re-export**, multi-hop, cycle guard terminates, single-module dist, symbol absent.
- `namemap.rs` — size summing: normal `RECORD`, rows with an empty size field, editable install with no payload → `None` not `0`.
- `tui/tree.rs` — path list → tree shape, shared prefixes, single file, deep nesting, collapse state.
- `tui/app.rs` — selection clamping at both ends, filter narrows and `Esc` restores, group switch resets selection sanely, pane focus cycling through all three, empty filter result doesn't panic.
- `tui/render.rs` — insta snapshots of `TestBackend` buffers, **one per verdict variant (all five)**, locking layout the way `report.rs`'s snapshot does.
- `tui/source.rs` — line window around a target, clamped: target near the top of a file, near the end, file shorter than the window, empty file, unreadable file yields the inline note rather than a panic.

## Risk summary

Most of this is routine: split panes, tables, trees, and file previews are what ratatui exists for; the size work sums a field already parsed; the core changes are additive fields. Staying read-only removes the two genuinely hairy areas outright — no PTY, and no subprocess/terminal-suspend choreography.

**`defsite.rs` is the one piece that could disappoint** — nothing in the ecosystem does static go-to-definition from Rust into installed Python packages. It's de-risked by construction (a miss degrades to a note, never a wrong answer), but build it **first, standalone, with tests, against real site-packages** before any TUI code, while course-correcting is still cheap.

`ratatui` 0.30 is recent and its API moved meaningfully from 0.29 — verify `init`/`restore`/`Frame`/widget signatures against docs.rs at implementation time rather than trusting older examples.

## Verification

1. `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` (Docker `rust:latest` — no local toolchain).
2. Non-interactive path untouched: existing snapshots pass, `scant .` output byte-identical, mkdocs CI smoke test green.
3. Measure the wheel-size delta from ratatui+crossterm against the current ~4.6 MB; feature-gate as `default = ["tui"]` only if material.
4. Real TTY run (`docker run -it`; piped runs can't exercise it) against mkdocs, then Superset's 165 rows — the readability stress test. Confirm specifically: `prophet`'s definition pane resolves `Prophet` to its `class` definition; a `registered` dependency (Superset has many) shows its loading mechanism; reported sizes are sane against `du -sh` on the same site-packages.
5. Confirm the terminal is restored intact after `q`, after Ctrl-C, and after a deliberately forced panic.

## Out of scope (follow-ups)

Any form of editing — `$EDITOR` handoff or an embedded editable pane · staleness detection and re-scan (moot while read-only) · clipboard yank of the definition (the obvious next step for the inline workflow once the layout proves itself) · transitive installed weight (needs a dependency graph) · per-line usage sites beyond the first per file (needs `build_dep_report` to stop collapsing each file's lines to their minimum) · syntax highlighting (`syntect` is heavy; v1 dims context and emphasizes the target line, keeping color additive per CLAUDE.md) · live threshold tuning · mouse support.

## Critical files

- `crates/scant-core/src/namemap.rs` — capture size + file list in the existing RECORD walk
- `crates/scant-core/src/analyze.rs` — additive fields (`symbol_names`, `import_roots`, `installed_bytes` on `DepReport`; `site_packages` on `Report`)
- `crates/scant-core/src/defsite.rs` — **new**, static definition finder + tests
- `crates/scant-core/src/lib.rs` — register `defsite`
- `crates/scant-cli/src/tui/{mod,app,tree,render,source}.rs` — **new**
- `crates/scant-cli/src/main.rs` — `--interactive`, TTY guard, dispatch
- `Cargo.toml` (root) — `ratatui`/`crossterm` in `[workspace.dependencies]`; `crates/scant-cli/Cargo.toml` — `{ workspace = true }` refs plus `insta` dev-dep
