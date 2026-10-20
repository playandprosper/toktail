# SPDX-License-Identifier: Apache-2.0
#
# Toktail: every token leaves a trail.
#
#   make demo      see Pip and the portal right away, with sample data
#   make install   build Toktail, install it and start it
#   make help      everything else
#
# Works on macOS (Apple Silicon and Intel, with the GNU make 3.81 that ships
# with it), on Linux, and on Windows from Git Bash or MSYS2 (or use
# scripts\build-windows.ps1 from PowerShell, no make needed).

# ------------------------------------------------------------ where am I
ifeq ($(OS),Windows_NT)
  PLATFORM := windows
else
  UNAME := $(shell uname -s)
  ifeq ($(UNAME),Darwin)
    PLATFORM := macos
  else
    PLATFORM := linux
  endif
endif

SHELL      := $(shell which bash 2>/dev/null || echo /bin/sh)
APP_NAME   := Toktail
VERSION    := $(shell grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
PORTAL     := http://127.0.0.1:8765/
DEMO_URL   := http://127.0.0.1:8799/
ARCH       := $(shell uname -m 2>/dev/null || echo x86_64)

ifeq ($(PLATFORM),macos)
  EXE        :=
  APP        := dist/$(APP_NAME).app
  INSTALLED  := /Applications/$(APP_NAME).app
  INSTALLED_BIN := $(INSTALLED)/Contents/MacOS/$(APP_NAME)
  DATA_DIR   := $(HOME)/Library/Application Support/Toktail
  DEMO_DIR   := $(HOME)/Library/Application Support/Toktail Demo
  LOG        := $(HOME)/Library/Logs/Toktail/toktail.log
  OPEN       := open
  CARGO_GUI  :=
  TRAY       := menu bar
endif
ifeq ($(PLATFORM),linux)
  EXE        :=
  INSTALLED_BIN := $(HOME)/.local/bin/toktail
  DATA_DIR   := $(or $(XDG_DATA_HOME),$(HOME)/.local/share)/Toktail
  DEMO_DIR   := $(or $(XDG_DATA_HOME),$(HOME)/.local/share)/Toktail Demo
  LOG        := $(DATA_DIR)/logs/toktail.log
  OPEN       := xdg-open
  # The tray icon needs GTK; without it Toktail builds as a headless server.
  # GTK + AppIndicator give the tray; without them Toktail builds headless.
  HAS_GTK    := $(shell pkg-config --exists gtk+-3.0 ayatana-appindicator3-0.1 2>/dev/null && echo yes)
  CARGO_GUI  := $(if $(HAS_GTK),,--no-default-features)
  TRAY       := $(if $(HAS_GTK),system tray,browser)
endif
ifeq ($(PLATFORM),windows)
  EXE        := .exe
  INSTALLED_BIN := $(LOCALAPPDATA)/Programs/Toktail/Toktail.exe
  DATA_DIR   := $(APPDATA)/Toktail
  DEMO_DIR   := $(APPDATA)/Toktail Demo
  LOG        := $(DATA_DIR)/logs/toktail.log
  OPEN       := cmd //c start ""
  CARGO_GUI  :=
  TRAY       := system tray
endif
BIN := target/release/$(APP_NAME)$(EXE)

# rustup installs cargo here; make it visible even in a fresh terminal.
export PATH := $(HOME)/.cargo/bin:$(PATH)

.DEFAULT_GOAL := help
.PHONY: help doctor rust build demo demo-reset run headless app install settings prompts models savings chat lmstudio \
        open logs status s3-secret test lint fmt dist dist-mac-universal service uninstall clean test-ui-data check-ui

help: ## Show this list
	@echo ""
	@echo "  Toktail $(VERSION)  —  every token leaves a trail   ($(PLATFORM), $(ARCH))"
	@echo ""
	@echo "  First time here? Run these in order:"
	@echo "    make demo        Build and play with sample data (no LM Studio needed)"
	@echo "    make install     Build Toktail, install it and start it (Pip lives in your $(TRAY))"
	@echo "    make settings    Add your LM Studio hosts (this computer or others, up to 17), their tokens and ports"
	@echo ""
	@echo "  All commands:"
	@grep -E '^[a-zA-Z0-9_-]+:.*## ' $(MAKEFILE_LIST) | \
	  awk 'BEGIN {FS = ":.*## "}; {printf "    make %-18s %s\n", $$1, $$2}'
	@echo ""

doctor: ## Check that this computer has what Toktail needs
	@echo "Checking this computer ($(PLATFORM), $(ARCH))…"
	@if [ "$(PLATFORM)" = "macos" ]; then echo "  ✓ macOS $$(sw_vers -productVersion) on $$(uname -m) (Toktail needs 11 Big Sur or later)"; \
	  if xcode-select -p >/dev/null 2>&1; then echo "  ✓ Xcode Command Line Tools"; else echo "  ✗ Xcode Command Line Tools missing: run  xcode-select --install"; fi; fi
	@if [ "$(PLATFORM)" = "linux" ]; then \
	  if [ -n "$(HAS_GTK)" ]; then echo "  ✓ GTK and AppIndicator: Pip goes in the system tray"; \
	  else echo "  · No GTK/AppIndicator dev files: Toktail builds headless (portal in the browser). For the tray: sudo apt install libgtk-3-dev libxdo-dev libayatana-appindicator3-dev"; fi; fi
	@if command -v cargo >/dev/null 2>&1; then echo "  ✓ Rust $$(cargo --version | cut -d' ' -f2)"; \
	  else echo "  ✗ Rust missing: run  make rust"; fi
	@if command -v lms >/dev/null 2>&1; then echo "  ✓ LM Studio CLI (lms)"; \
	  else echo "  · LM Studio CLI (lms) not found. Optional: change the server port in LM Studio's Developer tab instead."; fi
	@if [ -e "$(INSTALLED_BIN)" ]; then echo "  ✓ Toktail installed: $(INSTALLED_BIN)"; else echo "  · Toktail not installed yet: run  make install"; fi
	@if curl -fs -m 1 -H 'Host: 127.0.0.1:8765' $(PORTAL)api/ping >/dev/null 2>&1; then echo "  ✓ Toktail is running: $(PORTAL)"; \
	  else echo "  · Toktail isn't running"; fi

rust: ## Install Rust (and on a Mac the Xcode command line tools) if missing
	@if [ "$(PLATFORM)" = "macos" ] && ! xcode-select -p >/dev/null 2>&1; then \
	  echo "Installing the Xcode Command Line Tools. Finish the installer window, then run  make rust  again."; \
	  xcode-select --install; exit 1; fi
	@if command -v cargo >/dev/null 2>&1; then echo "Rust is already installed ($$(cargo --version))."; \
	elif [ "$(PLATFORM)" = "windows" ]; then \
	  echo "Install Rust from https://rustup.rs (the default MSVC setup), then run make again."; exit 1; \
	else \
	  echo "Installing Rust with rustup (https://rustup.rs)…"; \
	  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal; \
	  echo "Rust installed."; \
	fi
	@if command -v rustup >/dev/null 2>&1; then rustup component add rustfmt clippy >/dev/null 2>&1 || true; fi

build: rust ## Compile Toktail (release build) for this computer
	cargo build $(CARGO_GUI) --release

demo: build ## Try Toktail with sample data and pretend traffic (Ctrl-C to stop)
	@echo ""
	@echo "  Pip is starting in demo mode. Watch the $(TRAY), and the portal at $(DEMO_URL)"
	@echo "  Sample data lives in: $(DEMO_DIR)  (make demo-reset clears it)"
	@echo "  Your real data and settings aren't touched. Press Ctrl-C to stop."
	@echo ""
	@"$(BIN)" --demo $(if $(or $(HEADLESS),$(CARGO_GUI)),--headless,)

demo-reset: ## Delete the demo's sample data so the next demo starts fresh
	rm -rf "$(DEMO_DIR)"

run: build ## Run Toktail from the source folder (Ctrl-C to stop)
	"$(BIN)"

headless: build ## Run without the menu bar or tray: proxy, portal and S3 only
	"$(BIN)" --headless

app: rust ## macOS: build dist/Toktail.app for this Mac without installing it
	./scripts/build-app.sh

dist-mac-universal: rust ## macOS: one Toktail.app for Apple Silicon and Intel Macs, zipped
	./scripts/build-app.sh --universal
	ditto -c -k --keepParent "dist/$(APP_NAME).app" "dist/$(APP_NAME)-$(VERSION)-macos-universal.zip"
	@echo "dist/$(APP_NAME)-$(VERSION)-macos-universal.zip"

install: rust ## Build Toktail, install it and start it
ifeq ($(PLATFORM),macos)
	./scripts/build-app.sh --install
else ifeq ($(PLATFORM),windows)
	powershell -NoProfile -ExecutionPolicy Bypass -File scripts/build-windows.ps1 -Install
else
	cargo build $(CARGO_GUI) --release
	-pkill -x $(APP_NAME)
	mkdir -p "$(HOME)/.local/bin" "$(HOME)/.local/share/applications" "$(HOME)/.local/share/icons/hicolor/256x256/apps"
	install -m 755 "$(BIN)" "$(INSTALLED_BIN)"
	cp packaging/AppIcon-1024.png "$(HOME)/.local/share/icons/hicolor/256x256/apps/toktail.png"
	sed "s|@BIN@|$(INSTALLED_BIN)|" packaging/toktail.desktop > "$(HOME)/.local/share/applications/toktail.desktop"
	@if [ -n "$(CARGO_GUI)" ]; then echo "Headless build: run  make service  to keep it running in the background."; \
	else (nohup "$(INSTALLED_BIN)" >/dev/null 2>&1 &); fi
endif
	@echo ""
	@echo "  Toktail is installed. Pip is in your $(TRAY) and the portal is at $(PORTAL)"
	@echo "  Next: make settings  (add your LM Studio hosts: address, API token if needed, and the port your apps call)"
	@echo ""

service: ## Linux: run headless Toktail as a systemd user service (servers, LM Studio boxes)
	@[ "$(PLATFORM)" = "linux" ] || { echo "make service is for Linux; macOS and Windows start Toktail at login on their own."; exit 1; }
	cargo build --no-default-features --release
	mkdir -p "$(HOME)/.local/bin" "$(HOME)/.config/systemd/user"
	install -m 755 "$(BIN)" "$(INSTALLED_BIN)"
	sed "s|@BIN@|$(INSTALLED_BIN)|" packaging/toktail.service > "$(HOME)/.config/systemd/user/toktail.service"
	systemctl --user daemon-reload
	systemctl --user enable --now toktail.service
	@echo "Toktail runs as a user service. Logs: journalctl --user -u toktail -f. The portal only answers on this machine:"
	@echo "  from another computer, use  ssh -L 8765:127.0.0.1:8765 this-host  and open $(PORTAL)"

settings: ## Open the portal's Settings: hosts, saving prompts, encryption, S3, budgets
	$(OPEN) "$(PORTAL)#/settings/hosts"

prompts: ## Open the Prompts page: saved prompts and responses
	$(OPEN) "$(PORTAL)#/prompts"

chat: ## Open Pip's chat: ask about your tokens, or talk straight to a model
	$(OPEN) "$(PORTAL)#/prompts/chat"

models: ## Open the Models page: speed, wait, errors and cost per model
	$(OPEN) "$(PORTAL)#/models"

savings: ## Open the Savings page: hardware, lifecycle and savings
	$(OPEN) "$(PORTAL)#/savings"

lmstudio: ## Optional: move this computer's LM Studio to port 1235 so apps keep using 1234
	@if command -v lms >/dev/null 2>&1; then \
	  lms server stop >/dev/null 2>&1 || true; \
	  lms server start --port 1235 && echo "LM Studio now listens on 1235. Your apps keep calling http://localhost:1234/v1"; \
	else \
	  echo "The lms command isn't installed. In LM Studio, open the Developer tab, set the server port to 1235 and restart the server."; \
	  echo "(Or install the lms command: https://lmstudio.ai/docs/cli)"; \
	fi

open: ## Open the portal in your browser
	$(OPEN) "$(PORTAL)"

logs: ## Follow Toktail's log
	@mkdir -p "$$(dirname "$(LOG)")"; touch "$(LOG)"; tail -f "$(LOG)"

status: ## Show whether Toktail is running and what Pip is up to
	@curl -fs -m 2 -H 'Host: 127.0.0.1:8765' $(PORTAL)api/live?seconds=5 \
	  | python3 -c 'import json,sys; d=json.load(sys.stdin); print("Pip is %s, %.0f tokens/sec, %d request(s) in flight" % (d["state"], d["tokens_per_sec"], d["active_requests"])); [print("  %-14s %-10s %.0f tokens/sec" % (k, v["state"], v["tokens_per_sec"])) for k, v in d.get("hosts", {}).items()]' \
	  2>/dev/null || echo "Toktail isn't running. Start it with  make install  (or make run)"

s3-secret: ## Store your S3 secret key in the Keychain / Credential Manager
	@if [ -x "$(INSTALLED_BIN)" ]; then "$(INSTALLED_BIN)" --set-s3-secret; \
	else $(MAKE) build && "$(BIN)" --set-s3-secret; fi

test: rust ## Run the tests
	cargo test $(CARGO_GUI)

lint: rust ## Check formatting and lints, the same way CI does
	cargo fmt --check
	cargo clippy $(CARGO_GUI) --all-targets -- -D warnings

fmt: rust ## Format the code
	cargo fmt

check-ui: ## Check the portal's scripts and their pure helpers (needs Node)
	node scripts/test_ui_data.cjs

test-ui-data: check-ui

dist: rust ## Package Toktail for sharing: a zip (macOS universal, Windows) or tarball (Linux)
ifeq ($(PLATFORM),macos)
	$(MAKE) dist-mac-universal
else ifeq ($(PLATFORM),windows)
	powershell -NoProfile -ExecutionPolicy Bypass -File scripts/build-windows.ps1
else
	cargo build $(CARGO_GUI) --release
	rm -rf "dist/$(APP_NAME)-$(VERSION)-linux-$(ARCH)" && mkdir -p "dist/$(APP_NAME)-$(VERSION)-linux-$(ARCH)"
	cp "$(BIN)" README.md LICENSE NOTICE packaging/toktail.desktop packaging/toktail.service "dist/$(APP_NAME)-$(VERSION)-linux-$(ARCH)/"
	tar -C dist -czf "dist/$(APP_NAME)-$(VERSION)-linux-$(ARCH).tar.gz" "$(APP_NAME)-$(VERSION)-linux-$(ARCH)"
	@echo "dist/$(APP_NAME)-$(VERSION)-linux-$(ARCH).tar.gz"
endif

uninstall: ## Remove Toktail and its login item (keeps your CSV history)
	-pkill -x $(APP_NAME) 2>/dev/null || taskkill //IM Toktail.exe //F 2>/dev/null || true
ifeq ($(PLATFORM),macos)
	rm -rf "$(INSTALLED)"
	rm -f "$(HOME)/Library/LaunchAgents/com.playandprosper.toktail.plist"
else ifeq ($(PLATFORM),windows)
	-reg delete "HKCU\Software\Microsoft\Windows\CurrentVersion\Run" //v Toktail //f
	rm -rf "$(LOCALAPPDATA)/Programs/Toktail"
else
	-systemctl --user disable --now toktail.service 2>/dev/null
	rm -f "$(INSTALLED_BIN)" "$(HOME)/.config/autostart/toktail.desktop" "$(HOME)/.local/share/applications/toktail.desktop" "$(HOME)/.config/systemd/user/toktail.service"
endif
	@echo "Removed Toktail. Your CSV history and settings are still in: $(DATA_DIR)"

clean: ## Remove build output
	cargo clean
	rm -rf dist

