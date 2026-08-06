# Tidal API reference

The Tidal API is split across four hosts. Most content endpoints require a bearer token
(`Authorization: Bearer <access_token>`).

| Base URL | API style | Auth |
|---|---|---|
| `https://auth.tidal.com/v1/oauth2` | OAuth 2.0 form-encoded | none (client_id) |
| `https://api.tidal.com/v1` | legacy JSON | Bearer token |
| `https://api.tidal.com/v2` | newer JSON | Bearer token + `x-tidal-client-version` |
| `https://openapi.tidal.com/v2` | JSON:API (`data`/`included`) | Bearer token |

Common query params on most calls: `countryCode` (from `/sessions`, e.g. `US`),
`locale=en_US`, `deviceType=BROWSER`. v2 calls also accept `platform=WEB` where noted.
A 401 response triggers a token refresh (`refresh_token` grant) and one retry.

---

## 1. Auth

### POST `https://auth.tidal.com/v1/oauth2/device_authorization`
Start the device-code flow (limited-input clients only).

- Form body: `client_id`, `scope=r_usr w_usr w_sub`, optionally `client_secret`
- Error 400 with `not a Limited Input Device client` / `sub_status=1002` means the client
  does not support device flow (typically a web-player client id).

Response:

```json
{
  "deviceCode": "D-1b3f...",
  "userCode": "BQ7-KNP",
  "verificationUri": "https://tidal.com/device",
  "verificationUriComplete": "https://tidal.com/device?user_code=BQ7-KNP",
  "expiresIn": 600,
  "interval": 5
}
```

### POST `https://auth.tidal.com/v1/oauth2/token`
Single token endpoint used with three grant types:

| Grant type | Extra form fields |
|---|---|
| `urn:ietf:params:oauth:grant-type:device_code` | `device_code`, `client_id`, `scope` |
| `authorization_code` | `code`, `client_id`, `redirect_uri`, `scope=r_usr+w_usr+w_sub`, `code_verifier`, `client_unique_key` |
| `refresh_token` | `client_id`, `refresh_token`, `scope=r_usr w_usr w_sub` |

Response (`refreshToken` and `userId` are optional in some grant responses):

```json
{
  "accessToken": "eyJhbGciOiJSUzI1NiIs...",
  "refreshToken": "12345678-aaaa-bbbb-cccc-xxxxxxxxxxxx",
  "expiresIn": 86400,
  "tokenType": "Bearer",
  "userId": 123456789
}
```

Device-code polling returns HTTP 400 with `authorization_pending` (or `slow_down`) while the
user has not yet authorized. The caller must retry after `interval` seconds.

### GET `https://login.tidal.com/authorize`
PKCE browser flow (opened in a browser, not an API call):

```
https://login.tidal.com/authorize?response_type=code&redirect_uri=https%3A%2F%2Ftidal.com%2Fandroid%2Flogin%2Fauth&client_id={id}&lang=EN&appMode=android&client_unique_key={key}&code_challenge={c}&code_challenge_method=S256&restrict_signup=true
```

The redirect lands on `https://tidal.com/android/login/auth?code=...`. The code is then
exchanged via the `authorization_code` grant above.

---

## 2. Legacy API — `https://api.tidal.com/v1`

### GET `/v1/sessions`
Session info for the current token. Returns the `userId` and the `countryCode` used as the
default for all subsequent calls.

```json
{
  "userId": 123456789,
  "countryCode": "US"
}
```

### GET `/v1/users/{userId}`
User profile. Useful fields: `firstName`, `lastName`, `username`, `profileName`, `artistId`
(the artist profile id associated with the account, if any).

```json
{
  "id": 123456789,
  "username": "fan_987",
  "firstName": "Ada",
  "lastName": "Lovelace",
  "profileName": "ada",
  "artistId": 987654,
  "countryCode": "US"
}
```

### GET `/v1/users/{userId}/playlists`
Paginated list of the user's playlists.

- Query: `countryCode`, `limit`, `offset` (also `order`, `orderDirection`, `locale`, `deviceType`)

Response (paginated envelope):

```json
{
  "items": [
    {
      "uuid": "0f3154e6-6c0a-4b93-9f5b-4a3c2f1e0d9c",
      "title": "Morning Drive",
      "description": "Upbeat start to the day",
      "image": "3f1c.../1280x1280.jpg",
      "squareImage": "3f1c.../550x550.jpg",
      "numberOfTracks": 42,
      "numberOfVideos": 0,
      "creator": { "id": 123456789, "name": "Ada" },
      "type": "USER",
      "duration": 9134,
      "popularity": 100,
      "publicPlaylist": true,
      "url": "https://tidal.com/browse/playlist/0f3154e6-...",
      "created": "2023-01-15T10:00:00.000Z",
      "lastUpdated": "2024-02-01T08:30:00.000Z"
    }
  ],
  "totalNumberOfItems": 1,
  "offset": 0,
  "limit": 100
}
```

### GET `/v1/albums/{albumId}`
Album detail. Query: `countryCode`.

```json
{
  "id": 172819371,
  "title": "In Rainbows",
  "cover": "3f1c.../1280x1280.jpg",
  "vibrantColor": "#8a3324",
  "videoCover": null,
  "artist": { "id": 413, "name": "Radiohead" },
  "numberOfTracks": 10,
  "numberOfVideos": 0,
  "numberOfVolumes": 1,
  "duration": 2553,
  "releaseDate": "2007-10-10",
  "upc": "00602517409524",
  "type": "ALBUM",
  "copyright": "© 2007 XL Recordings",
  "explicit": false,
  "popularity": 98,
  "url": "https://tidal.com/browse/album/172819371",
  "audioQuality": "LOSSLESS",
  "streamReady": true,
  "allowStreaming": true,
  "streamStartDate": "2007-10-10",
  "audioModes": ["STEREO"],
  "mediaMetadata": { "tags": ["LOSSLESS"] }
}
```

Note: some responses return `artists` (an array) instead of `artist` (a single object).

### GET `/v1/albums/{albumId}/tracks`
- Query: `countryCode`, `limit`, `offset`

Response: same paginated envelope with `items` as track objects (see track shape below).

### GET `/v1/artists/{artistId}`
Artist detail. Query: `countryCode`.

```json
{
  "id": 413,
  "name": "Radiohead",
  "picture": "6b9a.../1280x1280.jpg",
  "handle": "radiohead",
  "userId": 4104,
  "popularity": 92,
  "url": "https://tidal.com/browse/artist/413",
  "spotlighted": false,
  "artistTypes": ["SOLO"],
  "artistRoles": [{ "category": "MAIN", "categoryId": 0 }]
}
```

### GET `/v1/artists/{artistId}/toptracks`
- Query: `countryCode`, `limit`, `offset`

Response: `{ "items": [ track, ... ], ... }`.

### GET `/v1/artists/{artistId}/albums`
- Query: `countryCode`, `limit`, `offset`

Response: `{ "items": [ album, ... ], ... }`.

### GET `/v1/artists/{artistId}/bio`
Response: `{ "text": "Radiohead are an English rock band..." }`. The endpoint may 404 when
no bio exists.

### GET `/v1/tracks/{trackId}`
Track detail. Query: `countryCode`.

Track shape (also used inside the `items` arrays of search, favorites, and playlists):

```json
{
  "id": 55371052,
  "title": "Weird Fishes / Arpeggi",
  "duration": 319,
  "version": null,
  "artist": { "id": 413, "name": "Radiohead" },
  "artists": [{ "id": 413, "name": "Radiohead", "type": "MAIN" }],
  "album": {
    "id": 172819371,
    "title": "In Rainbows",
    "cover": "3f1c.../1280x1280.jpg",
    "vibrantColor": "#8a3324"
  },
  "audioQuality": "LOSSLESS",
  "trackNumber": 4,
  "volumeNumber": 1,
  "isrc": "GBAYE0700489",
  "explicit": false,
  "popularity": 87,
  "replayGain": -7.7,
  "peak": 0.999969,
  "url": "https://tidal.com/browse/track/55371052",
  "streamReady": true,
  "allowStreaming": true,
  "premiumStreamingOnly": false,
  "streamStartDate": "2007-10-10",
  "audioModes": ["STEREO"],
  "mediaMetadata": { "tags": ["LOSSLESS"] },
  "mixes": { "TRACK_MIX": "0020088a01dc473eb48aafe3a39ed43a" }
}
```

### GET `/v1/tracks/{trackId}/lyrics`
Response:

```json
{
  "trackId": 55371052,
  "lyricsProvider": "MUSIXMATCH",
  "providerCommontrackId": "1473864",
  "providerLyricsId": "22837039",
  "lyrics": "I get eaten by the worms...\n\n",
  "subtitles": null,
  "isRightToLeft": false
}
```

### GET `/v1/tracks/{trackId}/credits`
Response: array of credit groups:

```json
[
  {
    "type": "PRODUCER",
    "contributors": [{ "name": "Nigel Godrich", "id": 1023432 }]
  },
  {
    "type": "WRITER",
    "contributors": [{ "name": "Colin Greenwood", "id": 93456 }]
  }
]
```

### GET `/v1/tracks/{trackId}/playbackinfopostpaywall`
Stream info for a track.

- Query: `countryCode`, `audioquality` (e.g. `LOSSLESS`, `HI_RES`), `playbackmode=STREAM`,
  `assetpresentation=FULL`

Response (`manifest` is base64-encoded):

```json
{
  "manifestMimeType": "application/vnd.tidal.bts",
  "manifest": "eyJ1cmxzIjogWyJodHRwczovL3...",
  "audioQuality": "LOSSLESS",
  "bitDepth": 16,
  "sampleRate": 44100,
  "albumReplayGain": -7.7,
  "albumPeakAmplitude": 0.999969,
  "trackReplayGain": -7.7,
  "trackPeakAmplitude": 0.999969
}
```

Decoded manifest formats:

- `application/vnd.tidal.bts` → JSON `{ "urls": [...], "codecs": "flac", "mimeType": ..., "encryptionType": ... }`; use the first URL.
- `application/dash+xml` → raw MPD XML (DASH). The codec is available in the XML as `codecs="..."` (`FLAC` is special).
- other JSON → `{ "urls": [...] }`; use the first URL.

### GET `/v1/videos/{videoId}`
Video metadata. Query: `countryCode`, `locale`, `deviceType`.

```json
{
  "id": 138838078,
  "title": "Weird Fishes / Arpeggi (Live)",
  "duration": 319,
  "imageId": "9c8b...",
  "vibrantColor": "#332211",
  "quality": "HIGH",
  "type": "VIDEO",
  "explicit": false,
  "adsPrePaywallOnly": false,
  "artist": { "id": 413, "name": "Radiohead" }
}
```

### GET `/v1/videos/{videoId}/playbackinfopostpaywall`
- Query: `countryCode`, `videoquality` (e.g. `HIGH`), `playbackmode=STREAM`,
  `assetpresentation=FULL`

Response:

```json
{
  "videoId": 138838078,
  "videoQuality": "HIGH",
  "manifestMimeType": "application/vnd.tidal.emu",
  "manifest": "eyJ1cmxzIjogW10sICJjb2RlY3MiOiAiYXZjMS4wIn0="
}
```

The decoded manifest is JSON `{ "urls": [...] }`; use the first URL.

### GET `/v1/playlists/{playlistId}`
Playlist details. The response includes an `etag` header; all playlist mutations require
that value as the `If-None-Match` header.

Response: playlist JSON document (see the playlist object under `/users/{userId}/playlists`).

### GET `/v1/playlists/{playlistId}/items`
Playlist entries — tracks **and** videos, each wrapped in `{ item, type }`.

- Query: `countryCode`, `limit`, `offset` (also `order`, `orderDirection`)

```json
{
  "items": [
    {
      "type": "track",
      "item": { "id": 55371052, "title": "Weird Fishes / Arpeggi", "duration": 319, ... }
    },
    {
      "type": "video",
      "item": { "id": 138838078, "title": "...", "imageId": "9c8b...", "duration": 0, ... }
    }
  ],
  "totalNumberOfItems": 42,
  "offset": 0,
  "limit": 100
}
```

Note: video entries can carry a null or absent `duration`.

### POST `/v1/playlists/{playlistId}/items`
Add tracks to a playlist.

- Headers: `If-None-Match: <etag from GET /playlists/{id}>`
- Form body: `trackIds` (comma-separated), `onDupes` (`FAIL` or `SKIP`), `onArtifactNotFound=FAIL`

Response: 2xx, empty body.

### DELETE `/v1/playlists/{playlistId}/items/{index}`
Remove the item at the given index.

- Headers: `If-None-Match: <etag>`
- Query: `countryCode`

Response: 2xx, empty body.

### DELETE `/v1/playlists/{playlistId}`
Delete the playlist.

- Headers: `If-None-Match: <etag>`
- Query: `countryCode`

Response: 2xx, empty body.

### GET `/v1/playlists/{playlistId}/recommendations/items`
- Query: `countryCode`, `limit`, `offset`, `locale`, `deviceType`

Response: paginated envelope with `items: [{ "item": track }]`.

### GET `/v1/search`
v1 search fallback.

- Query: `query`, `countryCode`, `limit`, `offset=0`,
  `types=ARTISTS,ALBUMS,TRACKS,PLAYLISTS,VIDEOS`,
  `includeContributors=true`, `includeUserPlaylists=true`, `supportsUserData=true`

Response: search sections (see the v2 search response below, minus `topHits`).

### GET `/v1/mixes/{mixId}/items`
Legacy mix track list.

- Query: `countryCode`

Response: `{ "items": [{ "item": track }, ...] }`.

### GET `/v1/pages/{slug}` — the Pages API
Home, tab, explore, album, mix, and artist pages. The response is a generic page document
that can appear in one of several layouts:

1. V1: `{ "rows": [{ "modules": [{ "type", "title", "pagedList" | "highlights" | "listItems" | "mix", "showMore" }] }] }`
2. V2: `{ "items": [{ "type": "HORIZONTAL_LIST", "title", "items": [{ "type", "data" }], "viewAll" }] }`
3. Tabs: `{ "tabs": [{ "title", "rows" | "items" }] }`
4. Categories: `{ "categories" | "sections": [...] }`
5. Flat view-all: `{ "title", "items": [...] }`

Module and section types: `MIX_HEADER`, `TRACK_LIST`, `ALBUM_HEADER`, `ALBUM_ITEMS`,
`ALBUM_LIST`, `ARTIST_LIST`, `HIGHLIGHT_MODULE`, `SHORTCUT_LIST`, `HORIZONTAL_LIST`,
`HORIZONTAL_LIST_WITH_CONTEXT`, `PAGE_LINKS`, `PAGE_LINKS_CLOUD`, `MULTIPLE_TOP_PROMOTIONS`,
`TEXT_BLOCK`, `SOCIAL`, `ARTICLE_LIST`, `FEATURED_PROMOTIONS` (the last four carry no
playable content).

Known page slugs:

- `pages/home`, `pages/for_you`, `pages/my_collection_my_mixes`, `pages/explore`, `pages/rising`
- `pages/mix?mixId={id}` — mix/radio track list plus `MIX_HEADER` metadata
- `pages/album?albumId={id}` — album page (`ALBUM_HEADER` → album + credits + review,
  `ALBUM_ITEMS` → paged track list with `totalNumberOfItems`)
- `pages/artist?artistId={id}` — artist page
- `pages/my_collection_recently_played` — recently played (verified live)
- `pages/show/essential_album`

`pages/suggested_new_tracks_for_you` and `pages/suggested_new_albums_for_you` 404 with
`subStatus 2001` as of 2026-08-06; sone lists them in a debug dump but they no longer
resolve. The same content is still served inside the home feed: the v2 feed includes
"Suggested new albums for you" and "Recommended new tracks" sections.

The `viewAll` and `showMore.apiPath` values embedded in any page response are themselves
fetchable endpoints.

### GET `/v1/ping`
Connectivity check. Any of 2xx / 404 / 401 counts as reachable.

---

## 3. Favorites (v1, `https://api.tidal.com/v1`)

The same pattern applies to every favorite type: `tracks`, `videos`, `albums`, `playlists`,
`artists`.

| Method | Path | Body / params | Response |
|---|---|---|---|
| GET | `/v1/users/{userId}/favorites/{type}` | `countryCode`, `limit`, `offset`, `order=DATE`, `orderDirection=DESC` | `{ items: [{ item: {...}, created: "ISO date" }], totalNumberOfItems, offset, limit }` |
| POST | `/v1/users/{userId}/favorites/{type}` | form: `trackId` / `videoIds` (+`onArtifactNotFound=FAIL`) / `albumId` / `uuid` / `artistId` | 2xx, empty body |
| DELETE | `/v1/users/{userId}/favorites/{type}/{id}` | `countryCode` | 2xx, empty body |

GET responses wrap each entity as `{ "item": {...}, "created": "2024-01-01T00:00:00.000Z" }`.
The tracks and albums variants can also carry a top-level `id` on each entry. The
`favorites/videos` endpoint rejects very large page sizes (a single `limit=10000` request
is refused); page through it in small batches.

Example (`favorites/tracks`):

```json
{
  "items": [
    {
      "id": 55371052,
      "created": "2024-01-01T00:00:00.000Z",
      "item": { "id": 55371052, "title": "Weird Fishes / Arpeggi", ... }
    }
  ],
  "totalNumberOfItems": 1,
  "offset": 0,
  "limit": 2000
}
```

### GET `/v1/users/{userId}/favorites/ids`
All favorite ids in one call, grouped by entity type.

- Query: `countryCode`, `locale`, `deviceType`

Response:

```json
{
  "TRACK": ["55371052", "55371053"],
  "ALBUM": ["172819371"],
  "ARTIST": ["413"],
  "PLAYLIST": ["0f3154e6-6c0a-4b93-9f5b-4a3c2f1e0d9c"]
}
```

---

## 4. v2 API — `https://api.tidal.com/v2`

All v2 requests should send the `x-tidal-client-version` header (e.g. `2025.11.3`).

### GET `/v2/search`
Primary search (used by the web app). Fall back to v1 search on failure.

- Query: `query`, `countryCode`, `limit`, `types=ARTISTS,ALBUMS,TRACKS,PLAYLISTS,VIDEOS`,
  `includeContributors=true`, `includeUserPlaylists=true`, `includeDidYouMean=true`,
  `supportsUserData=true`, `locale`, `deviceType`

Response:

```json
{
  "artists": { "items": [ { "id": 413, "name": "Radiohead", "picture": "...", "type": "MAIN" } ] },
  "albums": { "items": [ { "id": 172819371, "title": "In Rainbows", "artists": [...], "cover": "..." } ] },
  "tracks": { "items": [ { "id": 55371052, "title": "Weird Fishes / Arpeggi", "artists": [...], "album": {...}, "duration": 319 } ] },
  "playlists": { "items": [ { "uuid": "...", "title": "...", "squareImage": "..." } ] },
  "videos": { "items": [ { "id": 138838078, "title": "...", "imageId": "..." } ] },
  "topHits": [
    { "type": "ARTISTS", "value": { "id": 413, "name": "Radiohead", "picture": "..." } },
    { "type": "TRACKS", "value": { "id": 55371052, "title": "...", "artists": [{ "name": "Radiohead" }], "album": { "id": ..., "title": "...", "cover": "..." } } }
  ]
}
```

`topHits` entries are typed (`ARTISTS`, `ALBUMS`, `TRACKS`, `VIDEOS`, `PLAYLISTS`) with a
`value` payload.

### GET `/v2/suggestions/`
Search suggestions and direct hits for the mini-search dropdown.

- Query: `query`, `countryCode`, `explicit=true`, `hybrid=true`

Response:

```json
{
  "history": [ { "query": "radiohead" } ],
  "suggestions": [ { "query": "radiohead in rainbows" } ],
  "directHits": [
    { "type": "ARTISTS", "value": { "id": 413, "name": "Radiohead", "picture": "..." } }
  ]
}
```

### GET `/v2/home/feed/{feedSlug}`
Personalized home feed (what the web app uses). Feed slugs are the v2 feed tab types
lowercased (`static`, `editorial`, `uploads`, ...), as reported in
`header.vibes.items[].type`.

- Query: `countryCode`, `locale`, `deviceType`, `platform=WEB`, optional `cursor`

Response:

```json
{
  "header": {
    "vibes": { "items": [ { "name": "Home", "type": "STATIC" }, { "name": "Discover", "type": "EDITORIAL" } ] }
  },
  "items": [
    {
      "type": "HORIZONTAL_LIST",
      "moduleId": "m1",
      "title": { "text": "New Releases" },
      "items": [ { "type": "ALBUM", "data": { "id": 172819371, "title": "In Rainbows", "artists": [...], "cover": "..." } } ],
      "viewAll": "artist/ARTIST_ALBUMS/view-all?artistId=413"
    }
  ],
  "page": { "cursor": "eyJ...", "totalElements": 100 }
}
```

Section content types: `SHORTCUT_LIST`, `HORIZONTAL_LIST`, `HORIZONTAL_LIST_WITH_CONTEXT`,
`TRACK_LIST`. Horizontal lists carry items of a single entity kind (`MIX`, `ALBUM`,
`PLAYLIST`, `ARTIST`, `TRACK`).

### GET `/v2/favorites/mixes`
Favorite mixes.

- Query: `countryCode`, `locale`, `deviceType`, `limit`, `offset`, `order`, `orderDirection`

Response: a bare array or a wrapper `{ "items": [...] }`. Each item has a different shape
from the home-feed MIX entity:

```json
{
  "id": "0020088a01dc473eb48aafe3a39ed43a",
  "title": "Radiohead Mix",
  "subTitle": "Because you listened to Radiohead",
  "mixType": "ARTIST_MIX",
  "images": {
    "SMALL": { "url": "https://resources.tidal.com/images/.../320x320.jpg" },
    "MEDIUM": { "url": "..." },
    "LARGE": { "url": "..." }
  }
}
```

The response has no `totalNumberOfItems` field; a full page (items == limit) implies more
pages.

### PUT `/v2/favorites/mixes/add`
Add a favorite mix. Query: `countryCode`, `mixIds`, `onArtifactNotFound=FAIL`.
Response: 2xx, empty body.

### PUT `/v2/favorites/mixes/remove`
Remove a favorite mix. Query: `countryCode`, `mixIds`.
Response: 2xx, empty body.

### GET `/v2/my-collection/playlists/folders`
Playlist folder tree.

- Query: `folderId`, `offset`, `limit`, `order`, `orderDirection`, `countryCode`, `locale`,
  `deviceType`, optional `includeOnly`, optional `cursor`

Response: raw JSON containing `items` and optionally a `cursor`.

### GET `/v2/my-collection/playlists/folders/flattened`
All playlists across every folder, flattened.

- Query: `offset=0`, `limit=50`, `order=DATE`, `orderDirection=DESC`, `countryCode`, `locale`,
  `deviceType`, optional `cursor`

Response:

```json
{
  "items": [ ... folder items ... ],
  "cursor": "next-page-cursor"
}
```

Page through with `cursor` until it is absent.

### PUT `/v2/my-collection/playlists/folders/create-folder`
- Query: `folderId`, `name`, `countryCode`, `locale`, `deviceType`, optional `trns`

Response: 2xx JSON body (may be null).

### PUT `/v2/my-collection/playlists/folders/rename`
- Query: `trn` (folder resource name), `name`, `countryCode`, `locale`, `deviceType`

Response: 2xx, empty body.

### PUT `/v2/my-collection/playlists/folders/remove`
- Query: `trns`, `countryCode`, `locale`, `deviceType`

Response: 2xx, empty body.

### PUT `/v2/my-collection/playlists/folders/move`
- Query: `folderId`, `trns` (playlist resource name), `countryCode`, `locale`, `deviceType`

Response: 2xx, empty body.

### GET `/v2/artist/{artistId}`
Artist page (preferred over the v1 `pages/artist` fallback).

- Query: `countryCode`, `locale`, `deviceType`, `platform=WEB`

Response: raw page JSON (rows/modules or flat items layout).

### GET `/v2/artist/ARTIST_TOP_TRACKS/view-all`
- Query: `artistId`, `locale`, `countryCode`, `deviceType`, `platform=WEB`, `limit`, `offset`

Response: raw page JSON.

### GET `/v2/{viewAllPath}`
Generic "view all" endpoint. `viewAll` and `showMore.apiPath` values from any v2 response
resolve against the v2 base URL, for example:

- `artist/ARTIST_ALBUMS/view-all?artistId=413`
- `artist/ARTIST_SINGLES/view-all?artistId=413`
- `artist/ARTIST_VIDEOS/view-all?artistId=413`

Absolute `http` paths pass through unchanged.

- Query: `artistId` (when applicable), `locale`, `countryCode`, `deviceType`, `platform=WEB`,
  `limit`, `offset`

Response: raw page JSON.

### GET `/v2/nexplore`
Explore page (returns the same page layouts as the Pages API).

### GET `/v2/profiles/{userId}`
Social profile. Useful field: `numberOfFollowers`.

```json
{
  "numberOfFollowers": 1234567,
  ...
}
```

---

## 5. Open API — `https://openapi.tidal.com/v2` (JSON:API)

JSON:API style: envelope `{ "data": {...}, "included": [...] }`, content type
`application/vnd.api+json`, plus the `x-tidal-client-version` header.

### POST `/v2/playlists`
Create a playlist.

- Query: `countryCode`
- Body:

```json
{
  "data": {
    "type": "playlists",
    "attributes": { "name": "Morning Drive", "description": "Upbeat", "accessType": "PUBLIC" }
  }
}
```

Response:

```json
{
  "data": {
    "id": "0f3154e6-6c0a-4b93-9f5b-4a3c2f1e0d9c",
    "type": "playlists",
    "attributes": {
      "name": "Morning Drive",
      "description": "Upbeat",
      "accessType": "PUBLIC",
      "playlistType": "USER",
      "createdAt": "2024-02-01T08:30:00.000Z",
      "lastModifiedAt": "2024-02-01T08:30:00.000Z"
    }
  }
}
```

### PATCH `/v2/playlists/{playlistId}`
Update name, description, or access type.

- Headers: `Content-Type: application/vnd.api+json`
- Query: `countryCode`
- Body:

```json
{
  "data": {
    "id": "0f3154e6-...",
    "type": "playlists",
    "attributes": { "name": "New Title", "description": "...", "accessType": "PUBLIC" }
  }
}
```

Response: 204 No Content (empty) or the playlist JSON:API document.

### GET `/v2/playlists`
Public playlists for a user (profile page).

- Query: `filter[owners.id]={userId}`, `include=coverArt`, `countryCode`

Response: JSON:API document. `data[]` are playlists; `included[]` hold `coverArt` artworks.
Relevant playlist attributes: `accessType`, `name`, `numberOfItems`. The cover artwork id
lives under `relationships.coverArt`.

### GET `/v2/artists/{artistId}`
Full artist profile (read-only).

- Query: `include=profileArt,biography,owners`, `countryCode`

Response: JSON:API document with `included`:

```json
{
  "data": {
    "id": "413",
    "type": "artists",
    "attributes": {
      "name": "Radiohead",
      "handle": "radiohead",
      "externalLinks": [ { "href": "https://www.radiohead.com", "meta": { "type": "WEBSITE" } } ]
    },
    "relationships": {
      "profileArt": { "data": [{ "type": "artworks", "id": "art-1" }] },
      "biography": { "data": { "type": "artistBiographies", "id": "bio-1" } },
      "owners": { "data": [{ "type": "users", "id": "999" }] }
    }
  },
  "included": [
    {
      "type": "artworks",
      "id": "art-1",
      "attributes": {
        "files": [
          { "href": "https://resources.tidal.com/...", "meta": { "width": 1280, "height": 1280 } }
        ],
        "blurHash": "LKO2?U%2Tw=w]~RBVZRi};RPxuwH",
        "palette": ["#332211", "#ffffff"]
      }
    },
    { "type": "artistBiographies", "id": "bio-1", "attributes": { "text": "..." } }
  ]
}
```

### PATCH `/v2/artists/{artistId}`
Update artist metadata: display name, handle, and external links.

- Headers: `Content-Type: application/vnd.api+json`, `x-tidal-client-version`
- Query: `countryCode`
- Body: JSON:API document, `data.type: "artists"`, with the attributes to change

Response: 2xx, empty body.

### PATCH `/v2/artistBiographies/{biographyId}`
Update the artist bio text.

- Body:

```json
{
  "data": {
    "type": "artistBiographies",
    "id": "bio-1",
    "attributes": { "text": "..." }
  }
}
```

- Headers and query as above. Response: 2xx, empty body.

### POST `/v2/artworks`
Create an artwork (step 1 of a profile-picture upload).

- Body:

```json
{
  "data": {
    "type": "artworks",
    "attributes": {
      "mediaType": "IMAGE",
      "sourceFile": { "md5Hash": "abc123...", "size": 123456 }
    }
  }
}
```

Response:

```json
{
  "data": {
    "id": "art-1",
    "type": "artworks",
    "attributes": {
      "sourceFile": {
        "uploadLink": { "href": "https://s3.amazonaws.com/..." }
      }
    }
  }
}
```

### PUT `{uploadLink.href}` (S3, from the artworks response)
Upload the JPEG bytes directly to the signed URL.

- Headers: `content-md5: <base64 md5>`, `Content-Type: image/jpeg` (required)
- No Authorization header. Body: raw JPEG bytes.

Response: 2xx, empty body.

### GET `/v2/artworks/{artworkId}`
Poll artwork processing status (after upload, before linking).

- Query: `countryCode`

Response (status path):

```json
{
  "data": {
    "type": "artworks",
    "id": "art-1",
    "attributes": {
      "sourceFile": { "status": { "technicalFileStatus": "COMPLETED" } }
    }
  }
}
```

`technicalFileStatus` is `COMPLETED` when the file is ready. Otherwise the caller should
poll again after a short delay (about 800 ms, up to 20 tries).

### PATCH `/v2/artists/{artistId}/relationships/profileArt`
Set or clear the profile picture.

- Body to set: `{ "data": [ { "type": "artworks", "id": "art-1" } ] }`
- Body to clear: `{ "data": [] }`
- Headers and query as above. Response: 2xx, empty body.

### GET `/v2/artists/{artistId}/relationships/followers`
Fan count (fallback when `/v2/profiles/{id}` lacks `numberOfFollowers`).

Response:

```json
{ "data": [ { "type": "users", "id": "111" }, ... ] }
```

The count is the length of `data[]`.

---

## Full endpoint index

| Method | Endpoint | Purpose |
|---|---|---|
| POST | auth.tidal.com/v1/oauth2/device_authorization | device-code auth |
| POST | auth.tidal.com/v1/oauth2/token | token exchange (3 grants) |
| GET | login.tidal.com/authorize | PKCE browser login |
| GET | api.tidal.com/v1/ping | connectivity check |
| GET | api.tidal.com/v1/sessions | session + country code |
| GET | api.tidal.com/v1/users/{id} | profile, artistId |
| GET | api.tidal.com/v1/users/{id}/playlists | user playlists |
| GET/POST/DELETE | api.tidal.com/v1/users/{id}/favorites/{tracks\|videos\|albums\|playlists\|artists}[/{id}] | favorites CRUD |
| GET | api.tidal.com/v1/users/{id}/favorites/ids | all favorite ids |
| GET | api.tidal.com/v1/albums/{id} | album detail |
| GET | api.tidal.com/v1/albums/{id}/tracks | album tracks |
| GET | api.tidal.com/v1/artists/{id} | artist detail |
| GET | api.tidal.com/v1/artists/{id}/toptracks | artist top tracks |
| GET | api.tidal.com/v1/artists/{id}/albums | artist albums |
| GET | api.tidal.com/v1/artists/{id}/bio | artist bio |
| GET | api.tidal.com/v1/tracks/{id} | track detail |
| GET | api.tidal.com/v1/tracks/{id}/lyrics | lyrics |
| GET | api.tidal.com/v1/tracks/{id}/credits | credits |
| GET | api.tidal.com/v1/tracks/{id}/playbackinfopostpaywall | audio stream |
| GET | api.tidal.com/v1/videos/{id} | video metadata |
| GET | api.tidal.com/v1/videos/{id}/playbackinfopostpaywall | video stream |
| GET | api.tidal.com/v1/playlists/{id} | playlist detail + etag |
| GET | api.tidal.com/v1/playlists/{id}/items | playlist entries |
| POST | api.tidal.com/v1/playlists/{id}/items | add tracks |
| DELETE | api.tidal.com/v1/playlists/{id}/items/{index} | remove track |
| GET | api.tidal.com/v1/playlists/{id}/recommendations/items | recommendations |
| DELETE | api.tidal.com/v1/playlists/{id} | delete playlist |
| GET | api.tidal.com/v1/search | v1 search fallback |
| GET | api.tidal.com/v1/mixes/{id}/items | legacy mix items |
| GET | api.tidal.com/v1/pages/{slug} | pages API (home/album/mix/artist/explore/...) |
| GET | api.tidal.com/v2/search | primary search |
| GET | api.tidal.com/v2/suggestions/ | search suggestions |
| GET | api.tidal.com/v2/home/feed/{slug} | personalized home feed |
| GET | api.tidal.com/v2/favorites/mixes | favorite mixes |
| PUT | api.tidal.com/v2/favorites/mixes/add | favorite mix add |
| PUT | api.tidal.com/v2/favorites/mixes/remove | favorite mix remove |
| GET | api.tidal.com/v2/my-collection/playlists/folders | folder tree |
| GET | api.tidal.com/v2/my-collection/playlists/folders/flattened | all playlists flat |
| PUT | api.tidal.com/v2/my-collection/playlists/folders/create-folder | new folder |
| PUT | api.tidal.com/v2/my-collection/playlists/folders/rename | rename folder |
| PUT | api.tidal.com/v2/my-collection/playlists/folders/remove | delete folder |
| PUT | api.tidal.com/v2/my-collection/playlists/folders/move | move playlist |
| GET | api.tidal.com/v2/artist/{id} | artist page |
| GET | api.tidal.com/v2/artist/*/view-all | artist section view-all |
| GET | api.tidal.com/v2/nexplore | explore page |
| GET | api.tidal.com/v2/profiles/{id} | social profile (fan count) |
| POST | openapi.tidal.com/v2/playlists | create playlist |
| PATCH | openapi.tidal.com/v2/playlists/{id} | update playlist |
| GET | openapi.tidal.com/v2/playlists | public playlists by owner |
| GET | openapi.tidal.com/v2/artists/{id} | full artist profile |
| PATCH | openapi.tidal.com/v2/artists/{id} | artist meta / external links |
| PATCH | openapi.tidal.com/v2/artistBiographies/{id} | update bio |
| POST | openapi.tidal.com/v2/artworks | create artwork |
| PUT | {uploadLink.href} (S3) | upload image bytes |
| GET | openapi.tidal.com/v2/artworks/{id} | artwork status poll |
| PATCH | openapi.tidal.com/v2/artists/{id}/relationships/profileArt | set/clear profile art |
| GET | openapi.tidal.com/v2/artists/{id}/relationships/followers | fan count fallback |
