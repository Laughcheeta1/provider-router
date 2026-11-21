.PHONY: dev build test check

CARGO ?= cargo

dev:
	$(CARGO) run --locked --

build:
	$(CARGO) build --locked --release

test:
	$(CARGO) test --locked

check:
	$(CARGO) fmt --all -- --check
	$(CARGO) clippy --locked --all-targets -- -D warnings
