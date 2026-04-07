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
#   make exec          — open a shell inside Maestro as the maestro user
#   make restart       — down + up

# Auto-detect compose provider. Prefer podman if present, else docker.
# Both work identically with explicit -f flags.
COMPOSE := $(shell command -v podman >/dev/null 2>&1 && echo "podman compose" || echo "docker compose")

# podman-compose needs --podman-run-args BEFORE -f for interactive commands.
# Detect the native podman-compose binary for those cases.
PODMAN_COMPOSE_BIN := $(shell command -v podman-compose 2>/dev/null)
IS_PODMAN := $(shell command -v podman >/dev/null 2>&1 && echo 1 || echo 0)

# Set DIND=0 to run without the Docker-in-Docker sidecar.
DIND ?= 1
COMPOSE_FILES := -f docker-compose.yml
ifeq ($(DIND),1)
COMPOSE_FILES += -f docker-compose.dind.yml
endif

# Resolve the actual image name for the maestro service (compose may prefix with project name).
MAESTRO_IMAGE = $(shell $(COMPOSE) $(COMPOSE_FILES) images maestro --format '{{.Repository}}:{{.Tag}}' 2>/dev/null | head -1)

.PHONY: build up down setup test logs logs-maestro ps exec restart load-worker clean-dind

build:
	@mkdir -p skills
	$(COMPOSE) $(COMPOSE_FILES) build

up:
	$(COMPOSE) $(COMPOSE_FILES) up -d
ifeq ($(DIND),1)
	@$(MAKE) --no-print-directory load-worker
endif

down:
	$(COMPOSE) $(COMPOSE_FILES) down

setup:
ifeq ($(IS_PODMAN),1)
	$(PODMAN_COMPOSE_BIN) --podman-run-args="-it --network=host" $(COMPOSE_FILES) run --rm maestro setup
else
	$(COMPOSE) $(COMPOSE_FILES) run --rm -it --network=host maestro setup
endif

test:
ifeq ($(IS_PODMAN),1)
	$(PODMAN_COMPOSE_BIN) --podman-run-args="-it" $(COMPOSE_FILES) run --rm maestro test-workflow
else
	$(COMPOSE) $(COMPOSE_FILES) run --rm -it maestro test-workflow
endif

logs:
	$(COMPOSE) $(COMPOSE_FILES) logs -f

logs-maestro:
	$(COMPOSE) $(COMPOSE_FILES) logs -f maestro

ps:
	$(COMPOSE) $(COMPOSE_FILES) ps

exec:
ifeq ($(IS_PODMAN),1)
	podman exec -u maestro -it maestro bash
else
	$(COMPOSE) $(COMPOSE_FILES) exec -u maestro -it maestro bash
endif

load-worker:
	@IMAGE=$$(podman images --format '{{.Repository}}:{{.Tag}}' | grep maestro_maestro | head -1); \
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
