# Source of these fixtures

Recorded 2026-09-13 around 18:30 CEST from a running Prowlarr 2.5.2.5491
(nixpkgs package, NixOS 26.05) inside a systemd-nspawn container.

- `system-status.json` — `GET /api/v1/system/status`
- `downloadclient.json` — `GET /api/v1/downloadclient`
- `downloadclient-schema.json` — `GET /api/v1/downloadclient/schema`, only
  the QBittorrent and Sabnzbd templates
- `applications.json` — `GET /api/v1/applications`
- `applications-schema.json` — `GET /api/v1/applications/schema`, only the
  Radarr, Sonarr and Lidarr templates

Recorded with the service's key loaded as a credential inside the container
(`curl -H "X-Api-Key: …"`), filtered and **masked on the host** with `jq`
before anything left it: every `fields` entry whose `privacy` is not
`normal` and whose value is not `********`, `""` or `null` became
`"<masked>"` (the `userName` values), and any other string of 24 or more
token characters would have become `"<masked-token>"` -- field names
excluded, since Prowlarr has field names that long; no value was hit. Keys
arrive as `********` already: three in `applications.json`, two in
`downloadclient.json`. The addresses are the host's zone addresses, which
its public configuration names anyway.

At recording time the host's desired state (`lib/arr-einstellungen.nix`,
`.prowlarr`) matched: `plan` reported `unchanged` for both specs.
