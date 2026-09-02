<p align="center">    
  <img width="256" alt="Subtdial" src="https://github.com/user-attachments/assets/7e0188f8-586d-40d6-bdca-fe8a6c0b73ed" />
  <h1 align="center">Subtidal</h1>
</p>

> [!WARNING]
> This project is in an beta state! Breaking changes might occur.

Subtidal exposes your Tidal library through the OpenSubsonic API, so you can use any Subsonic-compatible client to browse, search, and stream Tidal's catalog as if it were your own self-hosted music server.

Some highlighted features are:
- Allows you to search and play of the whole Tidal catalogue
- Gives you access to your liked songs, artists, and albums
- Allows to play, create, edit and delete your playlists
- Play Tidal mixes (Daily Mix, My Mix, Discovery) as read-only playlists
- Scrobbles to either last.fm or listenbrainz
- Show AI labels on AI-generated music
- Optional Word-by-word synced provided by [Radiant Lyrics](https://radiant-lyrics.org)


## Getting Started 

> [!WARNING]
> A paid Tidal account is required!

Prerequisites: Docker with Compose support.

1. Copy `docker-compose.yml` and `settings.toml`
2. Edit `settings.toml`
   - Choose a username and password
   - Optionally set up either last.fm or listenbrainz scrobbling
4. Run `docker compose up -d`.
5. The first start prints a Tidal device-code URL. Open `docker compose logs -f subtidal` and complete the login once. If you have last.fm set up, repeat this step for that service too.
6. Point a Subsonic client at `http://localhost:8000` and log in with the username and password chosen in step 2.

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

## Local development

Prerequisites: a recent Rust nightly toolchain (edition 2024). The Nix flake provides one via rustup, pinned in `rust-toolchain.toml`: `nix develop`.

1. Run `just dev` or `cargo run`.
2. Complete the device-code login on first start.
3. Connect with `admin` / `admin` from `settings.toml`.
