# SPDX-License-Identifier: Apache-2.0
#
# Toktail: every token leaves a trail.
#
#   make demo      see Pip and the portal right away, with sample data
#   make install   build Toktail.app, put it in /Applications and start it
#   make help      everything else
#
# Works with the GNU make 3.81 that ships with macOS.

SHELL := /bin/bash
APP_NAME   := Toktail
APP        := dist/$(APP_NAME).app
INSTALLED  := /Applications/$(APP_NAME).app
BIN        := target/release/$(APP_NAME)
PORTAL     := http://127.0.0.1:8765/
DEMO_URL   := http://127.0.0.1:8799/
DATA_DIR   := $(HOME)/Library/Application Support/Toktail
DEMO_DIR   := $(HOME)/Library/Application Support/Toktail Demo
LOG        := $(HOME)/Library/Logs/Toktail/toktail.log
AGENT      := $(HOME)/Library/LaunchAgents/com.playandprosper.toktail.plist
VERSION    := $(shell grep -m1 '^version' Cargo.toml | cut -d'"' -f2)

# rustup installs cargo here; make it visible even in a fresh terminal.
export PATH := $(HOME)/.cargo/bin:$(PATH)

.DEFAULT_GOAL := help
.PHONY: help doctor rust build demo demo-reset run headless app install settings prompts savings lmstudio \
        open logs status s3-secret test lint fmt dist uninstall clean

help: ## Show this list
	@echo ""
	@echo "  Toktail $(VERSION)  —  every token leaves a trail"
	@echo ""
	@echo "  First time here? Run these in order:"
	@echo "    make demo        Build and play with sample data (no LM Studio needed)"
	@echo "    make install     Build Toktail.app, install it and start it"
	@echo "    make settings    Add your LM Studio hosts (this Mac or others, up to 17), their tokens and ports"
	@echo ""
	@echo "  All commands:"
	@grep -E '^[a-zA-Z0-9_-]+:.*## ' $(MAKEFILE_LIST) | \
	  awk 'BEGIN {FS = ":.*## "}; {printf "    make %-12s %s\n", $$1, $$2}'
	@echo ""

doctor: ## Check that this Mac has what Toktail needs
	@echo "Checking your Mac…"
	@if [ "$$(uname -s)" = "Darwin" ]; then echo "  ✓ macOS $$(sw_vers -productVersion) on $$(uname -m)"; \
	  else echo "  ✗ This isn't macOS. The menu bar app needs a Mac (make demo still works with HEADLESS=1)."; fi
	@if xcode-select -p >/dev/null 2>&1; then echo "  ✓ Xcode Command Line Tools"; \
	  else echo "  ✗ Xcode Command Line Tools missing: run  xcode-select --install"; fi
	@if command -v cargo >/dev/null 2>&1; then echo "  ✓ Rust $$(cargo --version | cut -d' ' -f2)"; \
	  else echo "  ✗ Rust missing: run  make rust"; fi
	@if command -v lms >/dev/null 2>&1; then echo "  ✓ LM Studio CLI (lms)"; \
	  else echo "  · LM Studio CLI (lms) not found. Optional: change the server port in LM Studio's Developer tab instead."; fi
	@if [ -d "$(INSTALLED)" ]; then echo "  ✓ Toktail installed in /Applications"; else echo "  · Toktail not installed yet: run  make install"; fi
	@if curl -fs -m 1 -H 'Host: 127.0.0.1:8765' $(PORTAL)api/ping >/dev/null 2>&1; then echo "  ✓ Toktail is running: $(PORTAL)"; \
	  else echo "  · Toktail isn't running"; fi

rust: ## Install the Xcode command line tools and Rust if they're missing
	@if [ "$$(uname -s)" = "Darwin" ] && ! xcode-select -p >/dev/null 2>&1; then \
	  echo "Installing the Xcode Command Line Tools. Finish the installer window, then run  make rust  again."; \
	  xcode-select --install; exit 1; fi
	@if command -v cargo >/dev/null 2>&1; then echo "Rust is already installed ($$(cargo --version))."; \
	else \
	  echo "Installing Rust with rustup (https://rustup.rs)…"; \
	  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal; \
	  echo "Rust installed."; \
	fi

build: rust ## Compile Toktail (release build)
	cargo build --release

demo: build ## Try Toktail with sample data and pretend traffic (Ctrl-C to stop)
	@echo ""
	@echo "  Pip is starting in demo mode. Watch the menu bar, and the portal at $(DEMO_URL)"
	@echo "  Sample data lives in: $(DEMO_DIR)  (make demo-reset clears it)"
	@echo "  Your real data and settings aren't touched. Press Ctrl-C to stop."
	@echo ""
	@$(BIN) --demo $(if $(HEADLESS),--headless,)

demo-reset: ## Delete the demo's sample data so the next demo starts fresh
	rm -rf "$(DEMO_DIR)"

run: build ## Run Toktail from the source folder (Ctrl-C to stop)
	$(BIN)

headless: build ## Run without the menu bar: proxy, portal and S3 only
	$(BIN) --headless

app: rust ## Build dist/Toktail.app without installing it
	./scripts/build-app.sh

install: rust ## Build Toktail.app, copy it to /Applications and start it
	./scripts/build-app.sh --install
	@echo ""
	@echo "  Toktail is running. Pip is in your menu bar and the portal is at $(PORTAL)"
	@echo "  Next: make settings  (add your LM Studio hosts: address, API token if needed, and the port your apps call)"
	@echo ""

settings: ## Open the portal's Settings: hosts, saving prompts, encryption, S3
	open "$(PORTAL)#/settings/hosts"

prompts: ## Open the portal's Prompts page: saved prompts, responses and chat
	open "$(PORTAL)#/prompts"

savings: ## Open the portal's Savings page: hardware, lifecycle and savings
	open "$(PORTAL)#/savings"

lmstudio: ## Optional: move this Mac's LM Studio to port 1235 so apps keep using 1234
	@if command -v lms >/dev/null 2>&1; then \
	  lms server stop >/dev/null 2>&1 || true; \
	  lms server start --port 1235 && echo "LM Studio now listens on 1235. Your apps keep calling http://localhost:1234/v1"; \
	else \
	  echo "The lms command isn't installed. In LM Studio, open the Developer tab, set the server port to 1235 and restart the server."; \
	  echo "(Or install the lms command: https://lmstudio.ai/docs/cli)"; \
	fi

open: ## Open the portal in your browser
	open $(PORTAL)

logs: ## Follow Toktail's log
	@mkdir -p "$$(dirname "$(LOG)")"; touch "$(LOG)"; tail -f "$(LOG)"

status: ## Show whether Toktail is running and what Pip is up to
	@curl -fs -m 2 -H 'Host: 127.0.0.1:8765' $(PORTAL)api/live?seconds=5 \
	  | python3 -c 'import json,sys; d=json.load(sys.stdin); print("Pip is %s, %.0f tokens/sec, %d request(s) in flight" % (d["state"], d["tokens_per_sec"], d["active_requests"])); [print("  %-14s %-10s %.0f tokens/sec" % (k, v["state"], v["tokens_per_sec"])) for k, v in d.get("hosts", {}).items()]' \
	  2>/dev/null || echo "Toktail isn't running. Start it with  open $(INSTALLED)  or  make install"

s3-secret: ## Store your S3 secret key in the Keychain
	@if [ -x "$(INSTALLED)/Contents/MacOS/$(APP_NAME)" ]; then "$(INSTALLED)/Contents/MacOS/$(APP_NAME)" --set-s3-secret; \
	else $(MAKE) build && $(BIN) --set-s3-secret; fi

test: rust ## Run the tests
	cargo test

lint: rust ## Check formatting and lints, the same way CI does
	cargo fmt --check
	cargo clippy --all-targets -- -D warnings

fmt: rust ## Format the code
	cargo fmt

dist: app ## Zip Toktail.app for sharing (dist/Toktail-<version>.zip)
	ditto -c -k --keepParent "$(APP)" "dist/$(APP_NAME)-$(VERSION).zip"
	@echo "dist/$(APP_NAME)-$(VERSION).zip"

uninstall: ## Remove Toktail.app and its login item (keeps your CSV history)
	-pkill -x $(APP_NAME)
	rm -rf "$(INSTALLED)"
	rm -f "$(AGENT)"
	@echo "Removed Toktail. Your CSV history and settings are still in: $(DATA_DIR)"

clean: ## Remove build output
	cargo clean
	rm -rf dist
