# Source of these fixtures

Recorded 2026-09-14 around 15:30 CEST from a running Seerr 3.2.0 (nixpkgs
package, NixOS 26.05) inside a systemd-nspawn container, with the service's
own API key as a credential inside the container (it acts as user 1, the
administrator).

- `main.json` — `GET /api/v1/settings/main`
- `jellyfin.json` — `GET /api/v1/settings/jellyfin`
- `radarr.json`, `sonarr.json` — `GET /api/v1/settings/radarr`, `…/sonarr`
- `webhook.json` — `GET /api/v1/settings/notifications/webhook`
- `radarr-0-profiles.json` — `GET /api/v1/settings/radarr/0/profiles`
  (Sonarr has no such route: 404)
- `radarr-test.json`, `sonarr-test.json` — `POST /api/v1/settings/radarr/test`
  and `…/sonarr/test`, with the stored entry's connection fields as the body.
  The route asks the service and stores nothing. **Trimmed:** `profiles` to
  `id` and `name` (a Radarr quality profile is a page of JSON), the rest
  (`rootFolders`, `tags`, `urlBase`) as answered.

**Masked more strictly than by field name.** Seerr answers the API keys of
Radarr, Sonarr and Jellyfin and the webhook's header value in the clear. On
the host `jq` replaced **every non-empty string** with `"<masked>"` except
the fields of an allow list: `name`, `type`, `activeProfileName`,
`activeDirectory`, `minimumAvailability`, `locale`, `discoverRegion`,
`streamingRegion`, `originalLanguage`, `applicationTitle`, `jsonPayload`,
`key`, `id`, `path`, `label`, `urlBase`, `enabled`. Empty strings stay empty.
Numbers, booleans, nulls and the shape are as recorded, keys sorted (`jq -S`).
The library ids are Jellyfin's, the same as in `jellyfin-10.11.11/`.

What the answers show that the description (`share/seerr-api.yml`) does
not: `JellyfinSettings` is documented with `hostname` and `serverID`, the
answer carries `ip`, `port`, `useSsl`, `urlBase`, `serverId` and `apiKey`;
`MainSettings` is documented without `locale`, `discoverRegion`,
`streamingRegion`, `cacheImages`; `WebhookSettings` without `embedPoster`.
`jsonPayload` comes back as a **string** holding the template's JSON: the
host stored it double-encoded, which is what the agent parses.

At recording time the service held the state the host wants: `plan` with
the host's five specs reported `unchanged` (see design §17).
