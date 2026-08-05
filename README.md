```
   src/
   ├── main.rs           # reads env config, builds TidalProvider, serves
   ├── config.rs         # env vars → Config struct
   ├── navidrome/        # the Subsonic protocol layer (existing, grows)
   │   ├── mod.rs
   │   ├── routes.rs     # all /rest/* routes
   │   ├── handlers.rs   # thin: parse params → provider call → envelope
   │   ├── models.rs     # envelope + all Subsonic DTOs
   │   ├── params.rs     # query param structs (u, v, c, f, per-endpoint)
   │   ├── auth.rs       # validate u/p or u/t+s
   │   ├── error.rs      # ProviderError → Subsonic error codes
   │   └── provider.rs   # the seam
   └── tidal/
       ├── mod.rs
       ├── client.rs     # reqwest client, token + refresh, base URL
       ├── models.rs     # Tidal JSON structs (Deserialize only)
       └── mapper.rs     # Tidal JSON → Subsonic DTOs
```
