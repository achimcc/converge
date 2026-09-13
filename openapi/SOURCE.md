# Vendored OpenAPI descriptions

Byte-for-byte copies, fetched 2026-09-13 from the release tags of the
versions the recorded fixtures come from:

| file | source | sha256 |
|---|---|---|
| `radarr-6.3.0.10514.json` | https://raw.githubusercontent.com/Radarr/Radarr/v6.3.0.10514/src/Radarr.Api.V3/openapi.json | `95ea9062485118d6a8abed8250b9bfbf94e4de0f55e9c5611da6805864f9a26e` |
| `sonarr-4.0.19.2979.json` | https://raw.githubusercontent.com/Sonarr/Sonarr/v4.0.19.2979/src/Sonarr.Api.V3/openapi.json | `3fd4c4f4385b1043c3568bd3b37fa6c3c0161135072962dffb611f4ff270e2b7` |
| `lidarr-3.1.0.4875.json` | https://raw.githubusercontent.com/Lidarr/Lidarr/v3.1.0.4875/src/Lidarr.Api.V1/openapi.json (fetched 2026-09-13, evening) | `4ae9e79e9662898ed4704ce80f091161587a9f0d664f9e518b11a038616e491f` |
| `prowlarr-2.5.2.5491.json` | https://raw.githubusercontent.com/Prowlarr/Prowlarr/v2.5.2.5491/src/Prowlarr.Api.V1/openapi.json (fetched 2026-09-13, evening) | `efe3dfb9a928658d8a1f2f307a965fb1275bad2853012a2f9bdc2404215d0fbb` |
| `jellyfin-10.11.11.json` | https://repo.jellyfin.org/files/openapi/stable/jellyfin-openapi-10.11.11.json (release artifact; the running server answers 500 at `/api-docs/openapi.json`) | `e29fc369ecae54676caeb50cbbc006b5c4ee959906c00f1e02f6f83db9c227fb` |

`trailarr-0.11.5.json` is not from a release tag: Trailarr publishes no
description in its repository. It was fetched 2026-09-13 around 16:10 CEST
with `GET /api/v1/openapi.json` from the running container the Trailarr
fixtures were recorded from (image
`docker.io/nandyalu/trailarr:0.11.5@sha256:bbf4e94d078cacf7d244ebdc30e288875c38dfb1c301fc384be506870365c013`)
and written with `jq -S` (keys sorted); nothing was masked — the description
holds names, types and defaults, no configured values.

| file | source | sha256 |
|---|---|---|
| `trailarr-0.11.5.json` | `GET /api/v1/openapi.json` of that container, `jq -S` | `11251eebd9493881e400c7b44e548f2be36cf1c2f09b9e38f5b19878c5a66257` |

ntfy publishes no OpenAPI description at all; its fields are checked by
recorded answers and at runtime only (`docs/design.md` §10).

`cargo test` runs the schema check against every file here. A consumer should
run `converge schema-check` against the file for the version it actually
deploys.

These files describe names and types, not behaviour: Radarr's and Sonarr's
list only `200` for `PUT /api/v3/qualitydefinition/update`, and Radarr
answers `202`.
