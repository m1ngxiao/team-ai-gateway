PYTHON ?= python3
PNPM ?= pnpm

.PHONY: help install-ui build-ui test-ui test-rust test-dashboard test-tools check-publication
help:
	@echo "install-ui build-ui test-ui test-rust test-dashboard test-tools check-publication"
install-ui:
	$(PNPM) -C apps install --frozen-lockfile
build-ui:
	$(PNPM) -C apps run build
test-ui:
	$(PNPM) -C apps run test:runtime
test-rust:
	cargo test --locked --release --workspace -- --test-threads=1
test-dashboard:
	cd services/dashboard && $(PYTHON) -m pytest tests && node --test tests/test_frontend.mjs
test-tools:
	$(PYTHON) -m pytest scripts/tests
check-publication:
	$(PYTHON) scripts/check-publication.py
