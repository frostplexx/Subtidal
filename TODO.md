# TODO

## Design decisions

- **ID scheme** — prefixed, reversible IDs encoding the Tidal ID: `t<track>`, `al<album>`, `ar<artist>`, `pl<playlist>`. Deterministic across sessions, no database (src/navidrome/ids.rs). Lenient parse: bare number = raw Tidal ID.
- **Entity mapping** — Tidal v1 camelCase JSON → Subsonic models in src/tidal/mapping/. Song: title, artist (multi-artist joins "A feat. B"), album, albumId, artistId, duration, year from releaseDate, genre, trackNumber, discNumber, contentType/suffix placeholder ("flac") until streaming lands.
- **Cover art** — never proxied: album/artist/playlist responses carry full `resources.tidal.com` image URLs in `coverArt`; getCoverArt 302-redirects to the resolved URL. Album/artist pics live at 160/320/640/1280, artist portraits 160/320/480/750.
- **Favorites/playlists** — star/unstar/getStarred map to Tidal favorites CRUD; playlists to Tidal user playlists. Tidal has no public play counts; optional local JSON store for Navidrome-parity playCount.
- [x] scrobble endpoint — accepts single/multiple id, submission, time; returns ok and logs the report (no backend yet); commit cbfa61a
- **Scrobble middleware** — `PlayReporter` trait, fan-out from the `scrobble` handler, best-effort (errors log, never fail the client request). Last.fm reporter: api_key + session key (sk), one-time auth.getToken → authorize → getSession, sk stored in Keychain. ListenBrainz: plain token, POST /1/submit-listen. Config = optional blocks in settings.toml.

## Next: catalog endpoints

### Browsing

- [x] Add getMusicFolders — single "Tidal" folder id 1, matches getUser folder list; getScanStatus — scanning=false, count=0 (no local library; Arpeggi may prompt a scan); commit 85e2d3a
- [ ] Add getIndexes + getArtists — expose favorited artists as the library index
- [x] Add getSong — single track detail; Feishin calls it from song context menus
- [ ] Add getMusicDirectory — album dir listing; some clients use it instead of getAlbum
- [ ] Add getAlbumInfo2 — album info (notes/artists); Tidal has no album notes, would be mostly empty

### Lists & playlists

- [x] Add getPlaylist — single playlist with its entries, paginated at 100 (Tidal's items cap); bad id → 70, missing → 10; commit 123f071
- [ ] Add createPlaylist / updatePlaylist / deletePlaylist — Tidal playlist CRUD
- [ ] Add addToPlaylist / removeFromPlaylist — modify Tidal playlist contents

### Favorites & art

- [x] Add star / unstar — mutate Tidal favorites; id/albumId/artistId, repeats allowed; cache invalidated on change; commit f8701c9

## Next: playback

- [x] Add stream endpoint — resolve Subsonic track ID to Tidal track, call playbackinfopostpaywall, serve audio: 302-redirect to the single-file CDN URL (BTS), zero server bandwidth; quality via maxBitRate/format → tidal_quality; hi-res tracks answer segmented DASH, handler falls back to HIGH; commit ba99373
- [x] Map Subsonic maxBitRate to Tidal quality — 0/unspecified → LOSSLESS (cascades to what the account/track offers), 1-64 → LOW (HE-AAC 96k), 65-320 → HIGH (AAC 320k), >320 → LOSSLESS; format=flac lifts to LOSSLESS, lossy formats cap at HIGH (handlers/tracks.rs tidal_quality)
- [x] Rewrite DASH manifest into HLS playlist — format=hls requests HI_RES and returns an m3u8 (EXT-X-MAP + numbered segments, absolute Tidal CDN URLs, zero server bandwidth); tracks without a hi-res master 302 to AAC; FLAC-in-fMP4 HLS plays in mpv/VLC/ExoPlayer, not hls.js; verify on a real network (sandbox proxy can't reach sp-ad-fa); commit 753bc25
- [x] Parse MPD fully — regex-based DashInfo in src/tidal/client/stream.rs: segment template (init/media/timescale/startNumber), SegmentTimeline (d/r runs), picks the highest-bandwidth representation; unit-tested against the live hi-res MPD shape
- [ ] Byte-serving hi-res (byte-proxy concat or ffmpeg remux to FLAC) — DECLINED by decision: the only universal path for non-HLS clients (Feishin, DSub) is server egress ≈ file size per play; user chose to keep zero server bandwidth; hi-res stays HLS-only (format=hls), default stream stays 302 to AAC
- [ ] Set real contentType/suffix per stream — placeholder "audio/flac"/"flac" in Child; the LOSSLESS tier usually cascades to AAC, so the placeholder is wrong once streaming works (src/tidal/mapping/song.rs)

## Next: scrobble middleware

- [x] Add updateNowPlaying + getNowPlaying — legacy updateNowPlaying aliased (VeloSonic still calls it; OpenSubsonic replaced it with scrobble submission=false); shared now-playing slot in navidrome/now_playing.rs, replaced per report, expired after 10 min; scrobble feeds it too; entry = full song + username/minutesAgo/playerId; commit 4b41274
- [x] Add reportPlayback — playbackReport extension (v1, already advertised); states starting/playing/paused/stopped drive the slot; stopped clears it and logs the completion (future PlayReporter hook); positionMs estimated forward from the last report while playing (playbackRate aware), frozen on pause; getNowPlaying entries gain state/positionMs/playbackRate; mediaType restricted to song; commit <TBD>
- [x] Add getSimilarSongs2 — seed artist top tracks + 3 closest similar artists' top tracks, shuffled, truncated to count (default 50); similar-artist fetch failure degrades to a warning; commit 4b41274
- [x] Add getArtistInfo — same payload as getArtistInfo2 wrapped as artistInfo; shared artist_info() core; commit 4b41274
- [x] Add getLyricsBySongId — Tidal /tracks/{id}/lyrics (plain + LRC subtitles); LRC parsed to synced line[] (first timestamp per line, metadata tags skipped), plain fallback; 404 → empty list ok; v1 shape, kind omitted unless enhanced=true; commit 4b41274
- [ ] Define PlayReporter trait — report(song, timestamp); fan out from scrobble handler; errors log only
- [ ] Add Last.fm reporter — api_key + sk; one-time auth.getToken → browser authorize → getSession; store sk in Keychain; track.scrobble + updateNowPlaying
- [ ] Add ListenBrainz reporter — plain token, POST /1/submit-listens
- [ ] Flip scrobblingEnabled to true — getUser reflects configured reporters

## Known limitations

- getNowPlaying serves a single shared slot (single-user server); entries expire after ten minutes without a report
- reportPlayback accepts mediaType=song only; podcasts are unsupported (mediaType=podcast fails with code 0)

## Decided, not started

- [ ] Add tidal_quality setting — optional override in settings.toml for stream quality

## Housekeeping

- [ ] Delete empty src/tidal/tidal_auth.rs — unreferenced leftover, still untracked

## Done

- [x] Add getSong — single track via /tracks/{id}; t<id> or bare number; year filled from album detail (track JSON has no releaseDate, album fetch hits meta cache); commit fee3b8e

- [x] Add getArtistInfo2 — bio (wimpLink markup stripped) + portraits 160/480/750 + similarArtist via /artists/{id}/bio + /similar; id resolves artist/album/song → first artist; musicBrainzId/lastFmUrl empty (Tidal exposes neither); commits f62486c + b953f2b (strip fix)

- [x] Add getStarred / getStarred2 — favorited artists, albums, songs from /favorites/{albums,artists,tracks} (parallel); favorite time → starred; getStarred legacy shapes, getStarred2 ID3 + artistImageUrl; Child gains starred field; commit 8014388

- [x] Fix album/artist covers — `coverArt` serialized as `cover_art` (missing serde rename); Feishin reads `coverArt`. Playlist covers: getCoverArt accepted only album ids; root cause was the `id` param parse (serde_urlencoded rejects Vec) breaking the whole query
- [x] Add getTopSongs — artist's top tracks via /artists/{id}/toptracks; id param wins (topSongsByArtistId extension v1, advertised), artist name resolves via search, count defaults 50; commit 3996b56
- [x] Add getAlbum — album detail + tracks in track order via /albums/{id} + /albums/{id}/tracks; album year fills in for tracks (no releaseDate on track JSON); commit ab43bf4
- [x] Add getArtist — artist detail + albums via /artists/{id} + /artists/{id}/albums; albumCount = albums returned (Tidal detail reports none); commit 2e47ce0
- [x] Fix newest tab — V2_URL has no trailing slash so the `/v2/` contains() guard never sent x-tidal-client-version; home feed 400'd. Fixed by matching `/v2`
- [x] Fix auth refresh-on-start — access_token refreshed every fresh process even with a valid stored token; now reuses the unexpired token
- [x] Add getAlbumList2 — favorites (starred/frequent/recent/byGenre), random (shuffled favorites), newest (v2 home feed), alphabeticalByName/alphabeticalByArtist, byYear
- [x] Add getPlaylists — Tidal user playlists, newest first, coverArt = full image URL
- [x] Add getGenres — empty genre list (Tidal exposes none)
- [x] Add jukeboxControl — in-memory state machine; status/playlist on get
- [x] Add getCoverArt — 302 redirect to resolved Tidal image URL; accepts full URL, bare playlist UUID, al<id>/ar<id>/bare album id; size snaps to valid Tidal dimensions (album 160/320/640/1280, artist 160/320/480/750)
- [x] Add search3 — maps Tidal /v1/search to searchResult3 (artist/album/song), honors count/offset params
- [x] Add ID parser — src/navidrome/ids.rs, prefixed encode/decode + tests
- [x] Map getUser — returns the Tidal account profile (GET /v1/users/{id}, cached), ignoring the passed username; auth reads settings.toml; roles reflect the bridge; scrobblingEnabled false until middleware lands
- [x] Fix device-auth parsing — Tidal returns camelCase JSON (deviceCode, userId); added serde(rename_all = "camelCase") to DeviceAuth and Session
- [x] Detect web-player client_ids — sub_status 1002 / "not a Limited Input Device client" → clear auth error instead of panic
- [x] Store tokens in macOS Keychain — keyring crate v4.1.6, service Subtidal/account tidal; removed plaintext tidal_tokens.json
- [x] Auto-present login at startup — no login CLI arg; needs_login() checks keyring, runs device-code flow when missing/expired
- [x] Add Tidal client — device-code login, token refresh, cached authenticated GETs, stream URL fetch
- [x] Embed Tidal credentials XOR-obfuscated — scripts/gen_embedded.py generates src/tidal/embedded.rs; real values embedded
- [x] Add getUser endpoint — role flags as strings, nested user, matches documented OpenSubsonic shape
- [x] Implement token auth — t + s (md5(password + salt)), p plaintext, p=enc:<hex>; error codes 10/40/70
- [x] Typed settings — config crate, Settings struct in OnceLock, settings.toml
