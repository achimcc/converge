# Source of these fixtures

Recorded 2026-09-13 around 11:47 CEST from a running Radarr 6.3.0.10514
(nixpkgs package, NixOS 26.05) inside a systemd-nspawn container.

- `system-status.json` — `GET /api/v3/system/status`
- `qualitydefinition.json` — `GET /api/v3/qualitydefinition`
- `qualityprofile.json` — `GET /api/v3/qualityprofile`, recorded 2026-09-13
  around 12:50 CEST the same way. Groups (`items` without a `quality`
  key) and the top-level `Unknown` item come from here.

Both were recorded verbatim with

```
systemd-run --machine=<container> --wait --pipe --quiet --collect \
  -p LoadCredential=radarr-api-key -- /run/current-system/sw/bin/bash -c \
  'k=$(< $CREDENTIALS_DIRECTORY/radarr-api-key); exec curl -sS \
     -H "X-Api-Key: $k" http://localhost:7878/api/v3/<endpoint>'
```

Neither endpoint carries credentials. The files were searched for long tokens
before committing; the only hits are Nix store hashes in `startupPath`.

Note what the recording shows and a hand-written fixture would not: **null
values are omitted**, not written as `null` — a quality without an upper
limit has no `maxSize` key at all.

- `desired.json` — the desired state the host applies: its table
  `lib/qualitaetsgroessen.nix` rendered with `nix eval --json`, the
  `.radarr` attribute. At recording time the service held exactly this
  state.

- `naming.json` — `GET /api/v3/config/naming`
- `mediamanagement.json` — `GET /api/v3/config/mediamanagement`
- `rootfolder.json` — `GET /api/v3/rootfolder`

These three were recorded 2026-09-13 around 18:20 CEST the same way (with
the credential loaded under another name). None of them carries credentials;
searched for long tokens, the only hits were field names.

- `downloadclient.json`, `notification.json` — the providers, and
  `downloadclient-schema.json`, `notification-schema.json` — the templates
  of the implementations the host uses (QBittorrent, Sabnzbd, Webhook),
  recorded 2026-09-13 around 18:30 CEST. **Masked on the host** before they
  left it: every `fields` entry whose `privacy` is not `normal` and whose
  value is not `********`, `""` or `null` became `"<masked>"` (that hits the
  `userName` values), and any other string of 24 or more token characters
  would have become `"<masked-token>"` (none did). Passwords and keys arrive
  as `********` already.
