# Source of these fixtures

Recorded 2026-09-18 from the running Kavita of the host this program was
written for (nixpkgs package 0.9.1.4, which reports itself as 0.9.1.3), with
an auth key of an administrator in `x-api-key` (design §25):

- `settings.json` — `GET /api/Settings`.
- `server-info-slim.json` — `GET /api/Server/server-info-slim`.
- `libraries.json` — `GET /api/Library/libraries` (recorded the same day,
  unmasked: folder paths, names and switches carry no secret).

**This is masked, not verbatim.** Before the answers left the host:

- `oidcConfig.secret` was replaced by `********`. Kavita answers asterisks
  of the secret's length there anyway; the length was not kept.
- `installId` was replaced by the nil UUID in both files.
- every non-empty string among the SMTP user and password fields would have
  been replaced by `<masked>` — on this host they are empty, as recorded.

**Built by hand, not recorded:**

- `constructed-libraries-after-create.json` — what `GET /api/Library/libraries`
  is expected to answer after `POST /api/Library/create` made a second
  library (name `Zeitungen`, type 1, folder `/tank/data/media/zeitungen`).
  No create was recorded: the instance had one library, and a second one
  made for a recording would have had to be deleted by hand. It is
  `libraries.json` plus a copy of its entry with `id`, `name`, `type`,
  `folders`, `allowScrobbling` (`false`: `AddLibrary` switches it off for
  Comic), `excludePatterns` (`[]`), `metadataProvider` (`4`) and
  `lastScanned` (`0001-01-01T00:00:00`, never scanned) changed, following
  `LibraryController.AddLibrary` at the tag `v0.9.1.4`. `includeInSearch`
  is `true` whatever was sent, since `AddLibrary` does not copy it.

Keys are in sorted order (`jq -S`). Everything else is as recorded.

Without a key, and with a key Kavita does not know, both endpoints answer
HTTP 401 with an empty body; `GET /api/Health` answers `Ok` without one.
