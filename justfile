
default:
    @just --list

dev log="subtidal=debug":
    @RUST_LOG={{log}} SUBTIDAL_TOKEN_FILE=/tmp/subtidal-tokens.json cargo run

# Run the server in docker with a loglevel, eg. `just docker subtidal=debug`.
docker loglevel="info":
    @RUST_LOG={{loglevel}} docker compose up -d
