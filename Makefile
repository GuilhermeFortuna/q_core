.PHONY: check ci hooks fmt fmt-check lint test wheel wheel-test qt qt-test contracts contracts-check \
	fixtures fixtures-check fixtures-backend fixtures-backend-check fixtures-test parity-isolation \
	bench-bar-window

BACKEND_REPO ?= https://github.com/GuilhermeFortuna/q_backend.git
CONTRACTS_REPO ?= https://github.com/GuilhermeFortuna/q_contracts.git
NUMERIC_FAMILIES := indicators
BACKEND_FAMILIES := bar_window exit_rules
MATURIN ?= $(shell command -v maturin 2>/dev/null || echo "uvx maturin")
QT_MINIMAL_DIR ?= $(shell find $(HOME)/.local/share/qt_minimal_download -name "QtCore" -type d 2>/dev/null | head -n 1)/../..

check: fmt-check lint test fixtures-test fixtures-check parity-isolation wheel-test qt-test contracts-check
	@echo "All workspace checks passed successfully."

ci:
	./scripts/ci.sh

hooks:
	./scripts/install-hooks.sh

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all --check

lint:
	cargo clippy --workspace --all-targets -- -D warnings

test:
	cargo test --workspace

contracts:
	contracts_tmp="$$(mktemp -d)"; \
	trap 'rm -rf "$$contracts_tmp"' EXIT; \
	git clone --quiet "$(CONTRACTS_REPO)" "$$contracts_tmp/q_contracts"; \
	git -C "$$contracts_tmp/q_contracts" checkout --quiet "$$(cat CONTRACTS_REV)"; \
	rm -rf contracts; \
	mkdir -p contracts; \
	cp -R "$$contracts_tmp/q_contracts/generated/rust/." contracts/; \
	mkdir -p contracts/schema/api/arrow; \
	cp "$$contracts_tmp/q_contracts/schema/api/arrow/bars.schema.json" contracts/schema/api/arrow/bars.schema.json

contracts-check:
	contracts_tmp="$$(mktemp -d)"; \
	trap 'rm -rf "$$contracts_tmp"' EXIT; \
	git clone --quiet "$(CONTRACTS_REPO)" "$$contracts_tmp/q_contracts"; \
	git -C "$$contracts_tmp/q_contracts" checkout --quiet "$$(cat CONTRACTS_REV)"; \
	generated_tmp="$$contracts_tmp/generated"; \
	if python3 -c 'import yaml' 2>/dev/null; then \
		python3 "$$contracts_tmp/q_contracts/tools/generate.py" --language rust --out "$$generated_tmp"; \
	else \
		uv run --project "$$contracts_tmp/q_contracts" python "$$contracts_tmp/q_contracts/tools/generate.py" \
			--language rust --out "$$generated_tmp"; \
	fi; \
	diff -ru --exclude=schema contracts "$$generated_tmp/rust"; \
	diff -u contracts/schema/api/arrow/bars.schema.json \
		"$$contracts_tmp/q_contracts/schema/api/arrow/bars.schema.json"

wheel:
	rm -rf dist
	$(MATURIN) build --release --out dist --manifest-path crates/q-py/Cargo.toml

wheel-test: wheel
	python3 tests/test_wheel.py
	python3 tests/test_bar_frame.py

bench-bar-window: wheel
	@venv_dir="$$(mktemp -d)"; \
	trap 'rm -rf "$$venv_dir"' EXIT; \
	uv venv "$$venv_dir"; \
	uv pip install --python "$$venv_dir/bin/python" dist/*.whl numpy pandas; \
	"$$venv_dir/bin/python" tests/bench_bar_window.py

qt:
	cargo build -p q-qt

qt-test: qt
	@inc_q_qt=$$(find target/debug/build -path "*/q-qt-*/out/cxxqtbuild/include" 2>/dev/null | head -n 1); \
	inc_cxx_qt_lib=$$(find target/debug/build -path "*/cxx-qt-lib-*/out/cxxqtbuild/include" 2>/dev/null | head -n 1); \
	qt_dir="$(QT_MINIMAL_DIR)"; \
	g++ -std=c++17 crates/q-qt/tests/harness.cpp -o target/debug/qt_harness \
		-I "$$inc_q_qt" \
		-I "$$inc_cxx_qt_lib" \
		-I "$$qt_dir/include" \
		-I "$$qt_dir/include/QtCore" \
		-L target/debug -lq_qt \
		-L "$$qt_dir/lib" -lQt6Core \
		-lpthread -ldl -lm -fPIC; \
	LD_LIBRARY_PATH="$$qt_dir/lib:$$LD_LIBRARY_PATH" target/debug/qt_harness

fixtures-test:
	uv run --frozen --project tools/reference python -m unittest discover -s tools/reference

fixtures:
	uv run --frozen --project tools/reference python tools/reference/export_reference.py --backend-repo "$(BACKEND_REPO)" $(NUMERIC_FAMILIES:%=--family %) --out fixtures/reference

fixtures-check:
	@fixtures_tmp="$$(mktemp -d)"; \
	trap 'rm -rf "$$fixtures_tmp"' EXIT; \
	uv run --frozen --project tools/reference python tools/reference/export_reference.py --backend-repo "$(BACKEND_REPO)" $(NUMERIC_FAMILIES:%=--family %) --out "$$fixtures_tmp"; \
	for fam in inputs $(NUMERIC_FAMILIES); do \
		uv run --frozen --project tools/reference python tools/reference/export_reference.py compare \
			"fixtures/reference/$$fam" "$$fixtures_tmp/$$fam" || exit 1; \
	done

fixtures-backend:
	@if [ -z "$(BACKEND_FAMILIES)" ]; then \
		echo "No backend families configured (BACKEND_FAMILIES is empty)."; \
	else \
		qb_tmp="$$(mktemp -d)"; \
		trap 'rm -rf "$$qb_tmp"' EXIT; \
		git clone --quiet "$(BACKEND_REPO)" "$$qb_tmp/q_backend"; \
		git -C "$$qb_tmp/q_backend" checkout --quiet "$$(cat BACKEND_REV)"; \
		uv sync --frozen --project "$$qb_tmp/q_backend"; \
		uv run --frozen --project "$$qb_tmp/q_backend" python tools/reference/export_reference.py \
			--backend-checkout "$$qb_tmp/q_backend" $(BACKEND_FAMILIES:%=--family %) --out fixtures/reference; \
	fi

fixtures-backend-check:
	@if [ -z "$(BACKEND_FAMILIES)" ]; then \
		echo "No backend families configured (BACKEND_FAMILIES is empty)."; \
	else \
		qb_tmp="$$(mktemp -d)"; \
		fixtures_tmp="$$(mktemp -d)"; \
		trap 'rm -rf "$$qb_tmp" "$$fixtures_tmp"' EXIT; \
		git clone --quiet "$(BACKEND_REPO)" "$$qb_tmp/q_backend"; \
		git -C "$$qb_tmp/q_backend" checkout --quiet "$$(cat BACKEND_REV)"; \
		uv sync --frozen --project "$$qb_tmp/q_backend"; \
		uv run --frozen --project "$$qb_tmp/q_backend" python tools/reference/export_reference.py \
			--backend-checkout "$$qb_tmp/q_backend" $(BACKEND_FAMILIES:%=--family %) --out "$$fixtures_tmp"; \
		for fam in $(BACKEND_FAMILIES); do \
			diff -ru "fixtures/reference/$$fam" "$$fixtures_tmp/$$fam" || exit 1; \
		done; \
	fi

parity-isolation:
	@if cargo tree -p q-py -e normal,build 2>/dev/null | grep -q "q-parity"; then \
		echo "ERROR: q-parity found in q-py dependency tree" >&2; exit 1; \
	fi
	@if cargo tree -p q-qt -e normal,build 2>/dev/null | grep -q "q-parity"; then \
		echo "ERROR: q-parity found in q-qt dependency tree" >&2; exit 1; \
	fi
