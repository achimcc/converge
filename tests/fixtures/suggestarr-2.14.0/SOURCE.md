# Source of these fixtures

Recorded 2026-09-17 around 16:13 CEST from a running SuggestArr 2.14.0
(the derived image `suggestarr-trusted-header:v2.14.0-10`) inside a
systemd-nspawn container.

- `config-fetch.json` — `GET /api/config/fetch`
- `jellyfin-libraries.json` — `GET /api/jellyfin/libraries`

Both were recorded with a JWT obtained from `POST /api/auth/login` for the
service account `converge`; the password travelled in a request body file,
never in `argv`.

**Masked on the host** before they left it, in two steps:

- Every key of SuggestArr's own `_SECRET_KEYS` set with a non-empty value
  became `"<masked>"`, and so did every field whose name contains `api_key`,
  `token`, `password`, `secret` or `psw` anywhere in the nested
  `integrations` block. Any other string of 20 or more characters from
  `[A-Za-z0-9_+/=-]` became `"<masked-token>"`.
- The Jellyfin `ItemId` values (and `PrimaryImageItemId`) were then replaced
  by stable pseudo ids derived from the library name. They are object ids,
  not credentials, but nothing about this house's Jellyfin needs to be in a
  public repository. Everything else — library names, paths, addresses — is
  verbatim.

`config-fetch.json` is the answer *before* converge ever wrote: `OMDB_API_KEY`
is `""`, `FILTER_RATING_SOURCE` is `tmdb`, and the settings the host's shell
unit had written are in place. `jellyfin-libraries.json` carries seven
libraries, one of them (`Privat`) with `CollectionType: homevideos` — the one
the desired state excludes.
