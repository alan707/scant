#!/usr/bin/env bash
# Installs a target repo's declared dependencies into <repo>/.venv, best-effort.
#
# scant resolves distribution names to import names from installed metadata, so the
# quality of every verdict depends on how much of the manifest actually got installed.
# That is why this walks down three strategies instead of failing on the first error:
# a repo where 40 of 190 packages are missing still produces a usable report (the rest
# land in `unknown`), but a repo where the whole resolve aborted produces a misleading one.
#
# Appends FT_INSTALL_MODE / FT_INSTALLED / FT_ATTEMPTED to $GITHUB_ENV.
set -uo pipefail

root="$1"
python_pin="${2:-}"
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

cd "$root" || exit 1
venv_args=()
[ -n "$python_pin" ] && venv_args=(--python "$python_pin")

# PEP 503 normalization, so "Flask-SQLAlchemy" and "flask_sqlalchemy" compare equal
normalize() { tr '[:upper:]' '[:lower:]' | tr '_.' '--' | tr -s '-'; }

fallback_loop() {
  echo "::group::Per-package install (tolerating failures)"
  local failures=0
  while IFS= read -r req; do
    [ -z "$req" ] && continue
    uv pip install --quiet --no-progress --python "$root/.venv/bin/python" "$req" 2>/dev/null || {
      failures=$((failures + 1))
      echo "  could not install: $req"
    }
  done < <(python3 "$script_dir/collect_deps.py" "$root")
  echo "  $failures package(s) failed to install"
  echo "::endgroup::"
}

mode=""
if [ -f uv.lock ]; then
  mode="uv sync"
  # --no-install-project: we want the dependencies, not a built copy of the app under test
  uv sync --frozen --no-install-project --all-extras --no-progress || {
    mode="uv sync + per-package"
    uv venv "${venv_args[@]}" 2>/dev/null
    fallback_loop
  }
elif [ -f requirements.txt ]; then
  mode="uv pip install -r"
  uv venv "${venv_args[@]}" || exit 1
  uv pip install --quiet --no-progress -r requirements.txt || {
    mode="per-package"
    fallback_loop
  }
else
  mode="per-package"
  uv venv "${venv_args[@]}" || exit 1
  fallback_loop
fi

declared=$(python3 "$script_dir/collect_deps.py" "$root" | sed -E 's/[[:space:]]*[;[<>=!~].*//' | sed -E 's/[[:space:]]+$//' | normalize | sort -u)
present=$(uv pip list --format json --python "$root/.venv/bin/python" 2>/dev/null | jq -r '.[].name' | normalize | sort -u)
attempted=$(echo "$declared" | grep -c . || true)
installed=$(comm -12 <(echo "$declared") <(echo "$present") | grep -c . || true)

echo "install mode: $mode -- $installed of $attempted declared packages resolved in the venv"
{
  echo "FT_INSTALL_MODE=$mode"
  echo "FT_INSTALLED=$installed"
  echo "FT_ATTEMPTED=$attempted"
} >> "${GITHUB_ENV:-/dev/null}"
