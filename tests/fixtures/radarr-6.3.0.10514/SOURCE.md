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

- `config-downloadclient.json` — `GET /api/v3/config/downloadclient`, recorded
  2026-09-17 around 18:00 CEST from the running service in `media-01`, the same way
  as the files above (the key travelled in a 0600 curl configuration, not in
  `argv`). The answer holds no secret; it is verbatim.

- `config-indexer.json` — `GET .../config/indexer` and `delayprofile.json` —
  `GET .../delayprofile`, both recorded 2026-09-18 from the running service.
  Filtered **on the host** through a `jq` that passes only the fields named in
  the OpenAPI component, so nothing unexpected could travel: every value is a
  number, a boolean, an empty string or an empty list -- these documents hold
  no secret. `preferredProtocol` was already `usenet` at recording time, the
  factory default; both delays were `0`.

- `customformat.json` — `GET /api/v3/customformat`, recorded 2026-09-22 from
  the running service the same way, keys sorted (`jq -S`). **Reduced, not
  masked:** the answer held 78 formats (about 700 KB, most of it the
  `selectOptions` lists a `select` field carries); this file keeps five of
  them **byte for byte** — `1080p`, `Repack/Proper`, `x265 (HD)`, `AV1` and
  `BR-DISK` —, chosen because between them they cover one specification and
  two, a regular expression and a `select` value, and `negate` both ways.
  Nothing that stayed was changed: a custom format carries no credential.

  The count matters when reading the tests: a note here says "5 other
  formats" where the running service would say 78.
