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

Recorded the same evening around 20:10 CEST, same instance (v0.9.0):

- `indexer.json` — `GET /api/v1/indexer` (all five indexers)
- `indexer-schema.json` — `GET /api/v1/indexer/schema`, only the templates
  named Karagarga, TorrentLeech, Torrent Network, Generic Newznab,
  MyAnonamouse and FunFile (both FunFile entries; 627 templates in all)
- `indexerproxy.json`, `indexerproxy-schema.json` — `GET /api/v1/indexerproxy`
  and its `/schema`
- `tag.json` — `GET /api/v1/tag`

Prowlarr answers a Cardigann indexer's `username` and `password` and
MyAnonamouse's `mamId` in the clear (`privacy normal`). These were masked on
the host as well: every `fields` entry whose name matches
`(?i)pass|user|mamid|key|token|secret|cookie|auth|captcha|2fa` and whose value
is a string other than `********` or `""` became `"<masked>"` -- two user
names, two passwords, one `mamId`, and the text of the `info_alt2fatoken`
hint (once in each file). A second pass over the files found no unmasked
value under such a name.

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

Recorded 2026-09-22 from the same instance, upgraded to **Prowlarr 2.6.5**:

- `appprofile.json` — `GET /api/v1/appprofile`, the one profile the service
  has (`Standard`, id 1). **Verbatim**, keys sorted (`jq -S`). An app profile
  carries no credential of any kind — four booleans, a number, a name and an
  id — so nothing was masked, and the file was searched for long tokens all
  the same.
