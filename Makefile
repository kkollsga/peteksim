# petekSim — build targets.
SRS_VENV := .venv-srs
PY := $(abspath $(SRS_VENV))/bin/python
ACCEPTANCE := crates/srs-py/tests/test_acceptance.py

.PHONY: help build test lint fmt develop gate view acceptance acceptance-gate

help:
	@echo "make build        cargo build the workspace"
	@echo "make test         cargo test the workspace"
	@echo "make lint         cargo clippy (warnings = errors) + rustfmt check"
	@echo "make fmt          cargo fmt"
	@echo "make develop      maturin develop srs-py into $(SRS_VENV)"
	@echo "make gate         full local gate: fmt + lint + build + test"
	@echo "make acceptance   THE R6 pre-stamp gate: fast leg + opt-in spill/render legs (rebuilds wheel)"
	@echo "make acceptance-gate  just the fast R6 gate (-m acceptance); assumes a fresh wheel"
	@echo "make view         build a model in Python (examples/) and open the 3D viewer"

build:
	cargo build --workspace

test:
	cargo test --workspace

lint:
	cargo clippy --workspace --all-targets
	cargo fmt --all --check

fmt:
	cargo fmt --all

develop:
	VIRTUAL_ENV=$(abspath $(SRS_VENV)) $(abspath $(SRS_VENV))/bin/maturin develop -m crates/srs-py/Cargo.toml

gate: fmt lint build test
	@echo "local gate green"

view: develop
	$(SRS_VENV)/bin/python examples/build_and_view.py

# The doctrine-R6 end-to-end acceptance gate (task_suite_acceptance_suite): the
# coordinator's pre-stamp gate for cross-repo features. `acceptance` rebuilds the
# wheel then runs the fast leg + the opt-in spill + render legs; `acceptance-gate`
# is just the fast leg against whatever wheel is installed.
acceptance: develop
	$(PY) -m pytest $(ACCEPTANCE) -m "acceptance or acceptance_spill or acceptance_render" -q

acceptance-gate:
	$(PY) -m pytest $(ACCEPTANCE) -m acceptance -q
