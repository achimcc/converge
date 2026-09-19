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

`kavita-0.9.1.4.json` comes from the source tree of the release Kavita ships
as `v0.9.1.4` (https://github.com/kareadita/kavita/archive/v0.9.1.4.tar.gz,
the nixpkgs source of the package the host deploys; fetched 2026-09-18),
copied byte for byte. It declares `info.version` 0.9.1.1: Kavita did not
regenerate it for the two patch releases after that, and the running 0.9.1.4
reports itself as 0.9.1.3. The file is named after the package, because that
is the version a consumer passes in.

| file | source | sha256 |
|---|---|---|
| `kavita-0.9.1.4.json` | `openapi.json` at the top of that tree | `e7ced995b077a067d4b9fc741e8fe24fee9f11346b123f2f53887379cb9e45df` |

`dispatcharr-0.31.0.json` is not from a release tag either: Dispatcharr
generates its description at runtime (drf-spectacular). It was fetched
2026-09-19 around 05:50 CEST with `GET /api/schema/?format=json` from a
throwaway container of
`ghcr.io/dispatcharr/dispatcharr:0.31.0@sha256:f81924fa3dbfeb463b3908be7e086bf58aacbd2ba56062bfb555ee3a471acf8f`,
written as answered. It is wrong about one body (design §29).

| file | source | sha256 |
|---|---|---|
| `dispatcharr-0.31.0.json` | `GET /api/schema/?format=json` of that container | `70ab1dd8950f9ec889fdabe33292a58c0a967de971e885bae3d833bcecbbea99` |

ntfy publishes no OpenAPI description at all; its fields are checked by
recorded answers and at runtime only (`docs/design.md` §10).

`cargo test` runs the schema check against every file here. A consumer should
run `converge schema-check` against the file for the version it actually
deploys.

These files describe names and types, not behaviour: Radarr's and Sonarr's
list only `200` for `PUT /api/v3/qualitydefinition/update`, and Radarr
answers `202`.
