# Codex app-server protocol schema

`codex_app_server_protocol.schemas.json` is the bundle emitted by
`codex app-server generate-json-schema --out <dir>` for the CLI version in
`VERSION`. `src/protocol.rs` hand-types only the subset del uses, derived from
this file. Regenerate and diff after upgrading the CLI.
