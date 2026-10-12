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

- `auth-oidc-providers.json` — `GET /api/v1/auth/oidc/providers`, recorded
  2026-09-18 from the running bindery. The issuer's host is replaced by
  `auth.example.org`; everything else is verbatim. The answer carries **no**
  `client_secret`: it is write-only (`ProviderPublicConfig`), which is why a
  spec hands it over on every apply, as for the other write-only fields (§14).

- `indexers.json` — `GET /api/v1/indexer`, recorded 2026-09-18 from the running
  bindery. The indexers are the ones Prowlarr syncs into it. Names, types,
  categories, flags and ids are verbatim; every `url` is replaced by the
  Prowlarr instance's address plus the row's id, because a tracker's URL can
  carry a passkey. `apiKey` is `""` in the answer already (write-only), and the
  timestamps are rounded. The masking ran **on the host**, so no tracker URL
  ever left the machine.

  Two things this answer settles, both of which the host had guessed wrong:
  there is **no `implementation` field** — the kind is `type` (`torznab` /
  `newznab`) — and five of the six rows carry `seedRatioSource: "user"`, put
  there by the host's shell unit, which `PUT`s every row on every guest start.

- `auth-status.json` — `GET /api/v1/auth/status` **without a key**, recorded
  2026-10-11 from the running bindery inside its container (design §52). The
  answer is verbatim: four flags and the auth mode, no name, nothing secret.
  `setupRequired` is `false` -- the host's bindery has its throwaway account.
- `constructed-auth-status-open.json` — built by hand: the recording with
  `setupRequired` set to `true`, which is what `Status` answers while
  `users.Count` is 0 (`internal/api/auth.go:114-143` at `v1.33.2`). A fresh
  instance to record it from would have had to be set up for the purpose.

- `setting-not-found.json` — `GET /api/v1/setting/search.interval`, recorded
  2026-10-12 from the running bindery (design §53). The answer is verbatim and
  came with status **404**: no migration seeds the key, so it has no row until
  somebody writes it. `import.mode` answered 200 in the same minute.
- `constructed-setting-search.interval.json` — built by hand after
  `setting-import.mode.json`: what the same path answers once the row exists
  (`SettingsHandler.Set` upserts and `Get` returns the row,
  `internal/api/settings_handler.go:252-304` at `v1.33.2`). Recording it would
  have meant writing the setting by hand first.
