default: lint test

build:
  cargo build

build-release:
  cargo build --release

test:
  cargo test

test-accept:
  INSTA_UPDATE=always cargo test

fmt:
  cargo fmt --all

install:
  cargo install --path .

lint:
  cargo clippy --all-targets -- -D warnings
  cargo fmt --all --check

run *FILES:
  cargo run -- -r custom-rules {{FILES}}
