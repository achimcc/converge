# Source of these fixtures

Recorded 2026-09-13 around 11:47 CEST from a running Sonarr 4.0.19.2979
(nixpkgs package, NixOS 26.05) inside a systemd-nspawn container.

- `system-status.json` — `GET /api/v3/system/status`
- `qualitydefinition.json` — `GET /api/v3/qualitydefinition`

Both were recorded verbatim with

```
systemd-run --machine=<container> --wait --pipe --quiet --collect \
  -p LoadCredential=sonarr-api-key -- /run/current-system/sw/bin/bash -c \
  'k=$(< $CREDENTIALS_DIRECTORY/sonarr-api-key); exec curl -sS \
     -H "X-Api-Key: $k" http://localhost:8989/api/v3/<endpoint>'
```

Neither endpoint carries credentials. The files were searched for long tokens
before committing; the only hits are Nix store hashes in `startupPath`.

Note what the recording shows and a hand-written fixture would not: **null
values are omitted**, not written as `null` — a quality without an upper
limit has no `maxSize` key at all.

- `desired.json` — the desired state the host applies: its table
  `lib/qualitaetsgroessen.nix` rendered with `nix eval --json`, the
  `.sonarr` attribute. At recording time the service held exactly this
  state.
