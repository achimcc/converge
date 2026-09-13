# converge — design

Status: agreed 2026-09-13, pilot scope. Sections 1 and 2 were agreed
section by section; sections 3 and 4 follow from decisions taken in the same
conversation (checked field names at three levels; a public English repo).

## 0. Why this exists

Nix can write a configuration *file*. Much of what a self-hosted service is
configured with does not live in a file but in the service's own database —
Radarr's quality size limits, Jellyfin's plugins, Seerr's link to Radarr.
The only way in is the running service's HTTP API.

The host this was written for solves that with *seeding units*: a oneshot
that, after the service is up, talks to its API with `curl` and `jq` and
writes the wanted values. There are 32 of them, about 6000 lines of shell
embedded in Nix strings. Every one re-implements the same seven steps —
wait until ready, read, compare, write, check the status code, read back
until the write has actually landed, report — and every re-implementation is
another chance at a trap that has already been hit once:

| trap | hit by |
|---|---|
| `202 Accepted` read as "saved" | quality sizes, 2026-09-06 |
| `exit 0` in the middle of a unit that does two things | skin manager, 2026-09-06 |
| response larger than 128 KiB passed through `argv` | cleanup unit, 2026-09-09 |
| `grep -q` under `pipefail` reports a present match as missing | guest probe, 2026-09-05 |
| field names guessed, found only after three deploy cycles | account seeding, 2026-09-12 |
| wait loop without a deadline holds the guest's boot | eleven units, review 2026-09-11 |
| status code printed but not acted upon | mail sent to everybody, 2026-09-05 |

`converge` does the seven steps once, correctly, and checks the field names
it relies on — on the input side, against the live answer, and against the
service's published OpenAPI description of exactly the version deployed.

## 1. Scope and shape

**A command-line program. One invocation reconciles one or more *specs*; one
spec is one task against one service instance.** No daemon, no database, no
state of its own: the desired state comes from the caller, the current state
lives in the service.

### The spec

One JSON file per task, written by Nix:

```json
{
  "service": "radarr",
  "base_url": "http://localhost:7878",
  "api_key_credential": "radarr-api-key",
  "task": "quality-definitions",
  "desired": {
    "Bluray-1080p": { "min": 35, "preferred": 95, "max": 100 }
  }
}
```

Parsed with `deny_unknown_fields` at every level, so a typo on the caller's
side (`prefered`) is an error, not a silently ignored key. The spec never
holds the key itself, only the name of a systemd credential; the value is
read from `$CREDENTIALS_DIRECTORY/<name>`.

### Commands

| command | does | exit |
|---|---|---|
| `converge apply <spec>...` | reconcile, write, read back | 0 done (changed or not), 1 any spec failed |
| `converge plan <spec>...` | show the difference, write nothing | 0 no difference, 2 differs, 1 error |
| `converge schema-check --service <s> --openapi <file>` | compare this program's wire types with the OpenAPI file | 0 / 1 |

Several specs in one call are processed in order; a failure in one does not
skip the next (the shell unit it replaces behaved the same way), but makes
the whole run exit 1.

### Code layout

A single crate.

- `spec` — the input file.
- `secret` — `Secret`, a string without `Debug` or `Display`.
- `client` — HTTP over `ureq`, the API key header, per-request timeouts,
  waiting for readiness.
- `engine` — the shared sequence *read → diff → write → read back*, driven
  through a `Task` trait.
- `services::arr` — wire types for Radarr/Sonarr API v3 and the task
  `quality-definitions`.
- `schema` — the OpenAPI comparison.
- `report` — output lines and exit code.

**Pilot scope:** the task `quality-definitions` for Radarr and Sonarr, which
share the API shape. Nothing else until the pilot has been evaluated.

`ureq` rather than `reqwest`: the program makes one request after another and
has no use for an async runtime. Plain HTTP only in the pilot — every target
is `localhost` — so no TLS stack is linked.

## 2. Sequence and failure handling

Two facts from the OpenAPI files of the deployed versions (Radarr
6.3.0.10514, Sonarr 4.0.19.2979) shape this: all three size fields are
`nullable` (`null` means unlimited), and so is `quality.name`. Radarr's
`Quality` also carries `modifier`, which Sonarr's lacks.

`apply`, step by step, each with its own error:

1. **Load the spec, strictly.** Unknown keys, or `min > preferred`, or
   `preferred > max`, fail before any request is made. Each of `min`,
   `preferred` and `max` is a number or `null`, as on the service; `null`
   means unlimited and counts as infinitely large in the ordering check
   (the host's table uses `null` for most of Radarr's qualities). No upper bounds are
   guessed; the OpenAPI file states none.
2. **Read the key** from `$CREDENTIALS_DIRECTORY/<name>`. A missing
   credential is reported by *name*, never by value. The key only ever
   travels as the `X-Api-Key` header — not in `argv`, not in the URL.
3. **Wait for readiness:** `GET /api/v3/system/status` until an answer with
   `version` arrives, at most 120 s. The version is reported.
4. **Read the current state:** `GET /api/v3/qualitydefinition` into typed
   structs.
   - A missing required field aborts, naming the field.
   - **A `nullable` field may also be absent.** Radarr's serializer omits
     null values instead of writing `null` (recorded 2026-09-13: a quality
     with no upper limit has no `maxSize` key at all). Absent and `null`
     are the same value here. The shell unit this replaces compared the
     two byte-wise, saw a difference on every run and wrote every time
     while logging "changed: nothing".
   - An entry without `quality.name` aborts.
   - Fields this program does not know (such as `modifier`) are carried
     along and written back untouched (`#[serde(flatten)]`), so a field
     added upstream is never erased by a write.
   - **An empty list is an error, not "nothing to do".**
5. **Match names.** A quality in the spec that the service does not know
   aborts, listing all of them. A quality the service has and the spec does
   not mention stays as it is and is listed as a note.
6. **Diff** per quality and field. `null` on the service against a number in
   the spec is a difference. Comparison is exact.
7. **No difference:** report `unchanged`, exit 0, no write.
8. **Write:** `PUT /api/v3/qualitydefinition/update` with the full list.
   - `200` and `202` count as accepted. `202` is observed behaviour and
     contradicts the OpenAPI file, which lists only `200`; the code says so.
   - Any other status is an error. From the body, **only** the validation
     fields (`propertyName`, `errorMessage`) are printed, never the raw
     body.
   - A failed write is not retried; the next timer run is the retry.
9. **Read back:** every 2 s, until no difference remains, at most 60 s.
   If it still differs, that is an error naming quality, current and
   desired value.

**Deadlines.** Each request has its own timeout (10 s). The whole run of one
spec has an overall deadline (default 5 min, `--deadline`), so the program
fails loudly before systemd kills it; the unit's `TimeoutStartSec` sits above
the sum.

**Output.** Plain text on stdout, one line per change
(`Bluray-1080p: min 50.8 -> 35`) and a closing line per spec. Errors on
stderr. No JSON mode until somebody needs one.

**Errors.** One error enum with context; no `panic!` and no `unwrap()`
outside tests. `Secret` has neither `Debug` nor `Display`, so it cannot end
up in an error message even by accident.

**Version drift.** `schema-check` runs against a specific version (section
3). At runtime the reported version is printed; the program does not know
which version its types were checked against and does not pretend to.

## 3. Checking field names — three levels

A field name can be wrong in three places, and each level catches a
different one.

### Level 1 — at runtime, strictly

Section 2: required fields are required, the spec rejects unknown keys, an
empty answer is an error. This catches everything, but only against the live
service — which is to say, inside the deploy cycle.

### Level 2 — recorded answers

`tests/fixtures/<service>-<version>/` holds real responses recorded from a
running instance: `system-status.json` and `qualitydefinition.json`. Tests
parse them with the production types and run the engine against them through
a fake transport. A guessed field name fails at the desk in seconds.

Recording rules:
- Only endpoints that carry no credentials are recorded; the recording is
  still passed through a mask for long tokens and checked by hand.
- Each fixture directory has a `SOURCE.md` saying when, from which version,
  and with which command it was recorded.
- **What cannot be recorded without harm is marked as constructed.** The
  pilot does not provoke a validation error on a live instance; the 400 body
  used in tests is built from the shape Radarr's source produces and says so
  in its file name (`constructed-validation-error.json`).

A fixture goes stale silently when the service is upgraded: the test keeps
checking the old answer. That gap is what level 3 is for.

### Level 3 — the OpenAPI description of the deployed version

Radarr and Sonarr publish `src/<Name>.Api.V3/openapi.json` at every release
tag. `converge schema-check` compares the program's wire types with it:

- **The field list is derived, not written down.** Wire types derive
  `schemars::JsonSchema`; the check walks the generated schema. A hand-kept
  list of fields would be one more list to forget to update.
- Each wire type names its OpenAPI component
  (`QualityDefinitionResource`, `Quality`, `SystemResource`).
- For every property the type declares: the component has it, the JSON type
  is compatible (`integer` ↔ integer types, `number` ↔ `f64`, `string` ↔
  `String`, `$ref` ↔ nested wire type), and if the component marks it
  `nullable`, the Rust type is an `Option`. The flattened catch-all map is
  not a property and is skipped.
- Every endpoint the task uses exists with its method, and its request or
  response schema refers to the expected component.

The repository vendors the OpenAPI files of the versions its fixtures come
from (`openapi/radarr-6.3.0.10514.json`, `openapi/sonarr-4.0.19.2979.json`),
and `cargo test` runs `schema-check` against them. A consumer runs it against
the version it actually deploys (section 4), so an upgrade that renames a
field fails the build, before any deploy.

**What OpenAPI does not tell.** It describes names and types, not
behaviour — the `202` above is the proof. Behaviour stays with level 2 and
with reading back at runtime.

## 4. Integration into the host

The host repository is written in German; this section describes the
contract, and the host documents its side in its own language.

- **Flake input**, pinned to a tag like `treff` and `signal-seerr`:
  `github:achimcc/converge/v0.1.0`, `inputs.nixpkgs.follows = "nixpkgs"`.
- **The unit keeps its name and its timer.** Its shell script is replaced by
  `ExecStart = converge apply <radarr spec> <sonarr spec>`; the specs are
  rendered with `builtins.toJSON` from the table that already exists.
  `LoadCredential` stays as it is. `TimeoutStartSec` stays above twice the
  per-spec deadline.
- **The schema check is part of the host's system build**, not a separate
  flake check (changed during integration, 2026-09-13). The spec file the
  unit runs is a derivation that first fetches `openapi.json` for the version
  of the package the host actually deploys (`config.services.<s>.package`),
  by a hash kept in a table keyed by version, runs `converge schema-check`
  against it, and only then copies the spec. A flake check could be skipped
  by a deploy; this cannot. A version without a hash is an evaluation error
  naming the command that produces it, so a package bump cannot pass without
  the schema being checked again.
- **Acceptance on the running host**, not on the build: `converge plan`
  against both instances reports no difference after the deploy, and a
  deliberate one-field change in the table shows up as exactly one changed
  line in `apply`, then as no difference in `plan`.
  *Done 2026-09-13:* `plan` unchanged for both; `apply` of Radarr's `Unknown`
  min 0 -> 1 and back, each "read back and confirmed"; afterwards all 30
  entries byte-identical to the morning's recording. Deployed as host
  generation 734; the unit's first run reported `unchanged` for both.

## 5. Second task: `quality-profiles` (v0.2.0, 2026-09-13)

Replaces the host's `notnagelstufe` unit: every quality profile of Radarr and
Sonarr must allow `Unknown`, the lowest rung, so a badly named release can be
grabbed when nothing better exists.

- **Only allowing.** The ladder itself belongs to whoever defines profiles
  (Recyclarr with TRaSH templates on that host). converge flips `allowed` on
  an existing top-level item; a profile without that item is an error naming
  it, not something to rebuild.
- **Groups keep their shape.** A group has no `quality` key; the item type
  skips serializing an absent quality, and nested `items` travel in the
  flattened map. Checked against the recorded profiles byte for byte.
- **One PUT per differing profile** to `/api/v3/qualityprofile/{id}`; errors
  name the filled-in path.
- **The schema check follows lists.** `QualityProfileResource.items` is a
  list of `QualityProfileQualityItemResource`; before v0.2.0 the check
  stopped at "array" and never compared the element's fields — the deployed
  versions passed for the wrong reason. It now descends and reports a list of
  the wrong component.

## 6. Jellyfin: fields by path (v0.3.0, 2026-09-13)

Jellyfin's `ServerConfiguration` has 56 properties and `LibraryOptions` 42;
the host sets two or three of them at a time, always the same way: read the
whole object, change a field, post the whole object back (the endpoint
replaces it), read back. Typing every property would be busywork, and a
typed subset would need a release for every new setting. Decided with the
host's owner on 2026-09-13: **the spec names fields by path, and the path is
checked** — not the Rust code.

### The spec

```json
{ "service": "jellyfin", "base_url": "http://localhost:8096",
  "api_key_credential": "jellyfin-api-key", "task": "server-configuration",
  "desired": { "TrickplayOptions.EnableKeyFrameOnlyExtraction": true,
               "TrickplayOptions.EnableHwAcceleration": true } }
```

Three tasks, each over one object Jellyfin replaces as a whole:

| task | read | write | desired |
|---|---|---|---|
| `server-configuration` | `GET /System/Configuration` | `POST /System/Configuration` | path → value in `ServerConfiguration` |
| `library-options` | `GET /Library/VirtualFolders` | `POST /Library/VirtualFolders/LibraryOptions` (`{Id, LibraryOptions}`) | `libraries` (names) and `set`: path → value in `LibraryOptions` |
| `scheduled-task-triggers` | `GET /ScheduledTasks` | `POST /ScheduledTasks/{taskId}/Triggers` | `key_prefix` and `triggers`: the complete trigger list, compared on the keys the spec names |

The key travels as `X-Emby-Token`. Readiness is `GET /System/Info` with a
`Version`. Writes answer `204`.

### How a path is checked

- **At build time**, `converge schema-check --service jellyfin --openapi <file>
  --spec <spec>` walks every path through the named OpenAPI component
  (following `$ref` and the `allOf: [{$ref}]` Jellyfin uses), and checks the
  value against the property: JSON type, `nullable` for `null`, and **enum
  membership**. The last one is not decoration: on 2026-09-07 the host set a
  trigger type the API accepted silently and never fired; `"Type": "Daily"`
  now fails the build.
- **At runtime**, every path must exist in the object Jellyfin returned —
  intermediate objects included. A path the answer does not carry is an
  error, never a key added on the way back.
- **Plugin configurations** (a later stage) have no schema in the OpenAPI
  file; for them only the runtime check and recorded answers apply, and the
  documentation of that task says so.

### What stays the same

The engine: wait, read, diff, write only on difference, read back until the
desired values are there, deadline. The whole object goes back with only the
named fields changed; nothing else is touched. Libraries not named, tasks not
matching the prefix, are left alone. A library name or key prefix that
matches nothing is an error, not "nothing to do".

## 7. Plugin configurations and secrets (v0.4.0, 2026-09-13)

Jellyfin plugins keep their settings in objects the OpenAPI file does not
describe: `GET /Plugins/{pluginId}/Configuration` refers to a
`BasePluginConfiguration` with no properties, and the POST declares no body.
**For plugin fields there is no build-time check.** What remains, and what
this task relies on instead:

- **Identity is checked.** The spec names each plugin by id *and* name;
  `GET /Plugins` must list that id with exactly that name. A mistyped id or a
  plugin that is not installed fails before anything is read.
- **Every path must exist in the answer.** Measured on the host on
  2026-09-13: the JSON API returns empty strings as `""` — it is the XML file
  on disk that omits them — so a never-set key is present and can be set; a
  misspelt path is not, and fails.
- **Recorded answers** of seven plugins back the tests, masked on the host
  before they left it (`tests/fixtures/jellyfin-10.11.11/SOURCE.md`).

```json
{ "service": "jellyfin", "task": "plugin-configurations", "…": "…",
  "desired": {
    "c531afa3de204055aca5a7cc43adf783": {
      "name": "Jellyfin Oscars",
      "secrets": { "OmdbApiKey": "jellyfin-omdb-key" } },
    "a31b415a5264419db1528c8192a54994": {
      "name": "Mediathek Downloader",
      "set": { "Network.AllowUnknownDomains": false, "WizardCompleted": true } } } }
```

One spec holds all plugins of a unit, so one readiness probe and one
deadline cover them.

### Secrets

`secrets` maps a path to the name of a systemd credential. Every credential
is read before the first request; a missing one stops the run. The value is
held as `Secret`, compared with the field, and written only inside the
request body. **It never appears in a change, a note or an error** — a
differing secret is reported as `(hidden) -> (hidden) from its credential`,
and not even the *old* value is shown (it may be a key someone else set).
A path may not be both in `set` and in `secrets`.

## 8. Shared lists: entries by key (v0.5.0, 2026-09-13)

Some plugin settings are lists that more than one writer fills. The
JavaScript Injector keeps `CustomJavaScripts`, where the host registers one
script next to whatever a person adds in the web UI, and
`PluginJavaScripts`, which other plugins fill themselves. Replacing such a
list with `set` would take everyone else's entries away. `lists` names the
entries converge is responsible for and nothing more:

```json
"f5a34f7b2e8a4e6aa7223a216a81b374": {
  "name": "JavaScript Injector",
  "set": { "DisableScriptInjectionMiddleware": false },
  "lists": {
    "CustomJavaScripts": {
      "key": "Name",
      "items": [ { "Name": "Skin-Manager-Vorgabe", "Script": "…",
                   "Enabled": true, "RequiresAuthentication": true } ] } } }
```

- **An entry is found by its key** — the item's value of `key`, compared as
  JSON. The first matching entry counts if the service carries a key twice.
- **A found entry gets the item's fields**; its other fields stay. A field
  the entry does not carry is an error, as for any path.
- **A missing entry is appended** after all others, as the item says it.
- **No entry is ever removed, and none moves.** Taking a script out is done
  by setting `Enabled` to `false`, which is a field like any other.
- **Changes name the entry**: `CustomJavaScripts[Name=Skin-Manager-Vorgabe].Enabled
  true -> false`, or `(missing) -> (added)`. Values longer than 80
  characters are cut to their beginning and their length — a script says
  nothing more in full.
- The spec is checked before anything is read: a dotted path, a non-empty
  key that every item carries as a string, no key twice, at least one field
  besides the key, and no path that is also in `set` or `secrets`.

The recorded SkinManager and JavaScript Injector configurations back the
tests (`tests/fixtures/jellyfin-10.11.11/SOURCE.md`).

## 9. Trailarr: connections and trailer profiles (v0.6.0, 2026-09-13)

Trailarr (0.11.5) downloads trailers for what Radarr and Sonarr hold. The
host configured it with a shell unit that posted two connections and set a
handful of fields on every trailer profile. Both become tasks:

| task | read | write | desired |
|---|---|---|---|
| `connections` | `GET /api/v1/connections/` | `POST /api/v1/connections/` (missing), `PUT /api/v1/connections/{connection_id}` (differing) | `connections`: name → `set` (top-level fields) and `api_key_credential` |
| `trailer-profiles` | `GET /api/v1/trailerprofiles/` | `POST /api/v1/trailerprofiles/{trailerprofile_id}/setting`, one `{key, value}` per field | `set`: fields every profile gets |

The key travels as `X-API-KEY`. Readiness is `GET /api/v1/settings/` with a
`version`; that answer also carries Trailarr's own key and the web UI's
password, so only `version` is decoded — the rest is never held.

```json
{ "service": "trailarr", "task": "connections", "…": "…",
  "desired": { "connections": {
    "Radarr": { "set": { "arr_type": "radarr", "url": "http://127.0.0.1:7878",
                         "monitor_new_media": true, "external_url": "",
                         "path_mappings": [] },
                "api_key_credential": "radarr-api-key" } } } }
```

### Why level 3 matters here

Trailarr is a FastAPI application: pydantic models validate the body, and a
field the model does not have is **dropped without a word**. The host's shell
unit sent `monitor` — a field Trailarr 0.11.5 does not have; its
`ConnectionCreate` has `monitor_new_media`. The request was accepted every
time, and whatever the unit meant to set, `monitor_new_media` kept its
default. This is exactly what the schema check catches:
`schema-check --service trailarr --spec` walks every `set` field through
**both** `ConnectionCreate` and `ConnectionUpdate` (a connection's fields are
sent when it is added and when it is updated), checks the JSON type, and
checks `arr_type` against the `ArrType` enum. Trailer profile fields are
checked against `TrailerProfileRead` the same way.

The description is OpenAPI 3.1 and writes a nullable property as
`anyOf: [<schema>, {type: null}]`, not `nullable: true`; the check reads
both. `UpdateSetting.value` is `anyOf: [integer, string, boolean]`, and the
wire type's untagged enum must name the same three types.

### Connections

- **Found by name.** A connection the spec names and the service lacks is
  `connection Radarr: (missing) -> (added)` and is created with
  `ConnectionCreate`: `name`, `api_key`, `path_mappings` (the spec's, or
  empty) and the `set` fields. To add one, `set` must name `arr_type` and
  `url` — Trailarr has no default for them — or the run fails before writing.
- **An existing connection** gets one change per differing `set` field; if
  any differs, one `PUT` with `name`, `api_key`, `path_mappings` and the
  `set` fields. `path_mappings` is required in `ConnectionUpdate`: the spec's
  list if it names one, otherwise the connection's own, so an update never
  empties it by accident.
- **The key is compared with its credential and never shown**:
  `api_key (hidden) -> (hidden) from its credential`, or `(empty)` if the
  service holds none. Trailarr returns the keys of Radarr and Sonarr in the
  clear; the wire type decodes them straight into `Secret`.
- `name`, `api_key`, `id` and `added_at` cannot be in `set`. A `set` field the
  answer does not carry is an error.
- **An empty list is a valid answer** — unlike everywhere else. A fresh
  Trailarr has no connections, and every connection the spec names then shows
  up as a change, so the comparison is never over an empty set.
- Connections the spec does not name are a note (`not in the spec:
  connection Lidarr`) and are never touched. Nothing is deleted.
- Writes answer `201` with a string; `200` is accepted too.

### Trailer profiles

- **Every profile gets the fields.** An empty list is an error, as usual.
- One change per profile and differing field (`trailer profile 2:
  always_search false -> true`); long values are cut as in §8.
- One `POST …/setting` per differing field, because that endpoint sets one
  setting. The host's shell unit saw it confirm before the value was
  visible; the engine reads back until it is.
- `id`, `customfilter` and `customfilter_id` cannot be set this way; values
  are strings, booleans or integers, which is what `UpdateSetting` takes.

The recorded answers and the OpenAPI file come from the same running
container (`tests/fixtures/trailarr-0.11.5/SOURCE.md`, `openapi/SOURCE.md`).

## 10. ntfy: account subscriptions with secret topics (v0.6.0, 2026-09-13)

An ntfy account keeps a list of subscriptions, which ntfy's web app loads
after logging in. The host declares which topics an account is subscribed
to. **A topic name is all it takes to read a topic**, so topics
are secrets in this task, held to the same rules as keys:

```json
{ "service": "ntfy", "base_url": "http://localhost:2586",
  "api_key_credential": "ntfy-token", "task": "account-subscriptions",
  "desired": { "base_url": "https://ntfy.rusty-vault.de",
               "topics_credential": "ntfy-abo-topics" } }
```

- The **credential** named in `topics_credential` holds one topic per line;
  blank lines and surrounding whitespace are ignored; at least one topic, none
  twice. It is read before the first request.
- **A topic never appears in output** — not in a change, a note, an error or
  a `Debug` dump. It is named by its number among the credential's non-empty
  lines: `ntfy account: subscription 3 (missing) -> (added)`. Topics are
  `Secret`s from the credential file to the request body, answers included.
- The token travels as `Authorization: Bearer <token>`; the credential holds
  the bare token (`Service::key_value` adds the prefix for ntfy only).
- **Readiness** is `GET /v1/health` answering `{"healthy": true}`. ntfy tells
  its version to administrators only, so the version line says
  `(not reported)`. The health endpoint needs no token, so a refused token
  fails at the first read (`GET /v1/account`, 401), not while waiting.
- **Read** `GET /v1/account`, of which only `subscriptions` (`base_url`,
  `topic`) is decoded; the account's tokens, sync topic and user name are
  never held. A decoding error says where it failed, not what it found.
  ntfy 2.26.0 **omits `subscriptions` when there are none**
  (`json:"subscriptions,omitempty"` in `server/types.go`), so a missing key
  is an empty list — otherwise an account could never get its first one.
  `username` must be present instead (only its presence is checked), which
  tells an account from any other JSON object.
- **A subscription counts** if one has the desired `base_url` and the topic.
  The same topic under another `base_url` is still a change, plus a note
  naming that `base_url`, which is not secret.
- **Write** `POST /v1/account/subscription` with `{base_url, topic}`, one per
  missing topic. Never `PATCH`, never `DELETE`, and `display_name` — the
  person's own label — is neither read nor written.
- **Error bodies are not shown at all** for this task: ntfy's messages may
  quote the request, and the request holds a topic.

### No OpenAPI description

ntfy publishes none. As for Jellyfin's plugin configurations (§7), the fields
are checked by a recorded answer (`tests/fixtures/ntfy-2.26.0/`) and at
runtime only. `schema-check --service ntfy` therefore takes no `--openapi`
— passing one is an error rather than silently ignored, so a build cannot
believe it checked something it did not — and only validates the given specs.
Every other service still requires `--openapi`.

### Considered and rejected: Grafana

Grafana's organisation roles were considered for the same release and left
out. Grafana re-syncs a user's org role from OAuth on **every login**: with no
`role_attribute_path` configured it assigns `auto_assign_org_role`, so a role
converge wrote would be overwritten at the next login and written again at the
next timer run — two writers flip-flopping over one field. The fix belongs in
Grafana's OAuth configuration, not in converge.

## 11. Servarr: naming, media management, root folders (v0.7.0, 2026-09-13)

Radarr, Sonarr and Lidarr are one code base (Servarr) behind two API
versions: v3 for Radarr and Sonarr, v1 for Lidarr. Their configuration
documents and root folders have the same shape in all three. On the host
they were set by OpenTofu resources that ran only when someone typed
`tofu apply`; the three tasks replace those resources.

| task | services | read | write | desired |
|---|---|---|---|---|
| `naming` | Radarr, Sonarr, Lidarr | `GET /api/vN/config/naming` | `PUT /api/vN/config/naming/{id}` | top-level fields of `NamingConfigResource` |
| `media-management` | Radarr, Sonarr, Lidarr | `GET /api/vN/config/mediamanagement` | `PUT …/mediamanagement/{id}` | top-level fields of `MediaManagementConfigResource` |
| `root-folders` | Radarr, Sonarr, Lidarr | `GET /api/vN/rootfolder` | `POST /api/vN/rootfolder` (missing), `PUT /api/v1/rootfolder/{id}` (Lidarr, differing) | folders by path, with fields and profiles by name |

### Documents

- **A document is one object with an integer `id`.** An answer without one,
  or not an object, is an error.
- The spec names **plain top-level field names**; `id` cannot be set. Every
  name must be in the answer, and at build time a property of the component
  with a matching type and enum value -- Radarr's `colonReplacementFormat`
  is an enum of strings, Sonarr's of integers, and both are checked as such.
- The write sends **the whole document back** with only the named fields
  changed and accepts `200` and `202`; the engine reads back.
- The names are the API's, not the ones a Terraform provider used for the
  same setting. The host's provider called one field `copy_using_hardlinks`
  for Radarr and `hardlinks_copy` for Sonarr and Lidarr; the API calls it
  `copyUsingHardlinks` everywhere, and `hardlinks_copy` fails the schema
  check.

### Root folders

```json
{ "service": "lidarr", "task": "root-folders", "…": "…",
  "desired": { "folders": {
    "/tank/data/media/music": {
      "set": { "name": "Musik", "defaultMonitorOption": "all" },
      "profiles": { "defaultQualityProfileId": "Standard",
                    "defaultMetadataProfileId": "Standard" } } } } }
```

- **Found by path**, compared as a string. A path must be absolute and
  without a trailing slash.
- **A missing folder is added** with `path`, the `set` fields and the
  profiles as ids: `root folder /tank/data/media/music: (missing) -> (added)`.
- **An existing folder** gets one change per differing field. Only Lidarr's
  API can update a root folder; Radarr's and Sonarr's folders are a path and
  nothing else, so their specs may name neither `set` nor `profiles`, and
  the parser says so.
- **Profiles are given by name.** Lidarr's root folder carries default
  quality and metadata profiles as ids. An id in the spec would be the wrong
  one, silently, after the database is rebuilt; the name is looked up in
  `GET /api/v1/qualityprofile` and `/metadataprofile`, and a name that
  matches no profile is an error naming it. The lists are only read when the
  spec names a profile, and an empty list is an error.
- `id`, `path`, `accessible`, `freeSpace`, `totalSpace` and
  `unmappedFolders` are the service's and cannot be in `set`.
- **An empty folder list is a valid answer** (a fresh service has none);
  folders the spec does not name are a note. Nothing is removed.
- At build time `path`, every `set` field and every profile field are
  checked as properties of `RootFolderResource`.

Recorded answers: `tests/fixtures/{radarr,sonarr}-*/{naming,mediamanagement,rootfolder}.json`
and `tests/fixtures/lidarr-3.1.0.4875/`. Before release, `plan` ran on the
host against all three services with the specs the host deploys, and
reported `unchanged` for all eight.

## 12. Servarr providers and hidden values (v0.8.0, 2026-09-13)

Download clients, notifications and Prowlarr's applications are Servarr
*providers*: a resource with top-level fields (`enable`, `priority`,
`onDownload`, `syncLevel`, …) and a `fields` list of
`{name, value, privacy, …}` entries whose names depend on the
implementation. Prowlarr (API v1) joins as a service for them.

| task | services | read | write |
|---|---|---|---|
| `download-clients` | Radarr, Sonarr, Lidarr, Prowlarr | `GET …/downloadclient` | `POST …/downloadclient?forceSave=true` (missing), `PUT …/downloadclient/{id}?forceSave=true` |
| `notifications` | Radarr, Sonarr, Lidarr, Prowlarr | `GET …/notification` | the same under `notification` |
| `applications` | Prowlarr | `GET /api/v1/applications` | the same under `applications` |

```json
{ "service": "radarr", "task": "download-clients", "…": "…",
  "desired": { "providers": {
    "qBittorrent": {
      "implementation": "QBittorrent",
      "set": { "enable": true, "priority": 1 },
      "fields": { "host": "10.0.10.11", "port": 8080, "username": "admin",
                  "movieCategory": "radarr" },
      "secret_fields": { "password": "qbittorrent-webui-password" } } } } }
```

### What the service hides

Measured on the host on 2026-09-13 against Radarr 6.3.0.10514, Sonarr
4.0.19.2979, Lidarr 3.1.0.4875 and Prowlarr 2.5.2.5491: every field with
`privacy` `password` or `apiKey` is answered as `********`; `userName` is
answered in the clear. The source says why and what follows:

- `SchemaBuilder.ReadFromSchema`: a field that comes back as `********` keeps
  the stored value. So a whole resource can be sent back as read without
  touching a secret -- and **a stale secret cannot be seen by reading.**
- `ProviderControllerBase.UpdateProvider` in Radarr, Sonarr and Lidarr writes,
  and tests the connection, only if the definition changed (memberwise
  `Equals` of the settings). **Prowlarr writes on every update** and tests
  unless `forceSave` is set, and an update of an application starts a full
  indexer sync to it (`ApplicationService`).

Decided with the host's owner: secrets are **handed over on every `apply`**,
not fingerprinted. converge keeps no state; Radarr, Sonarr and Lidarr compare
for themselves, and a secret changed in the web UI is set back as well. The
price is a write and an indexer sync per Prowlarr application per run, which
at a daily timer is nothing Prowlarr does not do on its own.

### The engine: `hand_over`

`Task` gains `hand_over(transport, current) -> lines`, empty by default. The
engine calls it in `apply` only: after `unchanged`, or after the read-back
of a write (so a provider just added gets its secret too). `plan` never
calls it. Each line names what was handed over, never its value:
`download client qBittorrent: password handed over from credentials (hidden;
the service compares)`. There is no read-back for a hidden value; Prowlarr's
own connection test against the stored value (HTTP 200) was the acceptance.

### Providers

- **Found by name.** A provider the spec names and the service lacks is added
  from the template `GET …/schema` returns for its `implementation`: the
  template with the name, the `set` fields, the `fields` values and the
  secrets, without `id`. The templates are read only when something is
  missing; an empty template list is an error.
- **An existing provider** must have the spec's implementation; converge does
  not change one. Every `set` key must be in the answer, every `fields` and
  `secret_fields` name in its `fields` list, and (v0.8.0) every secret field had to be
  one the service hides; since v0.9.0 a shown one is compared instead
  (§13). A `fields` entry without `value` is `null`.
- **Visible differences** are changes (`fields.port 8080 -> 8081`); the write
  sends the entry as read with the spec's values and the secrets.
- **Every write sends `forceSave=true`.** No connection test runs: a test
  against a download client with a stale password is a failed login, and
  qBittorrent bans the address after a few (the host, 2026-09-12).
- Writes accept `200`, `201` and `202`. Providers the spec does not name are a
  note; nothing is deleted.
- At build time only the `set` fields are checked, against
  `DownloadClientResource`, `NotificationResource` or `ApplicationResource`.
  `Field.value` has no type, so the `fields` entries are checked at runtime
  and against the recorded answers. Prowlarr 2.5.2's description lacks
  `enable` on `ApplicationResource`, although the service answers with it; a
  spec cannot set it.
- A request body that holds a secret is never printed; a failed serialization
  names the path only.

Recorded answers: `downloadclient*.json` and `notification*.json` in the
Radarr, Sonarr and Lidarr fixture directories, `tests/fixtures/prowlarr-2.5.2.5491/`,
masked on the host (`userName` values). Before release, `plan` ran on the
host with the eight provider specs the host deploys (`unchanged` for all),
and one `apply` handed Prowlarr's SABnzbd key over: the list stayed
byte-identical and Prowlarr's connection test with the stored key answered
200.

## 13. Prowlarr's indexers, indexer proxies, tags and shown secrets (v0.9.0, 2026-09-13)

Prowlarr's indexers and indexer proxies are providers too, and the host
needed them for the unit that replaced a 415-line shell script:

| task | services | read | write |
|---|---|---|---|
| `indexers` | Prowlarr | `GET /api/v1/indexer` | `POST`/`PUT …/indexer?forceSave=true` |
| `indexer-proxies` | Prowlarr | `GET /api/v1/indexerproxy` | the same under `indexerproxy` |

```json
{ "service": "prowlarr", "task": "indexers", "…": "…",
  "desired": { "providers": {
    "TNTracker": {
      "implementation": "Torznab", "template": "Torrent Network",
      "set": { "enable": true, "appProfileId": 1, "priority": 25 },
      "fields": { "baseUrl": "http://tntracker.org" },
      "secret_fields": { "apiKey": "tntracker-apikey" },
      "tags": ["umlautadaptarr"] } } } }
```

### Templates by name

`GET /api/v1/indexer/schema` answered 627 templates on 2026-09-13, most of
them `Cardigann` definitions: the implementation does not pick one. A
provider may name its `template`; a missing provider is added from the one
template with that name and the spec's implementation. None is an error, and
so are two (the recording has `FunFile` twice, once as `Cardigann` and once
as `FunFile`; the implementation tells those apart). Without `template` the
implementation picks, as before. An existing provider's template is not
checked: Prowlarr does not answer which one it came from.

### Tags by label

`tags` is a list of labels. Tag ids are the service's own and differ between
installations, so a spec names labels; converge reads `GET …/tag` only when a
provider names tags, adds a label the service lacks (`POST …/tag`) before
writing any provider, and sends the ids. A difference is shown as labels
(`tags ["vpn"] -> ["umlautadaptarr"]`; an id the service does not list shows
as `#id`). Servarr stores labels in lower case, so a spec's labels must be
lower case. `tags: []` is a statement -- the provider has no tag --; leaving
`tags` out leaves them alone.

### Secrets the service shows

Measured on the host on 2026-09-13 against Prowlarr 2.5.2.5491: a Cardigann
indexer's `username` and `password` and MyAnonamouse's `mamId` have `privacy
normal` and are answered in the clear; the Torznab and Newznab `apiKey`
fields are `********`. A stored credential is a secret either way, and
putting it into `fields` would put it into a world-readable spec. So:

- a `secret_fields` entry whose field the service **hides** is handed over on
  every `apply`, as in §12;
- one whose field the service **shows** is compared like a field. A
  difference is a change that names neither value
  (`fields.password (another value, not shown) -> (the credential's, not shown)`),
  and the write carries the credential. It is not handed over.

The recorded answers are masked accordingly: besides the §12 rule, every
`fields` entry whose name looks like a credential (`user`, `pass`, `key`,
`token`, `cookie`, `mamid`, …) and holds a string other than `********` or
`""` became `"<masked>"` on the host. Tests hold `<masked>` as those
credentials, and a stale password (`pw-7f3a9c-never-print-me`) must appear
in the request body and in no change, note or hand-over line; a sabotage
printing the credential or skipping the comparison turns two tests red.

The build checks `set` against `IndexerResource` and `IndexerProxyResource`,
and the tag endpoints against `TagResource`, for every Servarr service.

Before release, `plan` ran on the host with the two specs it deploys, the
real credentials loaded: `unchanged` for both -- the proxies, the tags and
the shown tracker credentials matched.

A spec cannot say "write the indexers only if the proxies are there": a
failing spec does not stop the next one (`src/main.rs`). The host runs the two specs as
two `ExecStart=` lines, and systemd stops at the first that fails.

## 14. Not in the pilot

- Other services and tasks (Authentik, Seerr, …).
- TLS, JSON output, a NixOS module.
- Deleting things. `converge` only sets what the spec names, and appends
  list entries (§8), connections (§9), subscriptions (§10), root folders (§11), providers (§12, §13) and tags (§13) it is
  responsible for.
