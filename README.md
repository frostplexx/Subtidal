This project is in an alpha state!

# Subtidal

A Subsonic API server that proxies Tidal.

## Getting Started (Docker)

Prerequisites: Docker with Compose support.

1. Copy `docker-compose.yml` and `settings.toml`
2. Run `docker compose up -d`.
3. The first start prints a Tidal device-code URL. Open `docker compose logs -f subtidal` and complete the login once. The session persists in the `subtidal-data` volume.
4. Point a Subsonic client at `http://localhost:8000` and log in with `admin` / `admin`.

The server reads `./settings.toml` through the Compose mount. Edit the file and run `docker compose restart`, or map any other file:

```yaml
- /path/to/your/settings.toml:/config/subtidal/settings.toml:ro
```

Adjust the log level with `RUST_LOG=subtidal=debug docker compose up -d`.

To run the published image directly:

```
docker run -d -p 8000:8000 \
  -e APP_USERNAME=admin -e APP_PASSWORD=admin -e APP_PORT=8000 \
  -v subtidal-data:/data \
  ghcr.io/frostplexx/subtidal:latest
```

`APP_*` env vars are the settings for this path. The image stores the Tidal token at `/data/tokens.json`.

If no image is published yet, clone the repo and run `docker compose up -d --build` from it.

## Local development

Prerequisites: a recent Rust toolchain (edition 2024). The Nix flake provides one: `nix develop`.

1. Run `just dev` or `cargo run`.
2. Complete the device-code login on first start.
3. Connect with `admin` / `admin` from `settings.toml`.

Source layout:

```
src/
├── main.rs            # startup, settings, login, server bind
├── settings.rs        # settings file discovery + APP_* env overrides
├── navidrome/         # Subsonic protocol layer
│   ├── routes.rs      # /rest/* routes
│   ├── auth.rs        # credential checks, body cap, rate limiting
│   ├── params.rs      # query param structs
│   ├── models/        # Subsonic DTOs
│   └── handlers/      # one module per endpoint group
└── tidal/
    ├── client/        # Tidal API calls
    ├── mapping/       # Tidal JSON -> Subsonic DTOs
    └── embedded.rs    # Tidal app credentials
```
