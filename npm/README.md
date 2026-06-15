# pgpaw

Read-only Postgres cache + realtime server over an embedded
[pglite](https://crates.io/crates/pglite-rs) logical replica.

This npm package ships a thin launcher; on install it downloads the prebuilt
`pgpaw` binary for your platform from the matching
[GitHub Release](https://github.com/Midwess/PgPaw/releases).

```bash
npm install -g pgpaw
pgpaw --help
```

Supported platforms: Linux x86_64, macOS arm64 (Apple Silicon). On other
platforms install from source with `cargo install pgpaw`.

Full documentation: https://github.com/Midwess/PgPaw
