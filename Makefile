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
#   make logs          — tail all container logs
#   make logs-maestro  — tail only the Maestro container
#   make ps            — show running containers
#   make exec          — open a shell inside Maestro as the maestro user
#   make restart       — down + up

# Auto-detect compose provider. Prefer podman if present, else docker.
# Both work identically with explicit -f flags.
COMPOSE := $(shell command -v podman >/dev/null 2>&1 && echo "podman compose" || echo "docker compose")

# Set DIND=0 to run without the Docker-in-Docker sidecar.
DIND ?= 1
COMPOSE_FILES := -f docker-compose.yml
ifeq ($(DIND),1)
COMPOSE_FILES += -f docker-compose.dind.yml
endif

.PHONY: build up down setup logs logs-maestro ps exec restart

build:
	$(COMPOSE) $(COMPOSE_FILES) build

up:
	$(COMPOSE) $(COMPOSE_FILES) up -d

down:
	$(COMPOSE) $(COMPOSE_FILES) down

setup:
	$(COMPOSE) $(COMPOSE_FILES) run --rm -it maestro setup

logs:
	$(COMPOSE) $(COMPOSE_FILES) logs -f

logs-maestro:
	$(COMPOSE) $(COMPOSE_FILES) logs -f maestro

ps:
	$(COMPOSE) $(COMPOSE_FILES) ps

exec:
	$(COMPOSE) $(COMPOSE_FILES) exec -u maestro -it maestro bash

restart: down up
