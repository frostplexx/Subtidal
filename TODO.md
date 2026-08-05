# TODO

## Next: catalog endpoints

### Search & browse

- [ ] Add search3 — map Tidal /v1/search results to Subsonic searchResult3 (artists, albums, songs)
- [ ] Add getMusicFolders — return a single "Tidal" folder
- [ ] Add getIndexes + getArtists — expose favorited artists as the library index
- [ ] Add getArtist — Tidal artist detail with top tracks and albums
- [ ] Add getAlbum — Tidal album detail, map tracks to Subsonic children
- [ ] Add getSong — Tidal track to Subsonic child

### Lists & playlists

- [ ] Add getAlbumList2 — map Tidal discovery and favorites (newest, frequent, recent, random)
- [ ] Add getPlaylists / getPlaylist — map Tidal user playlists and their items
- [ ] Add createPlaylist / updatePlaylist / deletePlaylist — Tidal playlist CRUD
- [ ] Add addToPlaylist / removeFromPlaylist — modify Tidal playlist contents

### Favorites & art

- [ ] Add getStarred / getStarred2 — favorited artists, albums, songs
- [ ] Add star / unstar — mutate Tidal favorites
- [ ] Add getCoverArt — proxy Tidal image URLs to a local endpoint; CDN URLs are short-lived

## Next: playback

- [ ] Add stream endpoint — resolve Subsonic track ID to Tidal track, call playbackinfopostpaywall, serve audio
- [ ] Rewrite DASH manifest into HLS playlist — m3u8 with EXT-X-MAP init segment pointing at Tidal CDN; zero server bandwidth; works only on HLS-sniffing clients (Symfonium, play:Sub, browsers)
- [ ] Add byte-proxy fallback — fetch init + segments server-side and return concatenated audio for raw-audio clients (DSub, Substreamer)
- [ ] Map Subsonic maxBitRate to Tidal quality — 0/unspecified → LOSSLESS, <320 → HIGH, etc. (LOW/HIGH/LOSSLESS/HI_RES)
- [ ] Parse MPD fully — segment templates and multiple representations; current extract_dash_url only grabs the first BaseURL (src/tidal/client.rs:482)

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
