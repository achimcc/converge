# Source of these fixtures

Recorded 2026-09-13 around 23:00 CEST from a running bindery 1.33.2 (built
from source, NixOS 26.05) inside a systemd-nspawn container.

- `health.json` — `GET /api/v1/health`
- `downloadclient.json` — `GET /api/v1/downloadclient` (SABnzbd, qBittorrent)
- `prowlarr.json` — `GET /api/v1/prowlarr`
- `rootfolder.json` — `GET /api/v1/rootfolder`
- `setting-import.mode.json` — `GET /api/v1/setting/import.mode`

Recorded with the service's key as a credential inside the container and
masked **on the host** with `jq` before anything left it: every non-empty
string under a key matching `(?i)pass|user|key|token|secret|cookie|auth|session`
became `"<masked>"`, and any string of 24 or more token characters would have
become `"<masked-token>"`. That hit qBittorrent's `username` -- and the `key`
field of the setting, whose value is just `import.mode`; it was set back by
hand. bindery answers `apiKey` and `password` as `""` already. The addresses
are the host's zone addresses, which its public configuration names anyway.

At recording time the host's desired state matched: `plan` reported
`unchanged` for all four bindery specs.
