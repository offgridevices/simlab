# SimLab commands. Run `make` to see them.
.DEFAULT_GOAL := help
SHELL := /bin/bash
export PATH := /opt/homebrew/bin:/opt/homebrew/opt/rustup/bin:$(HOME)/.cargo/bin:$(PATH)

.PHONY: help install update build dev test validate clean uninstall claude

help: ## Show these commands
	@grep -E '^[a-z-]+:.*## ' $(MAKEFILE_LIST) | awk -F':.*## ' '{printf "  make %-10s %s\n", $$1, $$2}'

install: ## Build SimLab and put SimLab.app in Applications (asks before installing anything missing)
	@./install.sh

update: ## Pull the newest SimLab from GitHub and reinstall it
	@git pull --ff-only
	@./install.sh

build: ## Build the app without installing it
	@./install.sh --build-only

dev: ## Run the engine and the screens with live reload (for working on SimLab)
	@trap 'kill 0' EXIT; \
	  cargo run -p simlab-engine -- serve & \
	  (cd web && pnpm dev) & \
	  wait

test: ## Run the quick tests
	cargo test --workspace
	cd web && pnpm test

validate: ## Run every accuracy check (slow) and print the results
	cargo test --workspace --release -- --include-ignored

claude: ## Connect Claude Code to SimLab
	claude mcp add simlab -- /Applications/SimLab.app/Contents/MacOS/simlab-engine mcp

clean: ## Delete build files (keeps your projects and results)
	cargo clean
	rm -rf web/dist app/src-tauri/target app/src-tauri/binaries

uninstall: ## Remove SimLab.app (keeps your projects and results)
	rm -rf /Applications/SimLab.app
	@echo "Your data is still in ~/Library/Application Support/SimLab"
