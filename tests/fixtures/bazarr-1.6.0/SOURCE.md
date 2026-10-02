# Source of these fixtures

Recorded 2026-10-02 around 23:30 CEST from a running Bazarr 1.6.0 (the
nixpkgs package, native NixOS module) on the host this program was written
for:

- `language-profiles.json` — `GET /api/system/languages/profiles`
- `languages.json` — `GET /api/system/languages`

each with the key in the `X-API-KEY` header.

**Verbatim.** Neither answer carries a credential: a profile holds its name,
its languages and its filters, and the language list holds Bazarr's 187
languages by name and code. `language-profiles.json` was written with
`jq .`, `languages.json` with `jq -c .`; nothing was changed.

At recording time the service already held the state the host wants: one
profile `DE+EN` with German and English in that order, every switch off, no
cutoff. It was created by the shell unit this task replaces, so a test that
adds the profile to an empty list expects exactly this answer back.

No language was enabled (`enabled` is false 187 times). Bazarr's web page
offers only enabled languages for a profile; the API takes any.

## Constructed

- `constructed-status-version-only.json` — the readiness probe's answer,
  reduced to `{"data": {"bazarr_version": "1.6.0"}}`. The real
  `GET /api/system/status` answer carries twelve more fields, among them the
  installation's directories and the versions of the Sonarr and Radarr it
  talks to; it was not recorded. The version string is the one the running
  instance reported.
