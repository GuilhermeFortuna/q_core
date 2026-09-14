.PHONY: check ci hooks fmt fmt-check lint test wheel wheel-test qt qt-test contracts contracts-check

CONTRACTS_REPO ?= https://github.com/GuilhermeFortuna/q_contracts.git
MATURIN ?= $(shell command -v maturin 2>/dev/null || echo "uvx maturin")
QT_MINIMAL_DIR ?= $(shell find $(HOME)/.local/share/qt_minimal_download -name "QtCore" -type d 2>/dev/null | head -n 1)/../..

check: fmt-check lint test wheel-test qt-test contracts-check
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
	cp -R "$$contracts_tmp/q_contracts/generated/rust/." contracts/

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
	diff -ru contracts "$$generated_tmp/rust"

wheel:
	rm -rf dist
	$(MATURIN) build --release --out dist --manifest-path crates/q-py/Cargo.toml

wheel-test: wheel
	python3 tests/test_wheel.py

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
