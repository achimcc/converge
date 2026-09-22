# Source of these fixtures

Recorded 2026-09-22 from the running authentik of the host this program was
written for (2026.5.6), with a bearer token in `Authorization` (design §37).
The token was a fifteen-minute token of a superuser service account, the kind
the host's unit creates for each run; it is in neither file.

- `settings.json` — `GET /api/v3/admin/settings/`.
- `version.json` — `GET /api/v3/admin/version/`.

**Verbatim, keys sorted** (`jq -S`). Nothing was masked, because the tenant
settings carry no secret: avatar mode, retention durations, page sizes and the
two switches the host sets (`impersonation` false, `reputation_lower_limit`
-10). The version answer carries versions and a build hash only.

Both endpoints answer HTTP 403 for a token whose account is no administrator
and HTTP 401 for a token authentik does not know; both bodies are
`{"detail": …}`, which no error of this program repeats.
