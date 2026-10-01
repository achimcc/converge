# Source of these fixtures

Recorded 2026-10-01 around 18:30 CEST from a Questarr **1.4.2** (tag `v1.4.2`,
built from source with Nix, Node 22, SQLite) started in a throwaway directory
on a workstation, listening on `127.0.0.1:5999`. Nothing here comes from a
deployed instance: the account, both download clients and the Prowlarr key
were made up for the recording, and the database was deleted afterwards.

`status-codes.txt` lists every request in order with the HTTP status it got.

## What was recorded

- `health.json` — `GET /api/health`, no token
- `auth-status-fresh.json`, `auth-status-set-up.json` — `GET /api/auth/status`
  before and after the account exists, no token
- `setup-answer.json` — `POST /api/auth/setup` on the fresh instance
- `setup-refused.json` — the same request once an account exists (403); a body
  with a too-short username gets the same 403, so the account check comes first
- `login-answer.json`, `login-refused.json` — `POST /api/auth/login` with the
  right and with a wrong password (401)
- `unauthenticated.json` (401, no `Authorization`) and `invalid-token.json`
  (**403**, a bearer token that is not one)
- `auth-me.json` — `GET /api/auth/me`
- `downloaders-empty.json`, `downloader-created.json` (the answer to the
  `POST`, 201), `downloaders.json` (SABnzbd and qBittorrent),
  `downloader-patched.json` (answer to `PATCH {"category": …}`),
  `downloader-test.json` (`POST /api/downloaders/{id}/test` against an address
  nothing listens on)
- `indexers-empty.json`, `prowlarr-sync-answer.json`,
  `prowlarr-sync-answer-again.json`, `indexers.json`,
  `prowlarr-sync-unreachable.json` (500) — the sync ran against a stub that
  answers `GET /api/v1/indexer` with one usenet and one torrent indexer
- `imports-config-default.json`, `imports-config-patched.json` (answer to the
  `PATCH`), `imports-config-set.json` (the `GET` after it),
  `imports-config-refused.json` (400, a transfer mode that does not exist),
  `imports-config-unknown-field.json` (400)

## What was masked, and what was deliberately not

Both `token` values (`setup-answer.json`, `login-answer.json`) became
`"<masked>"` before the files were copied here. They were signed with a JWT
secret that existed only for the recording.

Everything else is as answered, because the point of these files is what
Questarr shows and what it hides:

- A download client's `password` is answered as `"********"` when one is
  stored and as `null` when none is. The qBittorrent password that was sent
  appears in no answer.
- A download client's `username` is answered in **clear**. SABnzbd keeps its
  API key in that field (`shared/schema.ts`: there is no `apiKey` column), so
  `downloaders.json` shows the made-up key `fixture-sab-key-never-real`.
- An indexer's `apiKey` is answered as `"********"`. The Prowlarr key that was
  sent appears in no answer.
- `downloader-test.json` carries the SABnzbd key inside its `message`:
  Questarr puts the whole request URL, `apikey=` included, into the failure
  text (and into its log). converge never calls that endpoint.

## What the recording settled

- An unknown field in `PATCH /api/downloaders/{id}` is ignored (200, the field
  is not in the answer); in `PATCH /api/imports/config` it is refused (400).
- `PATCH /api/downloaders/{id}` with `{"password": "********"}` leaves the
  stored password as it is.
- Timestamps are ISO strings with milliseconds (`2026-10-01T16:32:05.000Z`).
- A synced indexer's `url` is `<Prowlarr URL>/<Prowlarr id>/api`; a second
  sync reports `0 added, 2 updated` and resets `categories` to `[]`.
- The answer to `PATCH /api/imports/config` equals the `GET` that follows.
