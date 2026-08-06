
default:
    @just --list

dev:
    @RUST_LOG=subtidal=debug SUBTIDAL_TOKEN_FILE=/tmp/subtidal-tokens.json cargo run
