SHELL := /bin/bash

TAURI_DIR := src-tauri
CARGO := cargo

.PHONY: help fmt check test build build-all dev ci clean precommit

help:
	@echo "Available targets:"
	@echo "  make fmt       - Run rustfmt check"
	@echo "  make check     - Run cargo check"
	@echo "  make test      - Run cargo test"
	@echo "  make build     - Build local runnable app bundle (avoids DMG flakiness on macOS)"
	@echo "  make build-all - Build all Tauri bundles configured in tauri.conf.json"
	@echo "  make dev       - Run Tauri app in dev mode"
	@echo "  make ci        - Run fmt + check + test"
	@echo "  make clean     - Clean Rust build artifacts"
	@echo "  make precommit - Run pre-commit hooks on all files"

fmt:
	cd $(TAURI_DIR) && $(CARGO) fmt --all -- --check

check:
	cd $(TAURI_DIR) && $(CARGO) check --locked

test:
	cd $(TAURI_DIR) && $(CARGO) test --locked

build:
	cd $(TAURI_DIR) && $(CARGO) tauri build --verbose --bundles app

build-all:
	cd $(TAURI_DIR) && $(CARGO) tauri build --verbose

dev:
	cd $(TAURI_DIR) && $(CARGO) tauri dev

ci: fmt check test

clean:
	cd $(TAURI_DIR) && $(CARGO) clean

precommit:
	pre-commit run --all-files
