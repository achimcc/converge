# Source of these fixtures

Recorded 2026-09-13 around 16:10 CEST from a running Trailarr 0.11.5
container (image
`docker.io/nandyalu/trailarr:0.11.5@sha256:bbf4e94d078cacf7d244ebdc30e288875c38dfb1c301fc384be506870365c013`)
on the host this program was written for:

- `connections.json` — `GET /api/v1/connections/`
- `trailerprofiles.json` — `GET /api/v1/trailerprofiles/`

each with the key in the `X-API-KEY` header, and the OpenAPI description in
`openapi/trailarr-0.11.5.json` from the same container.

**These are masked, not verbatim.** A connection carries the API key of the
Radarr or Sonarr it connects to, in the clear. Before the answers left the
host, every string field whose name matches
`pass|user|addr|mail|webhook|key|token|secret|cookie|auth` was replaced with
`"<masked>"` by `jq` on the host itself — the two `api_key` values — and the
files were written with `jq -S`, so keys are sorted. Nothing else was
changed; the trailer profiles carry no credentials.

At recording time the service already held the state the host wants: both
connections with `monitor_new_media` true and no path mappings, and both
trailer profiles with the host's search query, exclusion words and formats.
Tests use credentials whose value is `<masked>`, so the recorded state is
"already desired".

## Constructed

- `constructed-settings-version-only.json` — the readiness probe's answer,
  reduced to `{"version": "v0.11.5"}`. The real `GET /api/v1/settings/`
  answer carries some thirty-six more fields, among them Trailarr's own API
  key and the web UI's user name; it was not recorded. The version string is
  the one the running container reported and the OpenAPI description's
  `info.version` states.
