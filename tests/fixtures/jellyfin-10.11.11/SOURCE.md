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

## Plugins (recorded 2026-09-13 around 13:50 CEST)

- `plugins.json` — `GET /Plugins`
- `plugins/<id>.json` — `GET /Plugins/<id>/Configuration` for Jellyfin
  Oscars, MDBList Ratings, Ratings, Bazarr, Media Bar, Mediathek Downloader
  and Auto Collections.

**These are masked, not verbatim.** Plugin configurations carry API keys.
Before the answers left the host, every non-empty string field whose name
matches `key|token|secret|passw|auth` (case-insensitive) was replaced with
`"<masked>"` by `jq` on the host itself — four fields in total (`OmdbApiKey`,
`MdbListApiKey`, `TmdbApiToken`, `BazarrApiKey`). The remaining long values
were checked there too: plugin ids and download paths only. Everything else,
including empty strings as `""` (the JSON API does not omit them; the XML
file on disk does), is as recorded.

## SkinManager and JavaScript Injector (recorded 2026-09-13 around 14:16 CEST)

- `plugins/e10fb9d4c9414c6e82602641031c2618.json` — SkinManager
- `plugins/f5a34f7b2e8a4e6aa7223a216a81b374.json` — JavaScript Injector

A second run over every plugin, masked on the host the same way (the pattern
additionally covered `cookie`, `smtp.*user` and `username`) and written with
`jq -S`, so keys are sorted. **No field of these two was masked.** They carry
a theme choice and scripts: the host's own `Skin-Manager-Vorgabe` script
(public in the host repository) and the scripts other plugins register. The
long values were checked before committing: GUIDs, field names and
JavaScript identifiers; every `Token` is a variable name in a script.

GetAvatar was recorded too and deliberately left out: its `UserAvatars` says
which person picked which picture.
