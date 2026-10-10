# Source of these fixtures

Recorded 2026-10-10 around 10:57 CEST from a running Radarr 6.4.4.10685
(nixpkgs package, NixOS 26.05) inside a systemd-nspawn container, for the
`import-lists` task (design §50). The key travelled in a header file
(`vantage run --header-file`), never in `argv`.

- `importlist.json` — `GET /api/v3/importlist`, verbatim: the service had no
  import list, the answer is `[]`.
- `importlist-schema.json` — `GET /api/v3/importlist/schema`, **cut down on
  the workstation** to the one template the host uses:
  `jq '[.[] | select(.implementation=="TMDbListImport")]'`. The answer holds
  nineteen templates and 47 kB; none of them carries a stored value. Note
  what it shows: the template has **no `rootFolderPath` key** (null values
  are omitted) and no `name`.
- `qualityprofile-names.json` — `GET /api/v3/qualityprofile`, **cut down** to
  `jq 'map({id, name})'`: the task reads nothing else of it, and the whole
  answer is 94 kB of custom format scores.

None of the three carries a secret; each was looked at as a shape (paths and
types) before it was copied here.
