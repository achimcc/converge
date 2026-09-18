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

## 14. bindery: download clients, Prowlarr instances, root folders, settings (v0.10.0, 2026-09-13)

bindery (1.33) publishes no OpenAPI description. As for ntfy (§10),
`schema-check --service bindery` loads and validates the specs and nothing
more; field names are checked against the answer at runtime and against the
recorded answers in `tests/fixtures/bindery-1.33.2/`.

| task | read | write |
|---|---|---|
| `download-clients` | `GET /api/v1/downloadclient` | `POST` (missing), `PUT …/{id}` |
| `prowlarr-instances` | `GET /api/v1/prowlarr` | the same under `prowlarr` |
| `root-folders` | `GET /api/v1/rootfolder` | `POST` (missing); bindery has no update |
| `settings` | `GET /api/v1/setting/{key}` | `PUT …/{key}` with `{value}` |

```json
{ "service": "bindery", "task": "download-clients", "…": "…",
  "desired": { "clients": {
    "sabnzbd": { "set": { "type": "sabnzbd", "host": "10.0.10.10", "port": 8080 },
                 "secret_fields": { "apiKey": "sabnzbd-api-key" } } } } }
```

Readiness is `GET /api/v1/health`: `status: ok` and a version.

### Write-only secrets

Measured on the host on 2026-09-13 and read in bindery's source: `apiKey` and
`password` are answered as `""`, next to `apiKeyConfigured` and
`passwordConfigured`. An update decodes the body **over the stored row**, so a
key left out keeps its value, and an empty secret means "keep the stored one"
(`applyDownloadClientCredentials`, `resolveWriteOnlyAPIKey`); only an explicit
`clearApiKey` / `clearPassword` removes one, and converge never sends it. A
Prowlarr instance whose key actually changes passes it on to every indexer
synced from it.

So a secret is handed over on every `apply` with a `PUT` that carries **only
the secret fields** -- nothing else of the row is touched, and bindery evicts
its pooled client so the new value is used at once. A stale value cannot be
read; a secret that is not stored at all (its flag false) is a change. A
`secret_fields` entry must be `apiKey` or `password`, and one bindery answers
with a value is an error rather than a comparison.

This is the gap the host's shell unit had: it added the SABnzbd client and
the Prowlarr instance only when they were missing, so a rotated key never
reached bindery (the same class as the qBittorrent rotation on 2026-09-12).

Before release, `plan` ran on the host with the four specs it deploys and the
real credentials: `unchanged` for all four. Sabotages that print the secret in
a hand-over line or send the whole row instead of only the secret turn two
tests red.

## 15. Jellyfin named configurations (v0.11.0, 2026-09-14)

Jellyfin keeps more than `ServerConfiguration`: named documents under
`/System/Configuration/{key}`. The host needs two of them. `network` carries
`KnownProxies` — empty on the host although a reverse proxy sits in front, so
Jellyfin saw every client as the proxy's address — and `EnableUPnP`, which the
host set once during setup and nobody holds since. `branding` carries the
sign-in button in `LoginDisclaimer`, which another tool rewrote on every run
without ever converging.

| task | read | write | desired |
|---|---|---|---|
| `named-configuration` | `GET /System/Configuration/{key}` | `POST /System/Configuration/{key}` | `key` and `set`: path → value |

The same engine and the same rules as `server-configuration` (§6): the whole
document goes back with only the named fields changed, a path the answer does
not carry is an error.

### Why the key is an enum

The generic endpoint declares its answer as `string (binary)` and its body
without a schema: there is nothing to check a path against. So `key` accepts
only keys whose document the description names, and each maps to a component:

| key | component |
|---|---|
| `network` | `NetworkConfiguration` |
| `branding` | `BrandingOptionsDto` |

For branding it is the DTO and not the stored `BrandingOptions`: the POST
lands on Jellyfin's dedicated `/System/Configuration/Branding` (route matching
ignores case), which accepts `BrandingOptionsDto` and leaves out
`SplashscreenLocation`. A key outside the table is a spec error; adding one is
a release with its component, not a string in a spec.

### Null means absent

Jellyfin omits null values. The recorded branding answer has no `CustomCss` at
all, so a spec naming it fails at runtime with "the answer has no such field"
— the same rule as everywhere (a missing path is never added), and on this
host the field is not set.

## 16. Library ids by name (v0.12.0, 2026-09-14)

Jellyfin's sign-in plugins decide which libraries an account sees, and they
name libraries by **id**: LDAP-Auth's `EnabledFolders`, SSO-Auth's
`OidConfigs.<provider>.EnabledFolders` and the `Folders` of each entry in
`OidConfigs.<provider>.FolderRoleMapping`. An id exists only once the
library does, and a rebuilt host gets new ones — a spec cannot carry them.
The host's shell unit looked them up in `/Library/VirtualFolders` and pasted
them into an XML file.

In a plugin's `set`, a value may therefore hold a marker wherever an id list
belongs, at any depth:

```json
"505ce9d1d91642fa86ca673ef241d7df": {
  "name": "SSO-Auth",
  "set": {
    "OidConfigs.authentik.EnabledFolders": { "$library_ids": ["Filme", "Serien"] },
    "OidConfigs.authentik.FolderRoleMapping": [
      { "Role": "Medien",  "Folders": { "$library_ids": ["Filme", "Serien"] } },
      { "Role": "Privat",  "Folders": { "$library_ids": ["Privat"] } } ] },
  "secrets": { "OidConfigs.authentik.OidSecret": "jellyfin-oidc-secret" } }
```

- **Resolved on every read**, from `GET /Library/VirtualFolders`, to the list
  of `ItemId`s **in the order named**; the diff, the change lines and the
  write see only ids. A spec without a marker never asks for the libraries.
- **A name the service does not know is an error before anything is
  written**, and so is a name two libraries carry; so is an empty library
  list. Leaving an unknown library out would take it away from everyone the
  list grants it to.
- **The spec is checked first**: `$library_ids` stands alone in its object and
  lists distinct, non-empty names. Any other key starting with `$` is a
  misspelt marker and an error. Markers work in `set` only, not in `lists`
  items.
- The list is compared as a whole, like any other value: the same ids in
  another order are a change.

### What the recorded answers showed

`OidConfigs` is a JSON object keyed by provider name, and the role mapping is
`FolderRoleMapping` (the property name). The XML file on disk calls it
`FolderRoleMappings`, twice nested — a spec written from the file would fail
at runtime with "the answer has no such field", which is the point of that
rule. Both plugins read their configuration from the plugin instance on every
request, and Jellyfin's `POST /Plugins/{id}/Configuration` replaces that
instance's configuration, so no restart is needed after a write (read in the
sources of LDAP-Auth 23 and SSO-Auth 4.0.0.4).

SSO-Auth writes `CanonicalLinks` itself when someone signs in. converge sends
the object back as read, so they stay — unless a sign-in lands between the
read and the write, which a whole-object API cannot rule out.

A provider missing from `OidConfigs` is a missing path like any other:
converge does not create it. Creating it is a bootstrap step for the host
(SSO-Auth's own `POST /sso/OID/Add/{provider}`).

## 17. Seerr: main, Jellyfin and libraries, Radarr/Sonarr entries, webhook (v0.13.0, 2026-09-14)

Seerr (3.2) keeps its settings in `settings.json` and changes them through
`/api/v1/settings/...`. The host wrote that file with `jq` before every start
— looking up ids in Radarr, Sonarr and Jellyfin itself — and restarted Seerr
to pick a rotated key up. Four tasks replace the part that is not bootstrap:

| task | read | write | desired |
|---|---|---|---|
| `main` | `GET /settings/main` | `POST /settings/main` (merges) | `set`: top-level fields, not `apiKey` |
| `jellyfin` | `GET /settings/jellyfin` | `POST /settings/jellyfin` (tests the connection, fills `serverId` and `name`); `GET /settings/jellyfin/library?enable=<ids>` | `set`, `api_key_credential`, `libraries`: names — exactly these are enabled |
| `radarr-servers`, `sonarr-servers` | `GET /settings/{kind}` | `POST` (missing), `PUT /settings/{kind}/{id}` (replaces the entry) | `servers`: name → `set`, `api_key_credential`, `profile`, `root_folder` |
| `webhook` | `GET /settings/notifications/webhook` | `POST` (replaces) | `set` by path, `payload` (object), `headers`: name → credential |

Readiness: `GET /api/v1/status` answers a version without a key.

### The key is a person

`X-Api-Key` does not authenticate on its own: `middleware/auth.js` looks up
**user 1** for it, the administrator the first sign-in creates. Until then
every settings route answers 403 — so the probe treats 403 as fatal, not as
"not yet". And that first sign-in (`routes/auth.js`) needs `jellyfin.ip`,
`port` and `apiKey` in the file already, then fills `serverId`, `name` and a
fresh Jellyfin token itself. Five fields therefore stay a bootstrap the host
writes before the first start, only when missing; everything else is
converge's after the first sign-in.

### What the description gets wrong

Seerr ships `seerr-api.yml`, and its components misname what the running
service answers: `JellyfinSettings` has `hostname` and `serverID` where the
answer carries `ip`, `port`, `useSsl`, `urlBase`, `serverId`, `apiKey`;
`MainSettings` lacks `locale`, `discoverRegion`, `streamingRegion`,
`cacheImages`; `WebhookSettings` lacks `embedPoster`. Only the Radarr and
Sonarr components match. A build check against that file would reject
fields that exist and pass ones that do not, so `schema-check --service
seerr` validates the specs and nothing more, as for ntfy and bindery, and
the recorded answers in `tests/fixtures/seerr-3.2.0/` carry the names.

### Names, resolved by Seerr itself

`activeProfileId` is a Radarr number and `activeDirectory` a Radarr path;
the libraries are Jellyfin ids. None of them belongs in a spec (§16), and
converge does not talk to a third service to find them: Seerr's own
`POST /settings/{kind}/test` asks Radarr or Sonarr with the entry's
connection fields and answers `profiles` and `rootFolders` without storing
anything — it runs on every read, so a profile renamed in Radarr is an error
that names what the service has, before anything is compared. (Sonarr has no
`/{id}/profiles` route; the test serves both.) For the libraries,
`GET /settings/jellyfin/library?sync=true&enable=<ids>` makes Seerr fetch
the list from Jellyfin with its own type filter; a name still unknown
afterwards is an error.

**The route without `enable=` disables every library** — its last line runs
unconditionally. `library_query` always carries the parameter, a test holds
it there, and the spec refuses an empty list.

### Secrets Seerr shows

`GET /settings/radarr` answers `apiKey` in the clear, as do `jellyfin` and
the webhook's `customHeaders[].value`. They are compared against the
credential and never printed (§13): a difference is `(hidden) -> (hidden)
from its credential`. `PUT` replaces the whole entry, so the entry as read
goes back with the named fields changed — a field the spec does not name
(Sonarr's `activeLanguageProfileId`, say) survives. Only the `id` stays
behind: it is the path, and Seerr 3.2 validates the body against its OpenAPI
file, where `id` is `readOnly` — a body carrying it is answered `400
request.body.id is read-only` (measured 2026-09-15, the same body without it
200). A run that writes nothing never sends the `PUT`, so this stayed hidden
until a key was rotated (v0.15.2).

### The payload, encoded twice

The webhook agent decodes `jsonPayload` from base64 and then parses it
**twice** (`JSON.parse(JSON.parse(text))`); the route stores
`base64(jsonPayload)` of the request field after checking it parses once.
Seerr's own web UI sends the template as JSON text, so what it stores parses
once and the agent fails on it — silently, into the log. converge sends the
template's JSON as a JSON string literal, which is exactly what the agent
wants; `GET` answers the inner text as a string (recorded: a string equal to
the host's template), or an object when the UI stored it, which is reported
as a change. The template itself is an object in the spec.

Before release, `plan` ran on the host with its five specs and the real
credentials: `unchanged` for all five.

## 18. Koel: radio stations (v0.14.0, 2026-09-14)

Koel (9.11) keeps two global settings in its database — the media path and
the branding — and a list of radio stations everybody sees. The media path
starts a full scan inside the request; it stays a bootstrap of the host. The
branding was left out on purpose. Radio stations are the task:

| task | read | write | desired |
|---|---|---|---|
| `radio-stations` | `GET /api/me` (`include_public_media` must be off), `GET /api/radio/stations` | `POST /api/radio/stations` (missing, 201), `PUT /api/radio/stations/{id}` (differing, 200) | `stations`: a list of `name`, `url`, `description`, `is_public`, `homepage_url`, `logo_file` |

```json
{
  "service": "koel",
  "base_url": "http://10.0.254.10",
  "api_key_credential": "koel-token",
  "task": "radio-stations",
  "desired": { "stations": [
    { "name": "Radio Dreyeckland", "url": "https://stream.rdl.de/rdl",
      "description": "Free radio from Freiburg.", "is_public": true,
      "homepage_url": "https://rdl.de/", "logo_file": "/nix/store/…-rdl.png" } ] }
}
```

### The token and `Accept`

Koel's API authenticates with a Sanctum token as `Authorization: Bearer`, the
same header ntfy takes (§10); the credential holds the bare token. Tokens do
not expire, so the host creates one per run for the account the stations
belong to and deletes it afterwards — that is the host's business, converge
only reads the credential.

**Every request says `Accept: application/json`.** Without it Laravel
answers a request without a valid token not with 401 but with a **302 to the
web page** (measured), and the agent follows redirects into an HTML answer
with HTTP 200 — an "empty" success. `HttpTransport::accept_json` adds the
header for Koel only. On top of it an answer that is not JSON is an error that
says so, in the probe (fatal: waiting does not turn a web page into a list)
and in the read. `[]` parsed from JSON is a valid list; an empty body is not.

Readiness is the list itself: `GET /api/ping` needs no token and answers an
empty body, so it would prove nothing about the token. 401 and 403 are
fatal. Koel reports its version only in `GET /api/data`, which creates a
queue row for the account on its first call; the version is "not reported",
as for ntfy.

Laravel's validation errors (`{"message": …, "errors": {field: [messages]}}`)
join Servarr's shape in `validation_messages`: the fields and their messages
are shown, the summary and everything else in the body are not.

### Found by name

A station is found by `name`. Koel's own uniqueness is the URL per account,
but stream URLs change; keyed by URL, a new URL would add a second station and
leave the old one. The spec refuses a name or a URL given twice.

### Only the account's own stations (v0.14.1)

**The guarantee: converge writes only to stations the token's account owns,
and adds stations only for that account.** It holds because of what the list
contains, not because of anything a station carries:

- `GET /api/radio/stations` answers the account's own stations and — with
  the account's preference `include_public_media` on, Koel's default — every
  **public station of every account in the organization**
  (`RadioStationBuilder::accessible`). With the preference off it is
  `whereBelongsTo` the account: its own stations and nothing else.
- The answer names **no owner** (`RadioStationResource`: no `user_id`,
  `user` or `owner`), and `permissions.edit` cannot stand in for one: an
  administrator — the account the host uses — may edit every station of the
  organization (`RadioStationPolicy::edit`, `MANAGE_RADIO_STATIONS`), so it
  is `true` everywhere. Comparing an owner id with `GET /api/me` is
  therefore impossible.
- So before every read of the list (`plan`, `apply` and each read-back)
  converge reads `GET /api/me` and **refuses** unless
  `preferences.include_public_media` is `false` — a boolean it must find, or
  the run fails naming the field. v0.14.0 did not: a spec station named like
  one person's public station would have been matched and `PUT` onto that
  person's station, silently (fixed in v0.14.1; the recorded account had the
  default `true`). The host sets the preference for its account once
  (`PATCH /api/me/preferences`, or in its own setup).

`GET /api/me` also answers the account's Subsonic key and e-mail address. Only
the one field is read; a decode error names line and column, never text.

**Ambiguity** is left only among the account's own stations: two of its
stations with one name (created by hand) make the task fail before anything
is written, since nothing tells converge which one is meant.

What the guarantee does not cover: someone switching the preference on
between converge's read and its write (a window of one run), and a person
editing or deleting the account's stations through Koel — the next run
writes the spec back or adds the station again (with a new id).

### Written whole

**`PUT` carries the whole entry.** `RadioStationUpdateRequest` reads
`is_public` with `boolean()` (absent is `false`), `description` with
`string()` (absent is `''`) and `homepage_url` as given (absent is `null`): a
body with only the changed field would make the station private and wipe
its description. So every write sends `name`, `url`, `description`,
`is_public` and `homepage_url` from the spec, and the spec requires
`is_public`. A `description` Koel answers as `null` equals `""` — its update
turns one into the other.

Every `POST` and `PUT` makes Koel fetch the stream and demand an `audio/*`
content type (`HasAudioContentType`): a playlist link or a station that is
down fails the write with 422 — the run that would change it, not the daily
`unchanged` one.

### The logo only where there is none

Koel takes a logo as base64 image data (`ValidImageData`), not as a URL, and
stores it re-encoded under a random name; the answer carries a URL to that
file. Nothing can be compared. `logo_file` names an image file (the host
fetches it at build time), which converge reads before the first request,
types by its content (PNG, JPEG, GIF, WebP, SVG — SVG needs its own type to
reach Koel's sanitizer) and sends as a `data:` URI **when the station is
added, or when it has no logo**. A changed file does not replace a logo that
is there; a changed line says `logo (none) -> (from <file>)` and never shows
the data. On `PUT` without a logo change the field is left out, which Koel
reads as "keep".

A logo file above **2 MiB** is refused before it is read, naming its size.
Koel scales a logo down to 640 pixels wide (`ImageWritingConfig`), so a
larger file buys nothing, and as a `data:` URI it grows by a third: 2 MiB of
file is about 2.7 MiB of body — inside PHP's stock `post_max_size` (8 MiB)
and nginx's NixOS default `client_max_body_size` (10 MiB), which a Koel host
may keep.

### No deletion, no description

A station removed from the spec stays, like every other entry (§19);
removing the account's own stations that the spec no longer names is the
host's job, where the owner is known — the API answer does not name one.
`api-docs/api.yaml` in Koel's package still says 5.1.0 and knows neither
radio stations nor this route, so `schema-check --service koel` validates
the specs only, as for ntfy, bindery and Seerr. The field names come from
`RadioStationResource` and the two form requests; the recorded list was
empty (`tests/fixtures/koel-9.11.3/`), so a constructed list of three
stations carries the shape of an entry.

Before release of v0.14.0, `plan` ran inside the host's Koel container with
a token created for the run: one station `(missing) -> (added)`, exit 2; with
a wrong token HTTP 401 at once, exit 1. For v0.14.1 `GET /api/me` was recorded
from the same account (every string masked): `include_public_media` `true`,
so v0.14.1 refuses there until the host turns it off.

## 19. SuggestArr: the whole configuration (v0.15.0, 2026-09-17)

SuggestArr keeps its settings in one flat document. Three properties of the
service, each measured before a line was written, decide the shape of the
task.

### The database is authoritative, the file is not

`config.yaml` looks like the place to write, and a shell unit on the host had
been writing it at every container start. It is not the place. At startup
`migrate_integrations_from_config` copies the file's credentials into the
`integrations` table **only where a row is missing or empty**, and from then
on `merge_db_integrations_into_flat` lays the table over the file on every
read. A value written to the file therefore arrives exactly once — the first
time — and a rotated key never arrives at all. The same family as bindery's
download client, which was created only when absent (§14).

`POST /api/config/save` writes the file *and* synchronises the table, so it is
the only write that holds.

### A partial write is a loss

`save_env_vars` builds the stored document from every key it knows, taking
each one from the request body **or else from its default**. A body naming one
field would silently reset all the others. The task therefore reads the
document, replaces the named fields and sends it back whole — the shape of
Jellyfin's named configurations (§15), for a different reason: there it was
to leave other fields alone, here it is to keep them at all.

Two consequences for a spec: a field name the document does not carry is an
error and not something to add (`fetch` answers with every key, including the
unset ones), and an **empty value is refused** — SuggestArr drops empty values
when it writes, so the next read answers with the default and converge would
write, read back something else and never come to rest.

### The login, and why it is not an API key

SuggestArr's API keys authenticate its public `/api/v1` only; the middleware
refuses them anywhere else. Its configuration endpoints take a JWT, or the
identity header of a reverse proxy from a trusted peer address. Widening that
peer list would have made every process that can reach the port able to claim
any identity — on this host, a service account was the smaller door: `POST
/api/auth/login` is a public route, the bearer branch of the middleware runs
before it ever looks at `AUTH_MODE`, and the account's password sits in a
systemd credential. The login is therefore the one request that carries no
key, and `HttpTransport::anonymous` exists for it.

`GET /api/config/fetch` answers an admin with real keys, so converge can
compare a secret without a `hand_over` (§13) — but no value may reach a change
line. Every field in SuggestArr's own `_SECRET_KEYS` is reported as
`(hidden)`, on both sides of the arrow, whatever its value.

### Libraries are derived, not written down

`JELLYFIN_LIBRARIES` is a list of `{id, name}`. Ids exist only once the
libraries do, and a spec that carried them would rot the moment somebody
renames a library or adds one. `jellyfin_libraries` therefore names the
collection types to leave out, and the task reads the libraries from
SuggestArr itself (`GET /api/jellyfin/libraries`, which passes Jellyfin's
`VirtualFolders` through).

An empty list is not "no libraries" for SuggestArr — its client then fetches
all of them — so a rule that would exclude everything is an error, as is an
empty answer. The change line names the libraries, not their ids: what a
reader wants to see is that `Privat` is out, not that a 32-character id
changed.

### Ready means the libraries answer too (v0.15.1)

The first run on the host failed: the deploy had restarted SuggestArr's
container and Jellyfin's together, SuggestArr answered its configuration at
once, and `GET /api/jellyfin/libraries` said 404 ("No library found") for the
forty seconds Jellyfin needed (measured: the run at 17:14:45, Jellyfin active
at 17:15:27). A run that fails for a service still starting leaves a red unit
for an hour. With a library rule the readiness probe now asks the libraries
too, and any answer but 200 is "not yet"; an empty list in a 200 stays an
error.

## 21. The download client configuration (v0.16.0, 2026-09-17)

`config/downloadclient` is a document like `naming` and `media-management`, so
it is the same task with another `Kind` -- two endpoints, one component,
`DownloadClientConfigResource` for all three services. What makes it worth a
section is the field it holds: `enableCompletedDownloadHandling`. Off, the
service never imports what a client finished, and nothing says so -- no failed
unit, no health message, only downloads that stay where they are. The host had
the switch nowhere in its configuration (measured 2026-09-17: on in Radarr,
Sonarr and Lidarr), so declaring it is a guard, not a change.

Only Radarr carries `checkForFinishedDownloadInterval`; Sonarr and Lidarr
answer without it. A spec naming it against Sonarr fails at the field check,
which is the rule from §3 and not a special case: a path the answer does not
carry is an error, never an addition.

## 22. bindery: the OIDC providers of its own login (v0.17.0, 2026-09-18)

The host wrote them in `bindery-einrichten`: a `PUT` with the provider built by
`jq`, once per guest start, `RemainAfterExit` on the unit. A rotated
`client_secret` therefore arrived only at the next start of the container --
the same gap the download clients had before §14.

Three things make this its own task rather than another `ResourceApi`:

- **`PUT` replaces the whole list.** There is no per-entry route. So the task
  reads the list, applies the declared fields to the entries it names, appends
  the ones bindery does not hold yet, and sends everything back. A provider the
  spec does not name is reported as a note and travels back unchanged --
  dropping it would delete it, and converge does not delete (§1).
- **`client_secret` is write-only.** It is not in the answer
  (`ProviderPublicConfig`), so a stale one cannot be seen. It travels on every
  `apply`, like bindery's `apiKey` and `password` (§14), and it is *always*
  included for an entry being added, because bindery refuses a new provider
  without it ("client_secret required for new provider").
- **`status` is bindery's own.** It holds the result of the discovery check and
  is stripped from what goes back; a spec that tried to set it is refused.

What stays in the unit is the check that follows a write: bindery validates the
issuer's discovery document and puts the outcome in `status.state`, and an entry
can stand there while nobody can log in. That is a reading of a *result*, not a
desired state -- the shell waits for `ok` or `failed` and fails loudly.

## 20. Not in the pilot


- Other services and tasks (Authentik, Seerr, …).
- TLS, JSON output, a NixOS module.
- Deleting things. `converge` only sets what the spec names, and appends
  list entries (§8), connections (§9), subscriptions (§10), root folders (§11, §14), providers (§12, §13), tags (§13), bindery's entries (§14), Seerr's servers
  (§17) and Koel's radio stations (§18) it is responsible for. SuggestArr's
  configuration (§19) is a document, not a list: converge sets the named
  fields and carries every other one back unchanged.
