# Source of these fixtures

Recorded 2026-09-18 from the running Audiobookshelf 2.36.0 (nixpkgs package)
of the host this program was written for (design §27):

- `status.json` — `GET /status`, which needs no token.
- `auth-settings.json` — `GET /api/auth-settings`, with a short-lived access
  token of the root account.

**This is masked, not verbatim.** `authOpenIDClientID` and
`authOpenIDClientSecret` were replaced by `<masked>` before the answer left the
host — Audiobookshelf answers both in clear text to an administrator. Keys are
in sorted order (`jq -S`). Everything else is as recorded, among it
`authOpenIDAdvancedPermsClaim` as `""` (see §27 on empty strings).
