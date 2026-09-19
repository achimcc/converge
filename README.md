# converge

Brings a self-hosted service to a desired state through its HTTP API — and
checks every field name it relies on before it ever writes one.

Nix, Ansible and friends can write a configuration *file*. Much of what a
service like Radarr is configured with does not live in a file but in its
database, and the only way in is the API. The usual answer is a shell script
with `curl` and `jq` run after the service starts. `converge` is what such a
script should have been:

- it **waits** for the service, with a deadline;
- it **reads** the current state into typed structs and fails loudly on a
  missing field or an empty answer;
- it **writes only when something differs**;
- it **reads back** until the value has actually landed — `202 Accepted` is
  not "saved";
- it never puts the API key into `argv`, a URL, a log line or an error.

## Status

Early. **v0.28.0** — tasks for **Radarr**, **Sonarr** (API v3), **Lidarr**,
**Prowlarr** (API v1), **Jellyfin** (10.11), **Trailarr** (0.11), **ntfy** (2.26), **bindery** (1.33), **Seerr** (3.2), **Koel** (9.11),
**SuggestArr** (2.14), **Kavita** (0.9), **Audiobookshelf** (2.36) and
**Dispatcharr** (0.31), each replacing a shell unit or an OpenTofu resource on
the host it was written for:

| service | task | desired state |
|---|---|---|
| Radarr, Sonarr | `quality-definitions` | size limits (MB per minute) per quality |
| Radarr, Sonarr | `quality-profiles` | qualities every profile must allow |
| Radarr, Sonarr, Lidarr | `naming`, `media-management` | top-level fields of the configuration document |
| Radarr, Sonarr, Lidarr | `download-client-config` | the same for `config/downloadclient` -- among them the switch every import hangs on |
| Radarr, Sonarr, Lidarr | `indexer-config` | the same for `config/indexer`: retention, minimum age, maximum size, RSS interval. Only Radarr carries the four extra fields |
| Radarr, Sonarr, Lidarr | `delay-profiles` | the **default** delay profile -- the one without tags -- by its fields: `preferredProtocol` and the two delays. A tagged profile belongs to whoever set the tag and is left alone; `tags` and `order` cannot be set, they say which profile is meant |
| Radarr, Sonarr, Lidarr, Prowlarr | `download-clients`, `notifications` | providers by name: top-level fields, `fields` entries by name, secrets from credentials handed over on every `apply` |
| Prowlarr | `applications` | the same for Prowlarr's links to Radarr, Sonarr and Lidarr |
| Prowlarr | `indexers`, `indexer-proxies` | the same, added from a named template; tags by label (missing labels are added); secrets the service shows are compared, never printed |
| Radarr, Sonarr, Lidarr | `root-folders` | root folders by path; Lidarr's with fields and profiles by name; missing ones are added |
| Jellyfin | `server-configuration` | fields of `ServerConfiguration`, by path |
| Jellyfin | `named-configuration` | fields of a named configuration (`network`, `branding`, `livetv`), by path, checked against its component |
| Jellyfin | `library-options` | fields of the named libraries' `LibraryOptions`, by path |
| Jellyfin | `scheduled-task-triggers` | the trigger list of tasks whose key starts with a prefix |
| Jellyfin | `plugin-configurations` | fields of plugin configurations by path, keys from credentials, entries of shared lists by key, library ids by library name |
| Trailarr | `connections` | connections to Radarr and Sonarr by name: top-level fields, key from a credential; missing ones are added |
| Trailarr | `trailer-profiles` | fields every trailer profile gets |
| bindery | `download-clients`, `prowlarr-instances` | entries by name: top-level fields; secrets from credentials handed over on every `apply` in a `PUT` with only the secret (bindery answers them empty) |
| bindery | `root-folders`, `settings` | root folders by path (added when missing); settings by key |
| bindery | `oidc-providers` | the providers of bindery's own login, by `id`: fields compared, `client_secret` handed over on every `apply` (write-only); `PUT` replaces the whole list, so a provider the spec does not name travels back untouched |
| bindery | `indexers` | the switch over the indexers Prowlarr synced in: the named ones on, every **other** `torznab` one off, anything else left alone. The only task that writes to an entry the spec does not name; a name bindery does not hold is an error. Only rows that differ are written -- each `PUT` takes that row's seed-ratio override away from bindery's Prowlarr syncer |
| ntfy | `account-subscriptions` | subscriptions of an account; the topics are secrets, read from a credential and never printed |
| Seerr | `main` | top-level fields of the main settings (merged) |
| Seerr | `jellyfin` | the link to Jellyfin (key from a credential, compared) and the libraries Seerr scans, by name -- exactly these |
| Seerr | `radarr-servers`, `sonarr-servers` | entries by name: top-level fields, key from a credential; quality profile and root folder by name, resolved through Seerr's connection test; missing entries are added |
| Seerr | `webhook` | the webhook agent: fields by path, the payload template as an object (stored the way the agent parses it), header values from credentials |
| Koel | `radio-stations` | the account's own stations by name (refused while its `include_public_media` is on), every field written whole; a logo from an image file (at most 2 MiB), sent only where a station has none; missing stations are added |
| Kavita | `server-settings` | fields of `ServerSettingDto`, by path -- among them the OIDC switches. The key is an auth key of an administrator in `x-api-key`. What the host writes into `appsettings.json` (authority, client id, secret, scopes, port, addresses, base URL, cache size), the SMTP password and Kavita's own install fields are refused |
| Kavita | `libraries` | libraries by a folder they hold: fields of the update, written whole; a change of `type` is followed by a forced scan. converge does not create libraries |
| Audiobookshelf | `auth-settings` | the authentication settings by name, `PATCH`ed key by key; the OIDC client secret from a credential, compared without being shown. `""` and `null` are one value, as Audiobookshelf treats them (except the redirect subfolder) |
| Audiobookshelf | `admin-permissions` | permissions every account of the named types must hold, `PATCH`ed per account with the differing keys only; the answer carries every account's token, and nothing but username, type and the named permissions is ever shown |
| Dispatcharr | `stream-settings` | the default stream profile, by name |
| Dispatcharr | `m3u-accounts`, `epg-sources` | accounts and sources by name: a missing one is added, the named fields are set; an account's `server_url`, `username` and `password` and a source's `url` may come from credentials (`secret_fields`: compared unseen, the write-only password handed over on every apply); every task logs in as a service account, once per run |
| Dispatcharr | `m3u-groups` | the settings of channel groups within an account, among them the stream profile the group's channels get (by name); each group is written whole, so what the spec does not name stays; readiness waits until the account's playlist has been read |
| SuggestArr | `configuration` | the whole flat configuration: plain fields by name, secret fields from credentials (never shown), and the Jellyfin libraries derived from what the service reports, minus the collection types named |

## A spec

```json
{
  "service": "radarr",
  "base_url": "http://localhost:7878",
  "api_key_credential": "radarr-api-key",
  "task": "quality-definitions",
  "desired": {
    "Bluray-1080p": { "min": 12.5, "preferred": null, "max": null },
    "CAM":          { "min": 0,    "preferred": 95,   "max": 100 }
  }
}
```

Sizes are MB per minute, as the service stores them; `null` is unlimited.
Every key is required and unknown keys are an error, so `prefered` fails
instead of being ignored. The key itself is read from the systemd credential
named in `api_key_credential` (`$CREDENTIALS_DIRECTORY/<name>`). Qualities the
spec does not name are left as they are.

A `quality-profiles` spec only allows, it never takes a quality away, and it
only touches rungs that already exist at the top level of a profile's ladder:

```json
{
  "service": "sonarr",
  "base_url": "http://localhost:8989",
  "api_key_credential": "sonarr-api-key",
  "task": "quality-profiles",
  "desired": { "allow_in_every_profile": ["Unknown"] }
}
```

For Jellyfin, the spec names fields by path rather than converge declaring
them in code — `ServerConfiguration` alone has 56 properties. The paths are
checked all the same: at build time against the OpenAPI component (type,
nullability, enum membership), at runtime against the object Jellyfin
returns. A path the answer does not carry is an error, never an addition.
Plugin configurations are the exception the OpenAPI file forces: it
describes them without a single property, so their fields are checked at
runtime only — see `docs/design.md` §7. Values for keys come from systemd
credentials and are never printed. Named configurations
(`/System/Configuration/{key}`) are declared without a schema too; there the
key is a fixed list (`network`, `branding`), each checked against the
component its document is (`docs/design.md` §15).

```json
{
  "service": "jellyfin",
  "base_url": "http://localhost:8096",
  "api_key_credential": "jellyfin-api-key",
  "task": "scheduled-task-triggers",
  "desired": {
    "key_prefix": "Merge",
    "triggers": [ { "Type": "DailyTrigger", "TimeOfDayTicks": 198000000000 } ]
  }
}
```

A named configuration names its key and the fields by path; `schema-check`
checks `KnownProxies` against `NetworkConfiguration`, because that is the
document the key `network` stands for:

```json
{
  "service": "jellyfin",
  "base_url": "http://localhost:8096",
  "api_key_credential": "jellyfin-api-key",
  "task": "named-configuration",
  "desired": {
    "key": "network",
    "set": { "KnownProxies": ["10.0.20.11"], "EnableUPnP": false }
  }
}
```

Plugins that name libraries by id get them by name: `{"$library_ids": [...]}`
stands, at any depth of a `set` value, for the ids of those libraries in the
order named, looked up on every run. An unknown name fails before anything is
written (`docs/design.md` §16):

```json
{
  "service": "jellyfin",
  "base_url": "http://localhost:8096",
  "api_key_credential": "jellyfin-api-key",
  "task": "plugin-configurations",
  "desired": {
    "958aad6637844d2ab89aa7b6fab6e25c": {
      "name": "LDAP-Auth",
      "set": { "EnableAllFolders": false,
               "EnabledFolders": { "$library_ids": ["Filme", "Serien"] } },
      "secrets": { "LdapBindPassword": "jellyfin-ldap-bind-password" }
    }
  }
}
```

Koel's stations are a list found by name. The token is a Sanctum token that
travels as `Authorization: Bearer`, and every request asks for JSON — without
`Accept: application/json` Koel redirects a refused request to its web page.
Every write carries the whole entry, because Koel's update makes a station
private when `is_public` is left out; `logo_file` is read on every run and
sent only when the station has no logo, since Koel stores images under random
names. The list Koel answers also holds other people's public stations and
names no owner, so converge refuses to run until the token's account has
`include_public_media` off — then every station in it is the account's own
(`docs/design.md` §18):

```json
{
  "service": "koel",
  "base_url": "http://10.0.254.10",
  "api_key_credential": "koel-token",
  "task": "radio-stations",
  "desired": {
    "stations": [
      { "name": "Radio Dreyeckland", "url": "https://stream.rdl.de/rdl",
        "description": "Free radio from Freiburg.", "is_public": true,
        "homepage_url": "https://rdl.de/", "logo_file": "/nix/store/…-rdl.png" }
    ]
  }
}
```

SuggestArr keeps its configuration in one flat document, and the database
behind it — not `config.yaml` — decides what the service reads: the file is
copied into the `integrations` table once and never consulted for those keys
again, so a rotated key written to the file would never arrive.
`POST /api/config/save` writes both. That endpoint also fills every key it is
not given with its **default**, so converge reads the document, changes the
named fields and sends it back whole. There is no API key for it: the
credential holds the password of a service account, `POST /api/auth/login`
exchanges it for a JWT, and every later request carries that as a bearer
token — the one request that carries no key is the login. `GET
/api/config/fetch` answers with real keys, so every secret field is reported
as `(hidden)`, whatever its value.

`jellyfin_libraries` derives `JELLYFIN_LIBRARIES` from what SuggestArr itself
reports (`GET /api/jellyfin/libraries`) rather than from a written-down list
of ids: a library renamed in Jellyfin keeps working, and a new one joins by
itself. An empty list in SuggestArr means *all of them*, so the types to
leave out are named; a rule that would leave nothing is an error
(`docs/design.md` §19):

```json
{
  "service": "suggestarr",
  "base_url": "http://10.0.50.10:5000",
  "api_key_credential": "suggestarr-converge-passwort",
  "task": "configuration",
  "desired": {
    "username": "converge",
    "set": { "FILTER_RATING_SOURCE": "both", "FILTER_IMDB_THRESHOLD": 6.0 },
    "secrets": { "OMDB_API_KEY": "omdb-api-key" },
    "jellyfin_libraries": { "exclude_collection_types": ["homevideos"] }
  }
}
```

## Commands

| command | does | exit |
|---|---|---|
| `converge apply [--deadline <s>] <spec>...` | reconcile, write, read back | 0 done, 1 any spec failed |
| `converge plan [--deadline <s>] <spec>...` | show what `apply` would change | 0 equal, 2 differs, 1 error |
| `converge schema-check --service <radarr\|sonarr\|lidarr\|prowlarr\|jellyfin\|trailarr\|kavita> --openapi <file> [--spec <spec>]...` | compare the wire types — and the field paths of the given specs — with an OpenAPI file | 0 / 1 |
| `converge schema-check --service <ntfy\|bindery\|seerr\|koel\|suggestarr\|audiobookshelf> [--spec <spec>]...` | ntfy and bindery publish no OpenAPI description, Seerr's misnames its fields, Koel's describes a long-gone version, SuggestArr's covers only its public `/api/v1`: only validate the specs (`--openapi` is refused) | 0 / 1 |

Several specs are processed in order; one failing does not skip the next.

Trailarr drops body fields its models do not have without a word — the
host's old shell unit sent `monitor` where Trailarr 0.11.5 has
`monitor_new_media`. `schema-check --spec` checks every field a connection
spec sets against both `ConnectionCreate` and `ConnectionUpdate`, so that
typo fails the build (`docs/design.md` §9). For ntfy, whose topic names grant
read access, output names a topic only by its line in the credential:
`ntfy account: subscription 3 (missing) -> (added)` (§10).

```
radarr quality-definitions: service version 6.3.0.10514
radarr quality-definitions: note: not in the spec, left as they are: Raw-HD
radarr quality-definitions: Bluray-1080p: min 12.5 -> 35
radarr quality-definitions: changed 1 field(s), read back and confirmed
```

## Field names, checked three times

1. **At runtime**, strictly: required fields are required, an empty list is
   an error, fields the program does not know are carried along and written
   back untouched.
2. **Against recorded answers** (`tests/fixtures/`), so a guessed field name
   fails at the desk, not on the server. Each recording says where it came
   from.
3. **Against the OpenAPI description of the deployed version.** The wire
   types derive their JSON schema; `schema-check` compares it with the
   service's `openapi.json`. Run it in your build against the exact version
   you deploy, and an upgrade that renames a field fails before the deploy.
   ntfy and bindery publish no such description, and Seerr's and Koel's do
   not fit the running service; for them `schema-check` validates the specs,
   and only the first two checks apply to their fields.

OpenAPI describes names, not behaviour: Radarr's file lists `200` for the
update, the service answers `202`. That is what reading back is for.

## As a systemd unit (NixOS)

```nix
systemd.services.quality-sizes = {
  after = [ "radarr.service" ];
  serviceConfig = {
    Type = "oneshot";
    LoadCredential = [ "radarr-api-key:/run/secrets/radarr-api-key" ];
    ExecStart = "${converge}/bin/converge apply ${pkgs.writeText "radarr.json" (builtins.toJSON spec)}";
    TimeoutStartSec = "6min";  # above the 5 min default deadline per spec
  };
};
```

## Building

```
nix build            # the binary
nix flake check      # tests, clippy, rustfmt
```

## License

AGPL-3.0-only.
