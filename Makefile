# Maestro — container management shortcuts
#
# Wraps podman/docker compose with the correct -f flags.
# DinD sidecar is included by default; set DIND=0 to disable.
#
# Usage:
#   make build         — build the Maestro image
#   make up            — start Maestro + DinD sidecar
#   make down          — stop and remove containers
#   make setup         — interactive first-time auth setup
#   make test          — smoke test: auth, worktree, agent hello, cleanup
#   make logs          — tail all container logs
#   make logs-maestro  — tail only the Maestro container
#   make ps            — show running containers
#   make bash          — open a shell inside Maestro as the maestro user
#   make exec          — open a shell inside Maestro as the maestro user
#   make restart       — down + up

# Auto-detect compose provider. Prefer podman if present, else docker.
# Both work identically with explicit -f flags.
COMPOSE := $(shell command -v podman >/dev/null 2>&1 && echo "podman compose" || echo "docker compose")
# Verify the detected compose command actually works (the docker CLI may lack the compose plugin).
HAS_COMPOSE := $(shell $(COMPOSE) version >/dev/null 2>&1 && echo 1 || echo 0)

IS_PODMAN := $(shell command -v podman >/dev/null 2>&1 && echo 1 || echo 0)
# Detect standalone podman-compose binary (needs --podman-run-args for -it).
PODMAN_COMPOSE_BIN := $(shell command -v podman-compose 2>/dev/null)
# Project name = current directory name (matches podman-compose volume naming convention).
PROJECT_NAME := $(shell basename $(CURDIR))

# Set DIND=0 to run without the Docker-in-Docker sidecar.
DIND ?= 1
COMPOSE_FILES := -f docker-compose.yml
ifeq ($(DIND),1)
COMPOSE_FILES += -f docker-compose.dind.yml
endif

# Resolve the actual image name for the maestro service (compose may prefix with project name).
MAESTRO_IMAGE = $(shell $(COMPOSE) $(COMPOSE_FILES) images maestro --format '{{.Repository}}:{{.Tag}}' 2>/dev/null | head -1)

.PHONY: build build-local up down setup test logs logs-maestro ps bash exec restart load-worker clean-dind ui-build

ui-build:
	@echo "Building React dashboard..."
	cd ui && npm install --legacy-peer-deps && npm run build

build: ui-build
	@echo "Building Rust workspace..."
	cargo build
ifeq ($(HAS_COMPOSE),1)
	$(COMPOSE) $(COMPOSE_FILES) build || (echo "ERROR: Image build failed. Check the output above." >&2; exit 1)
else
	@echo "NOTE: docker/podman compose not available — skipping container image build."
endif

build-local:
	@if command -v docker >/dev/null 2>&1 && ! docker --version 2>&1 | grep -qi podman; then \
		echo "Building with Docker..."; \
		docker build --platform linux/amd64 --build-arg MAESTRO_VERSION=$$(cat VERSION) -t maestro:local-test .; \
	elif command -v podman >/dev/null 2>&1; then \
		echo "Building with Podman..."; \
		podman build --platform linux/amd64 --build-arg MAESTRO_VERSION=$$(cat VERSION) -t maestro:local-test .; \
	else \
		echo "ERROR: Neither docker nor podman found." >&2; exit 1; \
	fi

up:
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

down:
	$(COMPOSE) $(COMPOSE_FILES) down

setup:
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
		--network=host \
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
		--network host \
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

test:
ifeq ($(IS_PODMAN),1)
	$(COMPOSE) $(COMPOSE_FILES) run --rm -it maestro test-workflow
else
	$(COMPOSE) $(COMPOSE_FILES) run --rm -it maestro test-workflow
endif

logs:
	$(COMPOSE) $(COMPOSE_FILES) logs -f

logs-maestro:
	$(COMPOSE) $(COMPOSE_FILES) logs -f maestro

ps:
	$(COMPOSE) $(COMPOSE_FILES) ps

bash:
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
		"$$IMAGE"
else
	$(COMPOSE) $(COMPOSE_FILES) exec -u maestro -it maestro bash
endif

exec:
ifeq ($(IS_PODMAN),1)
	@$(MAKE) bash
else
	$(COMPOSE) $(COMPOSE_FILES) exec -u maestro -it maestro bash
endif

load-worker:
	@IMAGE=$$(podman images --format '{{.Repository}}:{{.Tag}}' | grep -E "maestro[-_]maestro" | head -1); \
	if [ -z "$$IMAGE" ]; then echo "ERROR: Maestro image not found. Run make build first." >&2; exit 1; fi; \
	echo "Waiting for DinD to be ready..."; \
	for i in $$(seq 1 30); do \
		if podman exec maestro-dind docker info >/dev/null 2>&1; then break; fi; \
		sleep 1; \
	done; \
	echo "Loading $$IMAGE into DinD..."; \
	podman save "$$IMAGE" | podman exec -i maestro-dind docker load; \
	echo "Tagging as maestro:latest on DinD..."; \
	podman exec maestro-dind docker tag "$$IMAGE" maestro:latest

clean-dind:
	@echo "Cleaning up DinD dangling images and volumes..."; \
	podman exec maestro-dind docker system prune -f || true; \
	echo "DinD cleanup complete. Run 'make load-worker' to reload maestro:latest if needed."

restart: down up
