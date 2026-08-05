# TODO

## Design decisions

- **ID scheme** — prefixed, reversible IDs encoding the Tidal ID: `t<track>`, `al<album>`, `ar<artist>`, `pl<playlist>`. Deterministic across sessions, no database (src/navidrome/ids.rs). Lenient parse: bare number = raw Tidal track ID.
- **Entity mapping** — Tidal v1 camelCase JSON → Subsonic models in src/tidal/mapping.rs. Song: title, artist (multi-artist joins "A feat. B"), album, albumId, artistId, duration, year from releaseDate, genre, trackNumber, discNumber, coverArt = al<albumId>. Omit bitRate/size (quality-dependent).
- **Cover art** — Tidal images on resources.tidal.com are long-lived. getCoverArt will 302-redirect to the resolved URL; cache id → URL in moka. Not yet built.
- **Favorites/playlists** — map star/unstar/getStarred to Tidal favorites CRUD; playlists to Tidal user playlists. Tidal has no public play counts; optional local JSON store for Navidrome-parity playCount.
- **Scrobble middleware** — `PlayReporter` trait, fan-out from the `scrobble` handler, best-effort (errors log, never fail the client request). Last.fm reporter: api_key + session key (sk), one-time auth.getToken → authorize → getSession, sk stored in Keychain. ListenBrainz: plain token, POST /1/submit-listen. Config = optional blocks in settings.toml.

## Next: catalog endpoints

### Search & browse

- [x] Add search3 — maps Tidal /v1/search to searchResult3 (artist/album/song), honors count/offset params (handlers.rs:72)
- [x] Add ID parser — src/navidrome/ids.rs, prefixed encode/decode + tests
- [x] Map getUser — returns the Tidal account profile (GET /v1/users/{id}, cached), ignoring the passed username; auth reads settings.toml; roles reflect the bridge; scrobblingEnabled false until middleware lands
- [ ] Add getMusicFolders — return a single "Tidal" folder
- [ ] Add getIndexes + getArtists — expose favorited artists as the library index
- [ ] Add getArtist — Tidal artist detail with top tracks and albums
- [ ] Add getAlbum — Tidal album detail, map tracks to Subsonic children

### Lists & playlists

- [ ] Add getAlbumList2 — map Tidal discovery and favorites (newest, frequent, recent, random)
- [ ] Add getPlaylists / getPlaylist — map Tidal user playlists and their items
- [ ] Add createPlaylist / updatePlaylist / deletePlaylist — Tidal playlist CRUD
- [ ] Add addToPlaylist / removeFromPlaylist — modify Tidal playlist contents

### Favorites & art

- [ ] Add getStarred / getStarred2 — favorited artists, albums, songs
- [ ] Add star / unstar — mutate Tidal favorites
- [x] Add getCoverArt — 302 redirect to resolved Tidal image URL; accepts al<id>/ar<id>/bare album id; size snaps to valid Tidal dimensions (album 160/320/640/1280, artist 160/320/480/750)

## Next: playback

- [ ] Add stream endpoint — resolve Subsonic track ID to Tidal track, call playbackinfopostpaywall, serve audio
- [ ] Rewrite DASH manifest into HLS playlist — m3u8 with EXT-X-MAP init segment pointing at Tidal CDN; zero server bandwidth; works only on HLS-sniffing clients (Symfonium, play:Sub, browsers)
- [ ] Add byte-proxy fallback — fetch init + segments server-side and return concatenated audio for raw-audio clients (DSub, Substreamer)
- [ ] Map Subsonic maxBitRate to Tidal quality — 0/unspecified → LOSSLESS, <320 → HIGH, etc. (LOW/HIGH/LOSSLESS/HI_RES)
- [ ] Parse MPD fully — segment templates and multiple representations; current extract_dash_url only grabs the first BaseURL (src/tidal/client.rs:482)

## Next: scrobble middleware

- [ ] Define PlayReporter trait — report(song, timestamp); fan out from scrobble handler; errors log only
- [ ] Add Last.fm reporter — api_key + sk; one-time auth.getToken → browser authorize → getSession; store sk in Keychain; track.scrobble + updateNowPlaying
- [ ] Add ListenBrainz reporter — plain token, POST /1/submit-listens
- [ ] Flip scrobblingEnabled to true — getUser reflects configured reporters

## Decided, not started

- [ ] Add tidal_quality setting — optional override in settings.toml for stream quality

## Housekeeping

- [ ] Delete empty src/tidal/tidal_auth.rs — unreferenced leftover, still untracked

## Done

- [x] Fix device-auth parsing — Tidal returns camelCase JSON (deviceCode, userId); added serde(rename_all = "camelCase") to DeviceAuth and Session
- [x] Detect web-player client_ids — sub_status 1002 / "not a Limited Input Device client" → clear auth error instead of panic
- [x] Store tokens in macOS Keychain — keyring crate v4.1.6, service HighTide/account tidal; removed plaintext tidal_tokens.json
- [x] Auto-present login at startup — no login CLI arg; needs_login() checks keyring, runs device-code flow when missing/expired
- [x] Add Tidal client — device-code login, token refresh, cached authenticated GETs, stream URL fetch
- [x] Embed Tidal credentials XOR-obfuscated — scripts/gen_embedded.py generates src/tidal/embedded.rs; real values embedded
- [x] Add getUser endpoint — role flags as strings, nested user, matches documented OpenSubsonic shape
- [x] Implement token auth — t + s (md5(password + salt)), p plaintext, p=enc:<hex>; error codes 10/40/70
- [x] Typed settings — config crate, Settings struct in OnceLock, settings.toml
