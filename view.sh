#!/usr/bin/env bash
# Build the petekSim Python extension and open the 3D viewer for an example
# model. One command — handles the venv + maturin build for you.
#
#   ./view.sh           # structured model (with relief)
#   ./view.sh --box     # box model with Monte Carlo P90/P50/P10
#
# Any args are passed through to examples/build_and_view.py. The viewer opens in
# your browser and serves until you press Ctrl-C.
set -euo pipefail
cd "$(dirname "$0")"

VENV=".venv-srs"

if [ ! -d "$VENV" ]; then
  echo "Creating venv $VENV ..."
  python3 -m venv "$VENV"
fi

# Ensure maturin is available in the venv.
if [ ! -x "$VENV/bin/maturin" ]; then
  echo "Installing maturin ..."
  "$VENV/bin/pip" install -q maturin
fi

# Build the extension into the venv (incremental; fast after the first time).
echo "Building peteksim (maturin develop) ..."
VIRTUAL_ENV="$PWD/$VENV" "$VENV/bin/maturin" develop -m crates/srs-py/Cargo.toml >/dev/null

# Build a model in code and open the viewer.
exec "$VENV/bin/python" examples/build_and_view.py "$@"
