# Vendored OpenAPI descriptions

Byte-for-byte copies, fetched 2026-09-13 from the release tags of the
versions the recorded fixtures come from:

| file | source | sha256 |
|---|---|---|
| `radarr-6.3.0.10514.json` | https://raw.githubusercontent.com/Radarr/Radarr/v6.3.0.10514/src/Radarr.Api.V3/openapi.json | `95ea9062485118d6a8abed8250b9bfbf94e4de0f55e9c5611da6805864f9a26e` |
| `sonarr-4.0.19.2979.json` | https://raw.githubusercontent.com/Sonarr/Sonarr/v4.0.19.2979/src/Sonarr.Api.V3/openapi.json | `3fd4c4f4385b1043c3568bd3b37fa6c3c0161135072962dffb611f4ff270e2b7` |
| `jellyfin-10.11.11.json` | https://repo.jellyfin.org/files/openapi/stable/jellyfin-openapi-10.11.11.json (release artifact; the running server answers 500 at `/api-docs/openapi.json`) | `e29fc369ecae54676caeb50cbbc006b5c4ee959906c00f1e02f6f83db9c227fb` |

`cargo test` runs the schema check against both. A consumer should run
`converge schema-check` against the file for the version it actually deploys.

These files describe names and types, not behaviour: both list only `200` for
`PUT /api/v3/qualitydefinition/update`, and Radarr answers `202`.
