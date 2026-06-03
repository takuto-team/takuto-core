# Maestro — container management shortcuts
#
# Wraps docker/podman compose with the correct -f flags.
# DinD sidecar is included by default; set DIND=0 to disable.
#
# Container engine: defaults to docker (fallback to podman if docker not found).
# Override on the command line:
#   make PODMAN=1 <target>   — force podman
#   make DOCKER=1 <target>   — force docker
ifdef PODMAN
  _ENGINE := podman
else ifdef DOCKER
  _ENGINE := docker
else
  _ENGINE := $(shell command -v docker >/dev/null 2>&1 && echo docker || echo podman)
endif

IS_PODMAN := $(shell [ "$(_ENGINE)" = "podman" ] && echo 1 || echo 0)
# Prefer the compose plugin (<engine> compose); fall back to the standalone binary (<engine>-compose).
_COMPOSE_PLUGIN := $(shell $(_ENGINE) compose version >/dev/null 2>&1 && echo 1 || echo 0)
ifeq ($(_COMPOSE_PLUGIN),1)
  COMPOSE := $(_ENGINE) compose
else
  COMPOSE := $(_ENGINE)-compose
endif
HAS_COMPOSE := $(shell $(COMPOSE) version >/dev/null 2>&1 && echo 1 || echo 0)

# Detect standalone podman-compose binary (needs --podman-run-args for -it).
PODMAN_COMPOSE_BIN := $(shell command -v podman-compose 2>/dev/null)
# Project name = current directory name (matches podman-compose volume naming convention).
PROJECT_NAME := $(shell basename $(CURDIR))

# Set DIND=0 to run without the Docker-in-Docker sidecar.
DIND ?= 1
# Set BACKEND=postgres or BACKEND=mariadb to layer the external-DB
# overlay on top of the SQLite default. `BACKEND=sqlite` (the default)
# uses the base compose file only — Maestro talks to its local
# {data_dir}/maestro.db. Plan-11 §8 importer auto-copies that SQLite
# file into the chosen external DB on first boot.
BACKEND ?= sqlite
COMPOSE_FILES := -f docker-compose.yml
ifeq ($(DIND),1)
COMPOSE_FILES += -f docker-compose.dind.yml
endif
ifeq ($(BACKEND),postgres)
COMPOSE_FILES += -f docker-compose.postgres.yml
else ifeq ($(BACKEND),mariadb)
COMPOSE_FILES += -f docker-compose.mariadb.yml
else ifneq ($(BACKEND),sqlite)
$(error BACKEND must be one of: sqlite, postgres, mariadb (got '$(BACKEND)'))
endif

# Resolve the actual image name for the maestro service (compose may prefix with project name).
MAESTRO_IMAGE = $(shell $(COMPOSE) $(COMPOSE_FILES) images maestro --format '{{.Repository}}:{{.Tag}}' 2>/dev/null | head -1)

# Docker image registry for push targets.
REGISTRY ?= ghcr.io/morphet81/maestro

.PHONY: help build build-local start stop auth test logs logs-maestro ps bash exec worker-bash restart load-worker clean-dind ui-build push push-arm64 push-amd64 check check-full backup-postgres backup-mariadb

# Output directory for `backup-postgres` / `backup-mariadb`. Gitignored.
DUMP_DIR ?= dump
.DEFAULT_GOAL := help

help: ## Show this help
	@echo ""
	@echo "  \033[1mMaestro — container management shortcuts\033[0m"
	@echo ""
	@echo "  \033[1mEngine\033[0m   defaults to docker — use PODMAN=1 to force podman"
	@echo "  \033[1mBackend\033[0m  defaults to sqlite — set BACKEND=postgres or BACKEND=mariadb"
	@echo "           e.g. \033[36mmake start BACKEND=postgres\033[0m"
	@echo "                \033[36mmake restart BACKEND=mariadb\033[0m"
	@echo "                \033[36mmake logs BACKEND=postgres\033[0m"
	@echo ""
	@echo "  \033[1mTargets\033[0m"
	@grep -E '^[a-zA-Z0-9_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "    \033[36m%-16s\033[0m %s\n", $$1, $$2}'
	@echo ""

ui-build: ## Build the React dashboard
	@echo "Building React dashboard..."
	cd ui && npm install --legacy-peer-deps && npm run build

build: ui-build ## Build Rust workspace + container image
	@echo "Building Rust workspace..."
	cargo build
ifeq ($(HAS_COMPOSE),1)
	$(COMPOSE) $(COMPOSE_FILES) build || (echo "ERROR: Image build failed. Check the output above." >&2; exit 1)
else
	@echo "NOTE: docker/podman compose not available — skipping container image build."
endif

build-local: ## Build container image locally (no compose)
	@echo "Building with $(_ENGINE)..."
ifeq ($(HAS_BUILDX),1)
	$(_ENGINE) buildx build --platform linux/amd64 --build-arg MAESTRO_VERSION=$$(cat VERSION) -t maestro:local-test --load .
else
	DOCKER_BUILDKIT=1 $(_ENGINE) build --platform linux/amd64 --build-arg MAESTRO_VERSION=$$(cat VERSION) -t maestro:local-test .
endif

start: ## Start Maestro + DinD sidecar
	@if [ ! -f .maestro/config.toml ]; then \
		echo "ERROR: .maestro/config.toml not found." >&2; \
		echo "       mkdir -p .maestro && cp config.toml.example .maestro/config.toml" >&2; \
		exit 1; \
	fi
	@if [ ! -f .maestro/maestro.env ]; then \
		echo "WARNING: .maestro/maestro.env not found — creating empty file."; \
		echo "         Add API tokens and secrets (see maestro.env.example)."; \
		touch .maestro/maestro.env; \
	fi
	@mkdir -p .maestro/workflows
	$(COMPOSE) $(COMPOSE_FILES) up -d || (echo "ERROR: Failed to start containers. Run 'make logs' for details." >&2; exit 1)
ifeq ($(DIND),1)
	@$(MAKE) --no-print-directory load-worker
endif

stop: ## Stop and remove containers
	$(COMPOSE) $(COMPOSE_FILES) down

auth: ## Interactive first-time auth setup
	@if [ ! -f .maestro/config.toml ]; then \
		echo "ERROR: .maestro/config.toml not found." >&2; \
		echo "       mkdir -p .maestro && cp config.toml.example .maestro/config.toml" >&2; \
		exit 1; \
	fi
	@if [ ! -f .maestro/maestro.env ]; then \
		echo "WARNING: .maestro/maestro.env not found — creating empty file."; \
		echo "         Add API tokens and secrets (see maestro.env.example)."; \
		touch .maestro/maestro.env; \
	fi
ifeq ($(IS_PODMAN),1)
	@P=$(PROJECT_NAME); \
	IMAGE=$$(podman images --format '{{.Repository}}:{{.Tag}}' | grep -E "(^|/)$${P}[_-]maestro:|^maestro[-_]maestro:" | head -1); \
	if [ -z "$$IMAGE" ]; then echo "ERROR: Maestro image not found. Run 'make build' first." >&2; exit 1; fi; \
	podman run --rm -it \
		--security-opt=label=disable \
		-v "$$(pwd)/.maestro/config.toml":/etc/maestro/config.toml:ro \
		-v "$$(pwd)/.maestro/workflows":/etc/maestro/workflows:ro \
		-v "$$(pwd)/.maestro/maestro.env":/etc/maestro/env:ro \
		-v "$${P}_maestro-data":/home/maestro/.maestro \
		-v "$${P}_claude-auth":/home/maestro/.claude \
		-v "$${P}_cursor-auth":/home/maestro/.cursor \
		-v "$${P}_agents-data":/home/maestro/.agents \
		-v "$${P}_gh-auth":/home/maestro/.config/gh \
		-v "$${P}_acli-auth":/home/maestro/.config/acli \
		-v "$${P}_fcli-auth":/home/maestro/.config/fcli \
		-v "$${P}_workspace":/workspace \
		-v "$${P}_npm-cache":/home/maestro/.npm \
		-v "$${P}_mise-data":/home/maestro/.local/share/mise \
		-v "$${P}_mise-cache":/home/maestro/.cache/mise \
		-v "$${P}_aws-config":/home/maestro/.aws \
		-v "$${P}_playwright-cache":/home/maestro/.cache/ms-playwright \
		-e MAESTRO_CONFIG=/etc/maestro/config.toml \
		-e MAESTRO_HOME=/home/maestro \
		-e MAESTRO_DATA_DIR=/home/maestro/.maestro \
		-e CURSOR_CONFIG_DIR=/home/maestro/.cursor \
		-e "FIGMA_API_TOKEN=$${FIGMA_API_TOKEN:-}" \
		-e NODE_OPTIONS=--dns-result-order=ipv4first \
		"$$IMAGE" setup
else
	@P=$(PROJECT_NAME); \
	IMAGE=$$(docker images --format '{{.Repository}}:{{.Tag}}' | grep -E "(^|/)$${P}[-_]maestro:|^maestro[-_]maestro:" | head -1); \
	if [ -z "$$IMAGE" ]; then echo "ERROR: Maestro image not found. Run 'make build' first." >&2; exit 1; fi; \
	docker run --rm -it \
		-v "$$(pwd)/.maestro/config.toml":/etc/maestro/config.toml:ro \
		-v "$$(pwd)/.maestro/workflows":/etc/maestro/workflows:ro \
		-v "$$(pwd)/.maestro/maestro.env":/etc/maestro/env:ro \
		-v "$${P}_maestro-data":/home/maestro/.maestro \
		-v "$${P}_claude-auth":/home/maestro/.claude \
		-v "$${P}_cursor-auth":/home/maestro/.cursor \
		-v "$${P}_agents-data":/home/maestro/.agents \
		-v "$${P}_gh-auth":/home/maestro/.config/gh \
		-v "$${P}_acli-auth":/home/maestro/.config/acli \
		-v "$${P}_fcli-auth":/home/maestro/.config/fcli \
		-v "$${P}_workspace":/workspace \
		-v "$${P}_npm-cache":/home/maestro/.npm \
		-v "$${P}_mise-data":/home/maestro/.local/share/mise \
		-v "$${P}_mise-cache":/home/maestro/.cache/mise \
		-v "$${P}_aws-config":/home/maestro/.aws \
		-v "$${P}_playwright-cache":/home/maestro/.cache/ms-playwright \
		-e MAESTRO_CONFIG=/etc/maestro/config.toml \
		-e MAESTRO_HOME=/home/maestro \
		-e MAESTRO_DATA_DIR=/home/maestro/.maestro \
		-e CURSOR_CONFIG_DIR=/home/maestro/.cursor \
		-e "FIGMA_API_TOKEN=$${FIGMA_API_TOKEN:-}" \
		-e NODE_OPTIONS=--dns-result-order=ipv4first \
		"$$IMAGE" setup
endif

test: ## Smoke test: auth, worktree, agent hello, cleanup
ifeq ($(IS_PODMAN),1)
	$(COMPOSE) $(COMPOSE_FILES) run --rm -it maestro test-workflow
else
	$(COMPOSE) $(COMPOSE_FILES) run --rm -it maestro test-workflow
endif

logs: ## Tail all container logs
	$(COMPOSE) $(COMPOSE_FILES) logs -f

logs-maestro: ## Tail only the Maestro container
	$(COMPOSE) $(COMPOSE_FILES) logs -f maestro

ps: ## Show running containers
	$(COMPOSE) $(COMPOSE_FILES) ps

bash: ## Open a shell inside Maestro as the maestro user
ifeq ($(IS_PODMAN),1)
	@P=$(PROJECT_NAME); \
	IMAGE=$$(podman images --format '{{.Repository}}:{{.Tag}}' | grep -E "(^|/)$${P}[_-]maestro:|^maestro[-_]maestro:" | head -1); \
	if [ -z "$$IMAGE" ]; then echo "ERROR: Maestro image not found. Run 'make build' first." >&2; exit 1; fi; \
	podman run --rm -it --user maestro \
		--security-opt=label=disable \
		--entrypoint /bin/bash \
		-v "$$(pwd)/.maestro/config.toml":/etc/maestro/config.toml:ro \
		-v "$$(pwd)/.maestro/workflows":/etc/maestro/workflows:ro \
		-v "$$(pwd)/.maestro/maestro.env":/etc/maestro/env:ro \
		-v "$${P}_maestro-data":/home/maestro/.maestro \
		-v "$${P}_claude-auth":/home/maestro/.claude \
		-v "$${P}_cursor-auth":/home/maestro/.cursor \
		-v "$${P}_agents-data":/home/maestro/.agents \
		-v "$${P}_gh-auth":/home/maestro/.config/gh \
		-v "$${P}_acli-auth":/home/maestro/.config/acli \
		-v "$${P}_fcli-auth":/home/maestro/.config/fcli \
		-v "$${P}_workspace":/workspace \
		-v "$${P}_npm-cache":/home/maestro/.npm \
		-v "$${P}_mise-data":/home/maestro/.local/share/mise \
		-v "$${P}_mise-cache":/home/maestro/.cache/mise \
		-v "$${P}_aws-config":/home/maestro/.aws \
		-v "$${P}_playwright-cache":/home/maestro/.cache/ms-playwright \
		-e HOME=/home/maestro \
		-e MAESTRO_CONFIG=/etc/maestro/config.toml \
		-e MAESTRO_HOME=/home/maestro \
		-e MAESTRO_DATA_DIR=/home/maestro/.maestro \
		-e CURSOR_CONFIG_DIR=/home/maestro/.cursor \
		"$$IMAGE" -c 'cd /workspaces && exec bash'
else
	$(COMPOSE) $(COMPOSE_FILES) exec -u maestro -it -w /workspaces maestro bash
endif

exec: ## Open a shell inside Maestro (alias for bash)
ifeq ($(IS_PODMAN),1)
	@$(MAKE) bash
else
	$(COMPOSE) $(COMPOSE_FILES) exec -u maestro -it maestro bash
endif

worker-bash: ## Open a bash shell inside the running worker container for a ticket (usage: make worker-bash GH-45)
	$(eval _KEY := $(filter-out $@,$(MAKECMDGOALS)))
	@if [ -z "$(_KEY)" ]; then echo "ERROR: Ticket key required. Usage: make worker-bash GH-45" >&2; exit 1; fi; \
	SANITIZED=$$(echo "$(_KEY)" | tr '[:upper:]' '[:lower:]' | sed 's/[^a-z0-9]/-/g'); \
	CONTAINER=$$($(COMPOSE) $(COMPOSE_FILES) exec -T dind docker ps --filter "name=maestro-worker-$${SANITIZED}-" --format '{{.Names}}' | head -1); \
	if [ -z "$$CONTAINER" ]; then echo "ERROR: No running worker container found for $(_KEY). Is a step currently executing?" >&2; exit 1; fi; \
	echo "Opening bash in $$CONTAINER..."; \
	exec $(COMPOSE) $(COMPOSE_FILES) exec dind docker exec -it "$$CONTAINER" bash

%:
	@:

load-worker: ## Load worker image into DinD
	@IMAGE=$$($(_ENGINE) images --format '{{.Repository}}:{{.Tag}}' | grep -E "(^|/)$(PROJECT_NAME)[-_]maestro:" | head -1); \
	if [ -z "$$IMAGE" ]; then echo "ERROR: Maestro image not found. Run make build first." >&2; exit 1; fi; \
	echo "Waiting for DinD to be ready..."; \
	for i in $$(seq 1 30); do \
		if $(COMPOSE) $(COMPOSE_FILES) exec -T dind docker info >/dev/null 2>&1; then break; fi; \
		sleep 1; \
	done; \
	echo "Loading $$IMAGE into DinD..."; \
	$(_ENGINE) save "$$IMAGE" | $(COMPOSE) $(COMPOSE_FILES) exec -T dind docker load; \
	echo "Tagging as maestro:latest on DinD..."; \
	$(COMPOSE) $(COMPOSE_FILES) exec -T dind docker tag "$$IMAGE" maestro:latest

# ── Push multi-arch image to registry ──────────────────────────────────────────
# Works with Docker (buildx) or Podman (manifest).
# Docker: one-time setup: docker buildx create --name multiarch --use
# Auth: docker login ghcr.io / podman login ghcr.io
VERSION := $(shell cat VERSION)
HAS_BUILDX := $(shell $(_ENGINE) buildx version >/dev/null 2>&1 && echo 1 || echo 0)

push: ## Build + push multi-arch image (amd64 + arm64)
ifeq ($(HAS_BUILDX),1)
	docker buildx build \
		--platform linux/amd64,linux/arm64 \
		--build-arg MAESTRO_VERSION=$(VERSION) \
		-t $(REGISTRY):$(VERSION) \
		-t $(REGISTRY):latest \
		--push .
else
	$(_ENGINE) manifest rm $(REGISTRY):$(VERSION) 2>/dev/null || true
	$(_ENGINE) build \
		--platform linux/amd64,linux/arm64 \
		--build-arg MAESTRO_VERSION=$(VERSION) \
		--manifest $(REGISTRY):$(VERSION) .
	$(_ENGINE) manifest push $(REGISTRY):$(VERSION) docker://$(REGISTRY):$(VERSION)
	$(_ENGINE) manifest push $(REGISTRY):$(VERSION) docker://$(REGISTRY):latest
endif

push-arm64: ## Build + push arm64 image only
ifeq ($(HAS_BUILDX),1)
	docker buildx build \
		--platform linux/arm64 \
		--build-arg MAESTRO_VERSION=$(VERSION) \
		-t $(REGISTRY):$(VERSION) \
		-t $(REGISTRY):latest \
		--push .
else
	$(_ENGINE) manifest rm $(REGISTRY):$(VERSION) 2>/dev/null || true
	$(_ENGINE) build \
		--platform linux/arm64 \
		--build-arg MAESTRO_VERSION=$(VERSION) \
		--manifest $(REGISTRY):$(VERSION) .
	$(_ENGINE) manifest push $(REGISTRY):$(VERSION) docker://$(REGISTRY):$(VERSION)
	$(_ENGINE) manifest push $(REGISTRY):$(VERSION) docker://$(REGISTRY):latest
endif

push-amd64: ## Build + push amd64 image only
ifeq ($(HAS_BUILDX),1)
	docker buildx build \
		--platform linux/amd64 \
		--build-arg MAESTRO_VERSION=$(VERSION) \
		-t $(REGISTRY):$(VERSION) \
		-t $(REGISTRY):latest \
		--push .
else
	$(_ENGINE) manifest rm $(REGISTRY):$(VERSION) 2>/dev/null || true
	$(_ENGINE) build \
		--platform linux/amd64 \
		--build-arg MAESTRO_VERSION=$(VERSION) \
		--manifest $(REGISTRY):$(VERSION) .
	$(_ENGINE) manifest push $(REGISTRY):$(VERSION) docker://$(REGISTRY):$(VERSION)
	$(_ENGINE) manifest push $(REGISTRY):$(VERSION) docker://$(REGISTRY):latest
endif

clean-dind: ## Clean up DinD dangling images and volumes
	@echo "Cleaning up DinD dangling images and volumes..."; \
	$(COMPOSE) $(COMPOSE_FILES) exec -T dind docker system prune -f || true; \
	echo "DinD cleanup complete. Run 'make load-worker' to reload maestro:latest if needed."

restart: stop start ## Restart (stop + start)

check: ## Run the same gates CI runs (fast subset — no network/docker)
	./scripts/preflight.sh

check-full: ## Run all CI gates including gitleaks, cargo-deny, cargo-audit, npm audit
	./scripts/preflight.sh --full

# ── DB backups ────────────────────────────────────────────────────────
# Both targets stream the dump from the running DB container through
# gzip on the host into a timestamped file under `$(DUMP_DIR)/`. The
# corresponding `BACKEND=...` stack must already be running (e.g.
# `make start BACKEND=postgres`).

backup-postgres: ## Dump the Postgres DB to dump/postgres-<timestamp>.sql.gz
	@mkdir -p $(DUMP_DIR)
	@TIMESTAMP=$$(date +%Y%m%d-%H%M%S); \
	OUT="$(DUMP_DIR)/postgres-$$TIMESTAMP.sql.gz"; \
	PGUSER=$${POSTGRES_USER:-maestro}; \
	PGDB=$${POSTGRES_DB:-maestro}; \
	echo "Dumping Postgres ($$PGDB as $$PGUSER) -> $$OUT"; \
	$(COMPOSE) -f docker-compose.yml -f docker-compose.postgres.yml exec -T postgres \
	  pg_dump --clean --if-exists -U "$$PGUSER" -d "$$PGDB" \
	  | gzip > "$$OUT"; \
	echo "Backup complete: $$OUT ($$(du -h "$$OUT" | cut -f1))"

backup-mariadb: ## Dump the MariaDB DB to dump/mariadb-<timestamp>.sql.gz
	@mkdir -p $(DUMP_DIR)
	@TIMESTAMP=$$(date +%Y%m%d-%H%M%S); \
	OUT="$(DUMP_DIR)/mariadb-$$TIMESTAMP.sql.gz"; \
	ROOT_PASS=$${MARIADB_ROOT_PASSWORD:-root}; \
	DB=$${MARIADB_DATABASE:-maestro}; \
	echo "Dumping MariaDB ($$DB as root) -> $$OUT"; \
	$(COMPOSE) -f docker-compose.yml -f docker-compose.mariadb.yml exec -T -e MYSQL_PWD="$$ROOT_PASS" mariadb \
	  mariadb-dump --single-transaction --add-drop-table -u root "$$DB" \
	  | gzip > "$$OUT"; \
	echo "Backup complete: $$OUT ($$(du -h "$$OUT" | cut -f1))"
