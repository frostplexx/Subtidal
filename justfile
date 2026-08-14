
default:
    @just --list

dev:
    @RUST_LOG=subtidal=debug SUBTIDAL_TOKEN_FILE=/tmp/subtidal-tokens.json cargo run

# Run the server in docker with a loglevel, eg. `just docker subtidal=debug`.
docker loglevel="info":
    @RUST_LOG={{loglevel}} docker compose up -d
