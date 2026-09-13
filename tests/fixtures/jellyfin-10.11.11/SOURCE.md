# Source of these fixtures

Recorded 2026-09-13 around 13:15 CEST from a running Jellyfin 10.11.11
(nixpkgs package, NixOS 26.05) inside a systemd-nspawn container, verbatim:

- `system-info.json` — `GET /System/Info`
- `system-configuration.json` — `GET /System/Configuration`
- `virtual-folders.json` — `GET /Library/VirtualFolders`
- `scheduled-tasks.json` — `GET /ScheduledTasks`

each with

```
systemd-run --machine=<container> --wait --pipe --quiet --collect -- \
  /run/current-system/sw/bin/bash -c \
  'k=$(< <key file>); exec curl -sS -H "X-Emby-Token: $k" http://localhost:8096/<endpoint>'
```

None of these endpoints carries credentials. Checked before committing: no
field name suggesting a password, secret or token; the long values are item
and task ids, the `Key` fields are task names, the rest are paths.

At recording time the service already held the state the host wants:
trickplay extraction on for `Filme` and `Serien`, key-frame-only and hardware
extraction on, and both Merge Versions tasks triggered daily at 05:30
(`TimeOfDayTicks` 198000000000).
