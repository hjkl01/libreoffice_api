.PHONY: help build release run check fmt fmt-check test clippy clean docker-build docker-up docker-down docker-logs swagger

IMAGE ?= libreoffice-api:latest
PORT ?= 8080

help:
	@echo "Available targets:"
	@echo "  build        Build debug binary"
	@echo "  release      Build release binary"
	@echo "  run          Run the API locally"
	@echo "  check        Check the Rust project"
	@echo "  fmt          Format Rust source"
	@echo "  fmt-check    Check Rust formatting"
	@echo "  test         Run tests"
	@echo "  clippy       Run Clippy"
	@echo "  clean        Remove Rust build artifacts"
	@echo "  docker-build Build Docker image"
	@echo "  docker-up    Start the service with Docker Compose"
	@echo "  docker-down  Stop the service"
	@echo "  docker-logs  Follow service logs"
	@echo "  swagger      Open Swagger UI URL"

build:
	cargo build

release:
	cargo build --release

run:
	cargo run

check:
	cargo check

fmt:
	cargo fmt

fmt-check:
	cargo fmt -- --check

test:
	cargo test

clippy:
	cargo clippy --all-targets --all-features -- -D warnings

clean:
	cargo clean

docker-build:
	docker build -t $(IMAGE) .

docker-up:
	docker compose up -d --build

docker-down:
	docker compose down

docker-logs:
	docker compose logs -f libreoffice-api

swagger:
	@echo "Swagger UI: http://localhost:$(PORT)/swagger-ui/"
	@echo "OpenAPI JSON: http://localhost:$(PORT)/api-docs/openapi.json"
