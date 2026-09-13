# Source of these fixtures

Recorded 2026-09-13 around 18:20 CEST from a running Lidarr 3.1.0.4875
(nixpkgs package, NixOS 26.05) inside a systemd-nspawn container.

- `system-status.json` — `GET /api/v1/system/status`
- `naming.json` — `GET /api/v1/config/naming`
- `mediamanagement.json` — `GET /api/v1/config/mediamanagement`
- `rootfolder.json` — `GET /api/v1/rootfolder`
- `qualityprofile.json` — `GET /api/v1/qualityprofile`
- `metadataprofile.json` — `GET /api/v1/metadataprofile`

All were recorded verbatim with

```
systemd-run --machine=<container> --wait --pipe --quiet --collect \
  -p LoadCredential=k:<path of the key> -- /run/current-system/sw/bin/bash -c \
  'k=$(< $CREDENTIALS_DIRECTORY/k); exec curl -sS \
     -H "X-Api-Key: $k" http://localhost:8686/api/v1/<endpoint>'
```

None of these endpoints carries credentials. The files were searched for
long tokens before committing; the only hits are field names and the Nix
store hash in `startupPath`. The paths are the host's library layout, not a
secret.

At recording time the host's desired state (`lib/arr-einstellungen.nix`,
`.lidarr`) matched: media management and the root folder with its profiles
`Standard` (quality id 3, metadata id 1).
