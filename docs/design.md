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
  connection Lidarr`) and are never touched -- unless the spec says
  `"exactly": true`, and then they are removed instead (§39, v0.36.0).
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
| `livetv` | `LiveTvOptions` (v0.24.0) |

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

### Live TV: the lists are the unit (v0.24.0, 2026-09-18)

The host offers public broadcasters' streams as Live TV: one M3U tuner and one
XMLTV guide source. Both live in `livetv` as lists — `TunerHosts` of
`TunerHostInfo`, `ListingProviders` of `ListingsProviderInfo` — and a spec
names each list **as a whole**. `schema-check` walks such a list element by
element, so a misspelt field inside a tuner fails the build like one at the
top level.

Why not entries by key, as for a plugin's shared lists (`lists`, v0.5.0)? On
this host nothing else writes these lists: Jellyfin's own `/LiveTv/TunerHosts`
and `/LiveTv/ListingProviders` exist for its dashboard, which no one uses
here. A list that is converge's alone is simply a value; a tuner someone adds
by hand is taken away on the next run, which is the point.

Two consequences for a spec. Jellyfin omits null values inside the list as
well, so an element must leave out exactly the fields Jellyfin leaves out, or
the comparison never comes to rest — the host checks that the second run says
`unchanged`. And the write goes through the generic named-configuration route,
not the dashboard's: Jellyfin neither generates an `Id` nor queues a guide
refresh. The spec carries its own ids, and refreshing the guide is an action,
which stays with the caller.

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

### Deletion only on request, and no description

A station removed from the spec stays, like every other entry (§19) — until
v0.36.0, where a spec may say `"exactly": true` and the account's other
stations are removed, never half the list or more (§39). Until then this was
the host's job, on the grounds that the API answer names no owner and only the
host knew whose a station was. What made it converge's after all is the
preference above: with `include_public_media` off, every station in the list
*is* the account's own, and nothing else can be reached.
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

## 23. bindery: the switch over its synced indexers (v0.18.0, 2026-09-18)

bindery holds the indexers Prowlarr syncs into it. The host wants exactly two
of them searched -- MyAnonamouse and AudioBookBay, the book and audiobook
trackers -- and the other `torznab` rows off, because a source whose hits it
cannot reach costs every search the wait for its answer (measured on
2026-09-02: TorrentLeech took 59.9 s for nothing, and the search fell from 60 s
to 4.2 s without it).

**This is the one task that writes to an entry the spec does not name**, and
that is why it is narrow rather than general. The obvious alternative was a
rule in the spec -- "these entries, and every other one matching X gets Y" --
which would have loosened §1 for *every* task, and a selector one word too wide
would then reach entries nobody declared. Instead the whole statement lives in
the code, and the spec is a list of names:

```json
{ "service": "bindery", "task": "indexers", "…": "…",
  "desired": { "enabled": ["MyAnonamouse", "AudioBookBay"] } }
```

- **named** -> `enabled: true`. Said explicitly, not merely "not switched off":
  before a name was on the list, this very task had switched that indexer off,
  and a task that only switched things off would leave it that way and report
  success.
- **`type: "torznab"` and not named** -> `enabled: false`.
- **anything else** -> untouched, and named in the run as such. bindery's
  `newznab` row (a Usenet indexer) is not what this task is about, and silence
  would leave that to be inferred.
- **a name bindery does not hold** -> an error. Without that rule a typo would
  switch off every torznab source and report success -- the failure this task
  exists to prevent, wearing the face of a green run.

Two things the recorded answer settled, both of which the host had wrong:
there is **no `implementation` field** (the kind is `type`), so the shell's
`.implementation // .type` never took its first branch; and an update decodes
**over the stored row** (`IndexerHandler.Update`), so the body carries
`enabled` alone rather than the row read back and modified.

### Why writing only on a difference is not a nicety here

`IndexerHandler.Update` ends every write with `idx.SeedRatioSource =
models.SeedRatioSourceUser`, and `applyProwlarrSeedRatio` (bindery #1065)
returns immediately for a row marked `user`. **So every `PUT` takes that row's
seed-ratio override away from the Prowlarr syncer, for good** -- and not even
another `PUT` puts it back, because the handler sets the field again. On
2026-09-18 five of the host's six indexers stood on `"user"`, all stamped with
the same guest start: its shell unit `PUT`s every row on every start. The task
writes only the rows that differ, which on a settled host is none.

Before release, `plan` ran on the host against the running bindery with the
spec it deploys: `unchanged`, with the note `left alone, not a torznab
indexer: indexer Treasure Maps`.

## 24. The indexer configuration and the default delay profile (v0.19.0, 2026-09-18)

Two documents Radarr, Sonarr and Lidarr all keep, and which nothing in the
host declared -- so the factory default decided.

| task | read | write |
|---|---|---|
| `indexer-config` | `GET .../config/indexer` | `PUT .../config/indexer/{id}` |
| `delay-profiles` | `GET .../delayprofile` | `PUT .../delayprofile/{id}` |

`indexer-config` is a `Document` like `naming` and `download-client-config`:
one object with an id, read it, change the named fields, `PUT` it back whole.
Only Radarr carries `preferIndexerFlags`, `availabilityDelay`,
`allowHardcodedSubs` and `whitelistedHardcodedSubs` (measured 2026-09-18 on all
three), so a spec naming one of them belongs to Radarr alone -- and a field the
answer does not carry is an error, never an addition (§3).

### The delay profile is a list of which exactly one entry is meant

A delay profile decides whether a torrent or a usenet release is taken and how
long the other one is waited for. The **default** profile is the one **without
tags**: everything falls back to it, and a tagged profile belongs to whoever
set that tag.

So the task finds its entry by that property rather than by an id -- ids are
database rows, and a rebuilt instance hands out different ones. No profile
without tags is an error, and more than one is too; a tagged profile is left
alone and named in the run. `tags` and `order` cannot be set by a spec: they
say *which* profile is meant, and the task answers that itself.

```json
{ "service": "radarr", "task": "delay-profiles", "…": "…",
  "desired": { "preferredProtocol": "usenet", "usenetDelay": 0, "torrentDelay": 30 } }
```

### Declaring a value that already matches is the point, not a waste

At recording time all three services had `preferredProtocol: "usenet"` and both
delays at `0` -- the factory default, which nobody had chosen. **"Not set" is no
statement about behaviour**: the host learned that on 2026-09-10, when a check
demanded the *absence* of Ghostfolio's `ENABLE_FEATURE_AUTH_TOKEN` and so
pinned an open door, because the default was *on*. A declared value that matches
is a guard; an undeclared one is a coincidence that the next upstream release
may end.

Before release, `plan` ran on the host against all three running services with
the specs it deploys: `indexer-config` `unchanged` everywhere, and
`delay-profiles` reporting exactly the one intended difference,
`torrentDelay 0 -> 30`.

## 25. Kavita: its server settings (v0.20.0, 2026-09-18)

The host set Kavita's OIDC switches with `sqlite3` in a unit that ran before
every start of the service and was *required* by it. Twice (2026-09-08,
2026-09-14) a database still locked by the stopping process failed that unit,
and with it Kavita. The reason given for staying with SQLite was "there is no
API access": no account had an API key.

**That was measured in the wrong table.** Since 0.8.9 Kavita keeps keys in
`AppUserAuthKey` (`ManualMigrations/v0.8.9/MigrateToAuthKeys.cs`), and
`AspNetUsers.ApiKey` is read by nobody -- a key written there was refused, and
the conclusion was drawn from that. Every account on the host already held two
keys (`opds`, `image-only`).

An auth key in `x-api-key` is tried before any other scheme
(`IdentityServiceExtensions.cs`, `ForwardDefaultSelector`) and signs the
request in **as its account, with all its roles**
(`AuthKeyAuthenticationHandler`). Kavita does not restrict a key by its name.
So an administrator's key reaches every administrator's endpoint, the settings
among them. The probe asks one of those (`/api/Server/server-info-slim`), so a
key of an account that is no administrator fails at once instead of at the
first write.

| task | read | write |
|---|---|---|
| `server-settings` | `GET /api/Settings` | `POST /api/Settings` (the whole document) |

Checked by path against `ServerSettingDto` of the description in Kavita's
source tree (`openapi/SOURCE.md`), like Jellyfin's `server-configuration` (§6).
The OIDC block takes effect at once: `SettingsService.UpdateSettings` sets
`Configuration.OidcSettings` in the running process; no restart.

### What a spec may not name

Kavita copies `authority`, `clientId`, `secret` and `customScopes` from
`appsettings.json` into the database **at every start** (`Seed.cs`), and the
host writes that file. A spec setting them would be undone at the next start
-- and a changed `authority` clears every account's OIDC link
(`oidcService.ClearOidcIds()`). `port`, `ipAddresses`, `baseUrl` and
`cacheSize` are "managed in appSetting.json" in Kavita's own words, and the
host writes that file too. `enabled` is derived, the install fields are
Kavita's. The SMTP password is a secret, and a spec is no place for one. All of
these are refused when the spec is loaded, and so is any path containing or
contained in one of them (`oidcConfig` as a whole).

### Two values in the answer

`oidcConfig.secret` comes back as asterisks of its length; when exactly those
asterisks come back in a `POST`, Kavita puts the stored secret in again
(`UpdateOidcSettings`). A round trip keeps it -- which is why the task sends the
document as it was read. `smtpConfig.password` comes back in clear text. No
change can show either (neither can be named), and no error carries anything
from a settings body: a decode error says only where it failed, and a refused
write shows Kavita's sentence only when it is one short line (Kavita answers a
translated rule, never a value).

## 26. Kavita: its libraries (v0.21.0, 2026-09-18)

The host kept its one library at the declared name and type with `sqlite3`
before every start of Kavita -- because "the update needs an administrator".
With an auth key (§25) it has one.

| task | read | write |
|---|---|---|
| `libraries` | `GET /api/Library/libraries` | `POST /api/Library/update` (the whole library), then `POST /api/Library/scan?libraryId=&force=true` after a change of `type` |

**Found by a folder it holds**, not by id or name: an id is a row, and the
name is one of the things being set. A folder no library holds is an error --
converge does not create libraries -- and so is a folder two libraries hold.
Libraries the spec does not name are listed as notes.

**The update is written whole, from the answer.** `UpdateLibraryDto` requires
fifteen fields; the answer (`LibraryDto`) carries each under the same name,
except the file types, `libraryFileTypes` there and `fileGroupTypes` here. A
field the answer lacks is an error before anything is sent. `id`, `folders`
and the file types cannot be named by a spec. Each spec field is checked
against both components: it is written with one and compared with the other.

**A change of `type` forces a scan.** Kavita's update scans after a type change
-- without `force`, and its scanner skips files that did not change on disk. A
library switched from Manga to Book would keep every series as the manga
parser read it. The host did this before by turning scan timestamps back in
the database; here it is the scan endpoint with `force=true`, sent only when
`type` differs.

Kavita's side navigation keeps a copy of a library's name per user
(`AppUserSideNavStream.Name`) and never updates it. In 0.9.1.4 the side
navigation itself shows the library's own name (`side-nav.component.html`,
`navStream.library.name`); the copy still shows on the page that customizes
the side navigation, in its search field, and in OPDS. It goes stale only when
a library is renamed. The task does not touch it: the streams belong to each
user, and there is no administrator's endpoint that writes another user's.

## 27. Audiobookshelf: its authentication settings (v0.22.0, 2026-09-18)

The host wrote the OIDC settings into `settings.server-settings` with
`sqlite3`, in a unit *required* by the service and run before every start,
without even a busy timeout -- a locked database kept Audiobookshelf down. The
reason: no access. The bootstrap password is thrown away, local login is off,
and nobody holds an API key.

**No key is needed.** Audiobookshelf accepts any JWT signed with its
`tokenSecret` that carries a `userId`, is not a refresh token and has not
expired (`TokenManager.jwtAuthCheck`, `isBearerAccessTokenPayload`) -- API keys
(`type: 'api'`) are the other branch, the one that needs a database row. The
host therefore signs an access token for the root account that lives fifteen
minutes, reading the secret from the database it already has; nothing is
written. converge sends it as `Authorization: Bearer`.

| task | read | write |
|---|---|---|
| `auth-settings` | `GET /api/auth-settings` | `PATCH /api/auth-settings`, only the keys that differ |

`PATCH` is applied key by key, and the running process switches auth
strategies on and off at once (`MiscController.updateAuthSettings`) -- no
restart. The probe is `/status`: no token, a version once a root account
exists. An administrator's token is checked at the first read (403 otherwise).

**The client secret is in the answer, in clear text.** It comes from a
credential (`secret_fields`), is compared without being shown, and travels
only when it differs. A spec cannot name it in `set`, nor
`authOpenIDSamplePermissions`, which Audiobookshelf derives.

**`""` is `null`.** Audiobookshelf stores an empty string as `null` on a write
and compares a stored `""` as `null` -- for every key but
`authOpenIDSubfolderForRedirectURLs`. The recorded answer holds
`authOpenIDAdvancedPermsClaim: ""`; a spec saying `null` would differ forever
while Audiobookshelf reported nothing to update. converge compares by the same
rule, and `authActiveAuthMethods` sorted, as Audiobookshelf does.

Audiobookshelf publishes no OpenAPI description; the specs are validated, the
field names checked at runtime and against the recorded answer.

## 28. Audiobookshelf: the permissions of its administrators (v0.23.0, 2026-09-18)

An account type carries no permission in Audiobookshelf 2.36: `User.canDelete`
is `permissions.delete && isActive`, nothing more. An account an OIDC login
creates starts with a reader's permissions, and its group claim sets only the
type. An administrator could not delete a misimported item until someone set
the fields. The host set them with `sqlite3` before every start -- so a new
administrator got them at the next restart.

**The advanced-permissions claim is no way out.** It is the obvious candidate
and it skips exactly these accounts: `OidcAuthStrategy.updateUserPermissions`
returns early for `admin` and `root`. It would instead rewrite every other
account's permissions at each login, and refuse a login whose userinfo lacks
the claim.

| task | read | write |
|---|---|---|
| `admin-permissions` | `GET /api/users` | `PATCH /api/users/{id}` with `{"permissions": {…}}`, per account, the differing keys only |

`UserController.update` merges the named keys into the account's permissions
and takes booleans only; a root account may be changed by root alone, which
the host's token is (§27). A spec names account types and permissions;
converge sets exactly those on every account of those types and leaves every
other account alone. No account of the types yet is a note, not an error --
on a fresh instance nobody has logged in.

**The answer carries every account's `token`.** Decoding keeps `id`,
`username`, `type` and `permissions` and drops the rest; a change names the
username, the permission and two booleans, and no error carries anything from
a body.

## 29. Dispatcharr: Live TV through an IPTV proxy (v0.25.0, 2026-09-19)

The host offers the public broadcasters' streams as Live TV in Jellyfin.
Jellyfin alone picked the first video stream ffmpeg listed in each HLS master
playlist -- 270p for the ARD channels -- and hung on ARTE. The broadcasters
deliver audio as separate renditions, so pointing Jellyfin at a single variant
loses the sound. Dispatcharr sits in between: its `streamlink` stream profile
asks for `best`, which muxes the best variant with its audio (1080p or 720p
with sound for all eighteen channels, measured on the host), and Jellyfin
reads plain MPEG-TS from it.

| task | read | write |
|---|---|---|
| `stream-settings` | `GET /api/core/streamprofiles/`, `GET /api/core/settings/` | `PATCH /api/core/settings/{id}/` with the whole `value` |
| `m3u-accounts` | `GET /api/m3u/accounts/` | `POST` a missing account, `PATCH /api/m3u/accounts/{id}/` with the spec's fields |
| `m3u-groups` | the accounts and `GET /api/channels/groups/` | `PATCH /api/m3u/accounts/{id}/group-settings/` with `{"group_settings": [...]}` |
| `epg-sources` | `GET /api/epg/sources/` | `POST` a missing source, `PATCH /api/epg/sources/{id}/` |

**The profile is named, not numbered.** The core setting `stream_settings`
stores the default profile's id; ids depend on the order an instance created
its profiles in. A spec names the profile, converge looks the id up, and an
unknown name is an error that lists the names there are.

**Groups wait for the playlist.** A new M3U account loads its groups
asynchronously (a Celery task, `refresh_account_on_save`); until then the
account has none, and the engine reads back without writing again. So the
groups are a task of their own whose readiness is "every group the spec names
is part of its account": on a fresh instance one run adds the account, then
waits for the groups, then sets them. Changing a group or an account's file
does not re-read the playlist -- that is an action (`POST
/api/m3u/refresh/{id}/`) and stays with the caller.

**The description is wrong once.** drf-spectacular declares the
group-settings body as `PatchedM3UAccount`; the view reads `group_settings`
from the request. The endpoint is listed without a shape, and a spec's group
fields are checked against `ChannelGroupM3UAccount`, the membership the
account answers with -- and at parse time against the four fields the view
reads. Channel numbers come back as floats (`1.0`); a spec's `1` is the same
number.

**Logins are rationed.** There is no API key a spec could hold (Dispatcharr
generates keys itself), so a task logs in as a service account, as for
SuggestArr (§19). Dispatcharr allows three logins a minute per client address:
a unit with four specs was refused on the fourth. converge now logs in once
per base URL and account and reuses the access token (thirty minutes) for
every spec of the run.

`refresh_interval` 0 means *never* for both accounts and sources
(`core/scheduling.py`: disabled unless a cron or a positive interval is set).
A spec that wants a guide that stays current says so.

## 30. Dispatcharr: an Xtream Codes account from credentials (v0.26.0, 2026-09-19)

The host adds a paid Xtream Codes provider next to the public broadcasters.
Such an account is three secrets -- the provider's `server_url`, a `username`
and a `password` -- and §29 kept all credentials out of a spec, because a plan
prints what differs. An account now takes them from credentials:

```json
"accounts": {"Anbieter": {"account_type": "XC", "is_active": true,
  "secret_fields": {"server_url": "xt-url", "username": "xt-user", "password": "xt-pass"}}}
```

`secret_fields` is not a Dispatcharr field; it maps a field to the credential
that holds its value, for M3U accounts only, and a field may not stand both
there and among the plain ones. The field names are still checked against the
OpenAPI components, so a renamed field fails the schema check.

Read in `M3UAccountSerializer` (0.31.0) and in the recorded answer:
`server_url` and `username` come back in the clear, `password` is
`write_only` and answers as `""`. So the two are **compared without being
shown** -- a change reads `(another value, not shown) -> (the credential's,
not shown)` -- and the password is **handed over on every `apply`**, as for
bindery (§14): a `PATCH` that carries only the password. Dispatcharr saves it
without side effects: `refresh_account_on_save` acts only on a created
account, and the refresh schedule is rewritten only when `refresh_interval`,
`is_active` or `refresh_task` change (`apps/m3u/signals.py`).

When an account is added or one of its readable secrets differs, the body
carries every secret: a new user name with the old password would be half an
account. A rotated password alone therefore reaches Dispatcharr on the next
`apply` without a change in the plan -- the plan cannot see it.

Nothing else changes for a group: an Xtream account's groups load like a
file's, asynchronously after the account's first refresh, and `m3u-groups`
waits for them as before.

## 31. Dispatcharr: an EPG source's URL from a credential (v0.27.0, 2026-09-19)

An Xtream Codes provider serves its guide at
`<server>/xmltv.php?username=<user>&password=<password>` -- measured on the
host's provider: 78 MB, 8350 channels, two days ahead, among them the German
Sky, DAZN and Eurosport channels its streams name in `epg_channel_id`.
Dispatcharr has no Xtream source type (`xmltv`, `schedules_direct`,
`dummy`), so the guide is an XMLTV source whose URL carries the account's
password. That URL cannot stand in a spec.

An EPG source therefore takes `url` from `secret_fields`, and only `url`.
Dispatcharr answers it in the clear (`EPGSourceSerializer`), so it is
compared without being shown, like an account's `server_url`; nothing is
handed over. The caller composes the credential -- on the host a sops
template joins the three values of the account.

## 32. Dispatcharr: a group's stream profile, and a write that kept nothing (v0.28.0, 2026-09-19)

The host's default stream profile is `streamlink` (§29): it asks the public
broadcasters' HLS playlists for `best` and muxes the audio rendition in. An
Xtream Codes stream is a bare MPEG-TS URL that no streamlink plugin claims --
measured on the host: `No plugin can handle URL`, 188 bytes in fifteen
seconds. Those channels need the `Proxy` profile.

Dispatcharr keeps a profile per channel, and the channel sync assigns a
group's `custom_properties.stream_profile_id` to every channel of the group,
existing ones included (`apps/m3u/tasks.py`). A group setting therefore takes
`stream_profile` **by name**, like the default profile in §29; converge looks
the id up (reading the profile list only when a spec names one) and sets it
in `custom_properties`. The web UI stores the id as a string, the sync reads
`int(...)`: `"3"` and `3` are the same profile.

Reading the view for this showed a fault in v0.25.0 to v0.27.0: the
group-settings view **replaces** each membership it is sent
(`bulk_create` with `update_conflicts`), and a field left out falls back to
its default -- `custom_properties` to `{}`. converge sent only the spec's
fields, so every write took away what the web UI had set on the group. A
group now goes out whole: the five fields the view writes, current values
first, the spec's on top.

## 33. Dispatcharr: which streams of a group become channels (v0.29.0, 2026-09-19)

An Xtream provider's group is not a channel list. The host's provider keeps
every channel in four to six variants -- `(720P)`, `(SAT)`, `(MOBIL)`, `SKYGO
… HD`, `SKYGO … 4K`, `NOW … ᴿᴬᵂ` -- next to event and 24/7 channels, and the
28 groups the host first enabled made 1143 channels. Measured with ffprobe,
the variants differ: `SKYGO … 4K` and `NOW … ᴿᴬᵂ` are 1080p at 50 fps,
`(720P)` and `SKYGO … HD` 720p, `(SAT)` 1080i.

The channel sync filters and renames per group from `custom_properties`:
`name_match_regex` (keep what matches), `name_match_exclude_regex`,
`name_regex_pattern` and `name_replace_pattern` (JavaScript-style `$1`; an
empty replacement removes the match). A group setting takes these four by
name, as text; converge compares them with the group's `custom_properties` and
writes them there, next to the stream profile (§32) and whatever else the web
UI set. A channel whose stream no longer passes the filter is removed by the
sync itself (orphaned auto channels, default `always`).

## 34. Redirects keep the key home, and decode errors quote no value (v0.30.0, 2026-09-19)

Two findings of the host's security audit (B39, B40), both measured.

**A redirect carried the key to another host.** ureq drops `Authorization`
and `Cookie` when it follows a redirect, but not a custom header: a service
answering `302 Location: http://elsewhere/…` had converge send its
`X-Api-Key` or `X-Emby-Token` there (two listeners, the second one saw the
key). The agent now follows nothing (`max_redirects(0)`); `get` follows a
redirect itself, and only on the service's own origin -- an absolute path, or
a URL with the same scheme, host and port. Another host, another port, a
scheme change or a protocol-relative `//host` is an error that names the
status and not the target. A trailing-slash redirect (Flask, Django) still
works. Writes follow nothing: a 3xx comes back as a status the caller
refuses -- ureq used to turn a redirected `PUT` into a `GET` without its body,
which was never right.

**A decode error quoted the value it could not read.** serde_json says
`invalid type: string "…", expected f64`, and the string is whatever the
service answered -- in a settings document, possibly a key. The message went
to the journal and on to Loki. `error::shape` keeps what is converge's own --
the kind of error, where it broke, `missing field` names and what the types
expected -- and drops what came from the answer. Every decode of a service's
answer goes through it; the spec's own file does not need to.

## 34. Dispatcharr: the guide entry of a channel, as an override (v0.31.0, 2026-09-19)

The Xtream provider's guide has no categories at all -- measured on the
host: 1131 programmes of the 60 channels, none with a `<category>` -- so a
media server cannot tell a football match from a film, and twelve of the
channels bring no tvg-id. The public guides of epgshare01 carry categories
(`Fußball`, `Sports`, `Motor Sports`, …) and know those channels, under
their own ids (`Sky.Sport.News.de`, `SkySp.PL.HD.uk`).

A channel's guide entry cannot simply be set: the channel sync of an
auto-created channel re-resolves `epg_data` from its stream's tvg-id on every
refresh (`apps/m3u/tasks.py`) and would undo it by the next morning. What the
sync leaves alone is the channel's **override** (`ChannelOverride`), and
`PATCH /api/channels/channels/edit/bulk/` with
`[{"id": …, "override": {"epg_data": …}}]` writes one -- measured on the host:
`override.epg_data_id` and `effective_epg_data_id` both the new entry, the
channel's own `epg_data_id` untouched. The same write queues the programme
import for the new entry.

The task `channel-epg` takes channels by their effective name, each with a
source by name and a tvg-id, and compares `effective_epg_data_id`. Readiness
waits until every named channel exists and every named entry has been read
from its source; a source added in the same run is parsed asynchronously.

Two things the description gets wrong or leaves open: `GET
/api/channels/channels/` is declared as a page, but answers a plain list
without `page_size` (recorded, and read that way, so no response shape is
checked); and `EPGData.tvg_id` and `epg_source` are nullable.

## 35. Dispatcharr: a channel's name and logo, next to its guide (v0.32.0, 2026-09-19)

The Xtream channels are named what the provider calls its variants
(`SKY SPORT BUNDESLIGA 3` after the group's rename) and many share one
picture -- all ten Bundesliga channels the same. A name and a logo are
override fields too, and the channel sync resets both of an auto-created
channel on every refresh, so `channel-epg` now takes, next to the guide
entry, a display `name` and a `logo_url`; each is optional, and an entry
names at least one.

Channels are now found by their OWN name, the one the sync gives them; the
displayed name is what this task changes, so a lookup by it would lose the
channel after the first run.

A logo is an object of its own with a unique `url` (`Logo.url`). converge
reads every logo -- the list is paginated whatever is asked (recorded: 50 a
page, `next` set; the view allows 1000) and only when a channel names one --
compares the URL behind `effective_logo_id`, and creates a missing logo with
`POST /api/channels/logos/` (201) before the override names its id. The bulk
override carries only the fields that differ, so the rest of an existing
override stays.

**One POST per URL, not per channel (v0.33.1, 2026-09-21).** `Logo.url` is
unique, and the write created a missing logo once for every channel that
named it: the second POST of the same URL answered 400 and the bulk override
was never sent. It went unnoticed while every channel had a picture of its
own; thirty channels sharing one (twenty event slots and ten round-the-clock
channels of one provider) hit it on the first run. The write now remembers
the logos it created, by URL, and names their id for every further channel.

## 36. Dispatcharr: fallback streams, from the channel's own group (v0.33.0, 2026-09-19)

Each Xtream channel plays one stream, the best variant its group's name
filter picks (`SKYGO: SKY SPORT GOLF 4K`). The provider carries others of the
same channel -- in the same group (`… HD`) and, for the German sports
channels, in another (`DE: SKY SPORT GOLF HD (SAT)`, group `DE| SPORT
HD/4K`). A channel's stream list is ordered, and Dispatcharr's proxy switches
to the next stream when one fails; so `channel-epg` takes, per channel,
`fallback_streams` by name, and makes the channel's list its own stream
followed by those.

**The channel sync leaves such a list alone** -- measured on the host: channel
`SKY SPORT GOLF` given `[400, 370]` by hand, the account refreshed, the sync
reported "0 created, 0 updated, 0 deleted" and the list was still
`[400, 370]`. The sync maps every stream of its group to the auto channel
holding it and deletes a channel when none of its streams in the group is
matched any more; a fallback the filter does not match is simply not looked
at.

**And that is exactly why a fallback from ANOTHER group is refused.** The
sync of that group finds the channel through the fallback, sees none of the
channel's streams in its group matched -- its filter was written for other
channels -- and deletes the channel ("Delete channels whose streams have all
disappeared", `apps/m3u/tasks.py`). The SAT variants above sit in the group
that feeds the Eurosport channels, which is synced. converge refuses such a
fallback as a `Mismatch` (fatal in the probe: waiting does not move a stream
into another group) instead of writing a list that deletes its channel on the
next refresh.

The channel's own stream is the FIRST of its list: the one the sync assigned.
Fallbacks are looked up by name in the stream list of the same account; one
that is not there is a note, not a wait -- the provider may have dropped it
for good -- and the list is written without it. A name that matches several
streams is refused. The stream list is paginated and read only when a channel
names a fallback. `PATCH /api/channels/channels/edit/bulk/` takes `streams`
next to `override` and keeps the given order (`api_views.py`, `edit_bulk`);
an entry now carries only what differs, since an empty `override` is a write,
not nothing.

## 37. Authentik: the tenant settings outside the blueprints (2026-09-22)

Authentik is configured with blueprints, and this host declares flows,
providers, groups and its brand in them. A handful of settings are not in
that schema: they belong to `authentik_tenants.tenant`, which no blueprint
model covers. The host therefore set two of them with a shell unit, hourly --
`reputation_lower_limit` (-10, how far a client's reputation may sink before
a policy stops trusting it) and `impersonation` (false, whether an
administrator may take over a user's session).

| task | read | write |
|---|---|---|
| `settings` | `GET /api/v3/admin/settings/` | `PATCH /api/v3/admin/settings/`, only the fields that differ |

`PATCH` takes `PatchedSettingsRequest`, which requires nothing and changes
exactly what its body names (measured on the host, 2026-09-18: HTTP 200, the
fields the body left out unchanged). `PUT` would take `SettingsRequest`, the
whole document -- the shape Kavita forces (§25) and the one that carries
somebody else's field back with it. Here it is not needed.

**Every field is a top-level field of `Settings`, named, not a path.** Two of
them are containers: `flags` is an object and `footer_links` a list, and
authentik stores each whole. A path into one of them would read a part and
write a part, and the write would drop whatever the path did not name; the
spec parser refuses a dotted key for that reason. `footer_links` as a whole
may be set -- it is one value.

**The token.** The credential holds a bearer token, as ntfy's and Koel's do.
The host's unit creates a fifteen-minute token of a superuser service account
for each run and hands converge that; converge sees a credential and nothing
more, and never learns where it came from or how long it lives. The probe
asks `GET /api/v3/admin/version/` for `version_current` -- an administrator's
endpoint, so a token authentik does not know (401) and a token whose account
is no administrator (403) both fail there, with a named reason, instead of at
the first write.

**Nothing of an answer is repeated.** A refused request answers
`{"detail": …}`, a refused write DRF's validation shape -- an object from
field names to messages, and a message may quote the value it refused. Only
the field names travel into an error; the status says the rest. The settings
themselves carry no secret, but that is not a licence to print a foreign body.

**The paths keep their trailing slash.** Django answers a path without one
with a redirect, and since v0.30.0 converge does not follow a redirect with
the key on it.

**The description's server prefix.** Authentik's OpenAPI file comes from
drf-spectacular and names a server, `/api/v3`; its paths therefore read
`/admin/settings/`. Dispatcharr's file, from the same generator, names no
server and spells every path in full. An `Endpoint` here always carries the
path a request goes to, so `schema::check` takes `servers[0].url` off before
it looks a path up -- and leaves a path that does not start with it alone.
A spec's fields are checked against `Settings`, which they are compared with,
and against `PatchedSettingsRequest`, which they are written with, as a
Kavita library's are checked against its two components (§26).

## 38. Audiobookshelf: its libraries (2026-09-22)

The host created its two libraries once, in the bootstrap, with a `curl`
against `POST /api/libraries` -- and that worked only inside the window in
which the local login was still on. Afterwards nobody looked again: a name,
an icon or a metadata provider changed in the web interface stayed changed,
and a third library was a hand's turn. With an access token of the root
account (§27) both are a task.

| task | read | write |
|---|---|---|
| `libraries` | `GET /api/libraries` | `POST /api/libraries` (missing), `PATCH /api/libraries/{id}` with the differing fields only (present) |

**Found by the `fullPath` of a folder it holds**, as Kavita's libraries are
(§26), and for the same two reasons: an id is a row a rebuilt instance hands
out differently, and the name is one of the things the task sets. A folder two
libraries hold is an error -- nothing says which one is meant. Libraries the
spec does not name are notes; converge deletes nothing.

**Unlike Kavita's task, this one creates.** Kavita's `POST /api/Library/create`
would need a library type, a folder list and a field set converge has no
answer to compare against; Audiobookshelf's `create` needs a name and a folder
and defaults the rest (`book`, `database`, `google`). So a folder no library
holds is a change, `(missing) -> (added)`, as with Koel's radio stations
(§18) and bindery's root folders (§14) -- and `name` becomes required there.
A spec that names a folder without a name fails in `diff`, before anything is
written: the body is built there as well.

**The fields are the ones `LibraryController.update` reads** (2.36.0, read on
the host): `name`, `provider`, `mediaType` and `icon` as strings, each only
when truthy -- so `""` is a spec error, since it would differ forever while
Audiobookshelf reported nothing to change --, `displayOrder` as a number, and
`settings` as an object it merges **key by key** into the stored settings,
each key type-checked (`markAsFinishedPercentComplete` 0 to 100 or null,
`markAsFinishedTimeRemaining` at least 0 or null, arrays and strings as such,
everything else against the type of the default). `folders` it reads too; a
spec cannot name them, they are what says which library is meant.

`settings` is therefore compared and written per key, and a change names
`settings.<key>`. A key the answer does not carry is an error, not an
addition: `podcastSearchRegion` is a podcast library's setting, and on a book
library Audiobookshelf would drop it.

`GET /api/libraries` answers `{"libraries": [...]}`, each library with its
`folders` as objects (`fullPath`, `id`, `libraryId`, `addedAt`). An empty list
is a valid answer -- a fresh instance holds no library, and every library the
spec names is then a creation. The recorded answer is
`tests/fixtures/audiobookshelf-2.36.0/libraries.json`.

Audiobookshelf publishes no OpenAPI description, so `schema-check` validates
the spec and nothing else, as for `auth-settings` (§27) and
`admin-permissions` (§28); the field names are checked against the recorded
answer and at runtime. 403 says the token's account is no administrator, 401
that the token was refused, and no error and no change carries anything from a
body.

## 39. Deleting, on request: `exactly` (v0.36.0, 2026-09-22)

Until this version converge removed nothing, and that was a decision with a
cost. The host it was written for still carried three shell remnants whose
only job was deleting: one that dropped Trailarr connections, one that removed
Koel's radio stations, one that cleaned up the quality profiles Recyclarr no
longer wrote. Three languages, three ways of naming the same thing, and not
one of them had a `plan` mode -- the only way to learn what such a script
would take away was to let it run.

**The switch is `"exactly": true`,** on the top level of a spec, next to
`service`, `task` and `desired`. It says: this spec names the whole
collection; every entry of it the spec does not name is removed. Without it
nothing changes -- the default is `false`, `false` may be written out, and an
entry outside the spec stays the note it has always been.

Decided with the host's owner on 2026-09-22: **opt-in per spec, never a
default, and visible in `plan`.** A flag per spec and not per run, because the
question "may this list be emptied" belongs to the list, not to the person
typing the command; a run-wide `--delete` would answer it for specs nobody was
thinking about. And a task that cannot delete refuses the switch (`task
quality-definitions does not support exactly`) instead of accepting one that
quietly does nothing -- the same reason `keep` without `exactly` is an error
rather than a field that is ignored.

### In the engine

Two methods on `Task`, both with a default that removes nothing, so the other
thirty-odd tasks are untouched:

| method | answers |
|---|---|
| `surplus(current)` | the subjects that would go, as strings |
| `remove(t, current)` | takes them away |

`plan` prints one line per subject, `<subject>: (present) -> (removed)` -- the
counterpart of the `(missing) -> (added)` a list task already prints --, counts
them among the changes and exits 2 like any other difference. `apply` runs
`remove` **before** `write`, and a failure in `remove` ends the run before a
single write goes out.

**The order is not a detail.** A Koel station renamed in the spec but keeping
its URL is one removal and one addition. Koel's URL is unique per account, so
writing first means a `POST` for the new name while the old station still
holds that URL -- 422, and the run fails with nothing done. Removing first,
the addition finds the URL free.

Afterwards the engine reads back as it always has, with one more condition:
`surplus` must come back **empty** as well as `diff`. An accepted `DELETE` is
no more a finished one than an accepted write is a saved one; an entry still
listed afterwards is `written, but after 60 s these still differ: …
(present) -> (removed)`. A run is `unchanged` only when neither has anything.

### The three tasks, and the guard each one inherited

Each task that got the switch took over the guard its shell predecessor had --
and each guard runs in `plan` too, and before the first `DELETE`.

**Trailarr `connections`** (§9): surplus is every connection whose `name` the
spec does not name; `DELETE /api/v1/connections/{connection_id}`, any 2xx is
good. A refusal is an `Error::Status` with the method, the path, the status and
one validation line naming the **connection** -- never the body, because
Trailarr's answers carry the API keys of the Radarr and Sonarr it connects to
(§9). The endpoint is in `ENDPOINTS`, so `schema-check` holds it against the
deployed description like every other one.

**Koel `radio-stations`** (§18): surplus is every station of the **account's
own** list the spec does not name. It can be nothing else: `read` already
refuses to run while `include_public_media` is on, and with it off the list is
`whereBelongsTo` the account (§18). That check was written so a `PUT` could not
land on somebody else's station; it is what makes a `DELETE` here defensible at
all.

> **And never half the list or more.** A station is found by its `name`, and a
> name is a string somebody typed. One different Unicode normalisation on
> either side and not a single spec station would match any of Koel's -- every
> one of them would look like a surplus, and a correct, green run would empty
> the account. `2 * surplus >= all` is therefore an error before anything is
> removed: `would remove 2 of 3 stations (half or more) -- nothing removed`.
> The rule is deliberately crude; it is not there to catch a careful edit but a
> total mismatch.

**Radarr and Sonarr `quality-profiles`** (§5): the spec named only
`allow_in_every_profile` until now. With `exactly` -- and only with it --
`desired.keep` names the profiles that survive; it is required there, may not
be empty, and may not repeat a name. Surplus is every profile whose `name`
`keep` does not hold; `DELETE /api/v3/qualityprofile/{id}`, 200 and 202 are
good.

> **Nothing is removed until every kept profile is there.** On the host,
> Recyclarr writes the profiles from TRaSH's templates and converge takes the
> rest away -- two programs on one collection. If Recyclarr has not run yet, or
> a template renamed a profile, the profiles converge means to keep are missing
> and "the rest" is *everything*. So a kept profile the service lacks is an
> error naming the count: `1 of 2 kept profiles present -- has the profile
> writer run yet? nothing removed`.
>
> **A profile still in use is reported, not forced.** A profile films or series
> hang on is refused by the service with a 4xx. converge does not reach around
> that -- deleting the profile would mean deciding what happens to the media on
> it, and that is not converge's decision. The other removals are still tried,
> and the run ends red with every name that stayed: `not removed: profile Anime
> (HTTP 409, still in use?)`. The status is all that is taken from the answer.

A profile on its way out is also left out of `diff` and of `write`: it need not
allow `Unknown` in order to be deleted, and a `PUT` racing its own `DELETE`
would be pointless. The same holds for the notes -- with `exactly`, "not in the
spec, left as it is" would be a lie, and those entries are removals instead.

### What Koel's `DELETE` route does

`DELETE /api/radio/stations/{station}`, read at tag **v9.11.3** of
`github.com/koel/koel`. The routes live in `routes/api.base.php`, not
`routes/api.php`, and the `/api` prefix comes from that file itself
(`Route::prefix('api')`), not from a provider. The block is
`Route::apiResource('stations', RadioStationController::class)` inside
`Route::group(['prefix' => 'radio'])`, so the parameter is `{station}` and its
siblings are the `GET` and `POST` converge already uses.

`RadioStationController::destroy` authorises and then calls
**`$station->delete()`** on the Eloquent model, answering
`response()->noContent()` -- 204 with an empty body. That matters: the host's
old shell removed a station through the model *on purpose*, "so the observer
clears the logo away", and the API route goes the very same way.
`RadioStation` carries `#[ObservedBy(RadioStationObserver::class)]`, whose
`deleted` hook runs `ModelImageObserver::onModelDeleted` and unlinks the stored
image. A query-builder delete would have skipped the model events and orphaned
the logo file; this one does not. converge accepts 204 and 200; anything else
is an error naming the station.

The policy on `destroy` is `RadioStationPolicy::delete`, which is
`edit`: the owner, or an account with `MANAGE_RADIO_STATIONS` in the same
organization. converge's account is the owner of every station it sees, since
that is what `include_public_media` off means.

## 40. Jellyfin: account policies and display preferences (2026-09-22)

Three shell units on the host write into Jellyfin per account, on a timer:
one sets `AuthenticationProviderId` to the LDAP plugin, one sets four
permissions (`EnableAllFolders` false, `EnableSubtitleManagement` true,
`EnableLiveTvAccess` true, `EnableLiveTvManagement` false), one sets the
custom pref `livetv-favoritechannelsattop` to `"false"`. Every value is a
constant in a shell script, and nothing reads them back. `IsAdministrator` is
the field none of them touches: who is an administrator is whatever somebody
once clicked, and the recorded answer shows it -- one of nine accounts, and no
file in the host repository says so.

| task | read | write |
|---|---|---|
| `user-policies` | `GET /Users` (`UserDto` with `Policy` embedded) | `POST /Users/{userId}/Policy` with the whole `UserPolicy`, per differing account |
| `display-preferences` | `GET /Users`, then `GET /DisplayPreferences/usersettings?userId={id}&client={client}` per account | `POST` of the same path with the whole `DisplayPreferencesDto` |

**Both write the whole document, because Jellyfin replaces it.**
`UpdateUserPolicy` declares a required body of one `UserPolicy` and answers
`204`; there is no partial body and nothing it could merge into. A write that
carried only the named fields would
therefore reset the other thirty-six to their defaults -- which folders an
account may see, its device list, its bitrate limit. So the task reads the
policy, changes the named fields in it and sends it back as it came, the way
`server-configuration` does with the configuration document (§6). The same
holds for the display preferences: the document carries sort order, image
sizes and the sidebar switch next to `CustomPrefs`.

**`all` and `accounts`, because both questions are real.** Four of the five
fields the host sets are the same for everybody, and writing them nine times
into a spec would be nine chances to mistype one. `IsAdministrator` is the
opposite: exactly one account carries `true` and the rest `false` -- and
saying it that way is the point, because a spec that named only the
administrator would leave a second one, added by hand in the web interface,
standing. `all` says what everybody carries, `accounts` overrides it by the
account's `Name`, and the two together make "one administrator" a statement
converge can hold.

**An account name the answer does not hold is a note, not an error.** It was
an error at first, and on the host that would have been wrong: the
administrators are derived from the authentik group `Verwaltung`, but a
Jellyfin account does not exist until its owner has signed in once. A new
member of that group would therefore have kept the unit red -- every run, for
every account -- until the day they first logged in, and the one thing such a
red says nothing about is whether the eight accounts that *are* there still
carry what the spec names. So the name is skipped, `converge` says
`account <name>: not on the service yet — skipped`, nothing is written for it,
and the outcome is whatever the accounts the service does hold make it. The
price is that a misspelled name is now quiet as well; the trade is deliberate,
because the other kind of silence was worse. `all` is untouched by this: it
names no account, so it has no name that could be absent.

**A policy field the answer does not carry is an error; a custom pref it does
not carry is a change.** The difference is in the two documents.
`UserPolicy` is a C# class: Jellyfin serialises every property, so a name that
is not in the answer is a misspelling -- `EnableAllFoldrs` would otherwise be
added to the policy and written back forever. `CustomPrefs` is a string map a
client fills as it goes; on the recorded instance it holds eleven keys, two of
them `null`, and an account that has never opened the Live TV page simply has
no `livetv-favoritechannelsattop`. That is `(missing) -> "false"`, the case
the host's own unit was written for.

**The values are strings, all of them.** `CustomPrefs` is
`additionalProperties: {type: string, nullable: true}`, and the web client
stores `"false"`, `"true"`, `"10000"`. A spec that wrote `false` would be a
spec that never converges, so `custom_prefs` takes strings and a boolean is a
spec error. For the same reason `schema-check` says nothing about these keys:
they are not properties, and no description can know them. A `user-policies`
spec's fields *are* properties, and are checked against `UserPolicy` for
existence, type and enum, as `server-configuration`'s are (§3).

**The trap is that there are two writers.** The host's `jellyfin-gruppenabgleich`
runs every fifteen minutes and writes each account's policy **whole** as well
-- it has to, for the same reason converge does. Two writers on one document
is the pattern that cost the most here before (two authentik files describing
one membership, two Jellyfin plugins setting one folder list): each write is
correct on its own, and the one that ran last wins. Converge cannot be the only
writer, and it does not try: it changes the fields a spec names and carries
every other one back as it read it, so its write is a no-op for everything the
group sync owns. **What the two must not do is disagree about a named field.**
As long as the constants in the host's units and the spec say the same thing,
the order of the two writers does not matter; the moment they differ, the
symptom is a value that flips every fifteen minutes and a `plan` that is red
whenever it happens to run in the wrong half of the cycle. The host's units
are what a spec replaces, one at a time -- not what it runs beside.

The recorded answers are `tests/fixtures/jellyfin-10.11.11/users.json` (masked:
the nine account names are `konto1` … `konto9`, and what says when somebody
last watched something is removed) and
`displaypreferences-usersettings.json`. `auth-providers.json` is recorded with
them: it is what turns the provider id of a policy into a name.
`GET /Users` carries no credential, but nothing from a body ever reaches an
error here either -- a refused write says the status, the account's id in the
path, and which of the two refusals it was.

## 41. Prowlarr app profiles, custom formats and their scores (2026-09-22)

Three tasks from one inventory of the host, and one thread running through
them: **a collection with two writers**. Recyclarr writes seventy-odd custom
formats into Radarr and Sonarr from TRaSH's templates; the host has one format
of its own, a `3D` rule nobody publishes a template for. Recyclarr cannot
score that format -- its `custom_formats` list takes TRaSH ids, and a format
it does not know is not in it -- and `reset_unmatched_scores`, which is what
keeps a profile clean, sets every score it did not write to **0**. So the
host's own format was worth nothing after every Recyclarr run, and the only
place it could be scored was Radarr's own web interface, by hand.

Hence the split:

| task | services | read | write | desired |
|---|---|---|---|---|
| `app-profiles` | Prowlarr | `GET /api/v1/appprofile` | `POST /api/v1/appprofile`, `PUT …/appprofile/{id}` | `profiles`: name → the four fields |
| `custom-formats` | Radarr, Sonarr | `GET /api/v3/customformat` | `POST …/customformat`, `PUT …/customformat/{id}` | `formats`: name → the renaming switch and the specifications |
| `quality-profiles` (extended) | Radarr, Sonarr | as in §5 | as in §5 | `format_scores`: format name → points |

The host's answer to the second writer is `except` in Recyclarr's
configuration: the format stays out of Recyclarr's reach, converge owns it and
its score, and the two programs stop writing over each other. That is a
decision on the host and not in this program, but it is the reason these three
tasks exist at all.

### Prowlarr's app profiles

An app profile says how Prowlarr searches an indexer: `enableRss`,
`enableAutomaticSearch`, `enableInteractiveSearch`, and `minimumSeeders`, the
number below which a torrent is not handed on to Radarr or Sonarr. Prowlarr
ships exactly one (`Standard`, id 1) and offers no way of writing another but
the API -- so a second one is always something somebody clicked together, and
nothing in a repository says what it holds.

- **Found by name.** `id` and `name` are therefore not settable: the name is
  how a profile is found, the id is the service's. A spec naming another field
  is refused with the four that exist.
- **A missing profile is added** with `name` and the fields the spec gives
  (`app profile Wenig Seeder: (missing) -> (added)`). An existing one is
  written **whole**, as it was read, with the named fields changed -- so a
  field a later Prowlarr adds travels back untouched.
- **The write takes 200 and 202**, the update being `202 Accepted` like every
  other Servarr update; the engine reads back either way.
- `AppProfileResource` is a wire type with all six of the component's fields,
  so `schema-check` compares them, and a spec's fields go through
  `check_paths` on top: `"minimumSeeders": "one"` fails at build time.

### An indexer's app profile by name

`IndexerResource.appProfileId` is an id, and an id in a spec is the wrong one
after the database is rebuilt -- the same argument as for a root folder's
default profiles (§11) and for Seerr's quality profile and root folder (§17).
An `indexers` spec may therefore carry `app_profile` with the profile's
**name** instead:

```json
{ "service": "prowlarr", "task": "indexers", "…": "…",
  "desired": { "providers": {
    "TNTracker": { "implementation": "Torznab", "app_profile": "Standard",
                   "set": { "enable": true, "priority": 25 } } } } }
```

- **Naming both is a spec error** (`app_profile names the profile and
  appProfileId its id -- name one of them`). There would be nothing to decide
  which one wins, and a silent winner is worse than a refusal.
- **Only a Prowlarr `indexers` spec may carry it.** No other provider resource
  has an `appProfileId` at all, so a `download-clients` or `applications` spec
  with one is refused rather than ignored.
- **The list is read only when a spec names a profile**, as the tag list is
  (§13) and the profile lists for a root folder are (§11). A spec that gives
  the id never asks for it.
- **A name the service does not have fails before the first write**, naming
  the indexer and the name and nothing from the answer:
  `not found on the service: indexer TNTracker: app_profile names "Gibt es
  nicht", which the service does not have`. It is raised in `diff`, so `plan`
  says it too.

### Custom formats

A custom format is a name, `includeCustomFormatWhenRenaming`, and a list of
specifications -- each a rule (`ReleaseTitleSpecification` with a regular
expression, `ResolutionSpecification` with a number) that can be negated and
required, with `fields` entries that depend on the implementation.

- **Formats the spec does not name are left alone, and `exactly` is refused
  for this task.** This is the whole point: "the rest" here is Recyclarr's
  seventy formats, and a green run that took them away would be a disaster
  nobody asked for. They are **counted** in the note rather than listed --
  `not in the spec, left as they are: 78 other formats` --, because
  seventy-eight lines saying "left as it is" bury the one line that says
  something.
- **Within a named format the spec is complete.** Its specification list is
  compared by name: one the spec does not name is `custom format 3D:
  specification Old: (present) -> (removed)` and is gone from the body that is
  written. There is no second writer inside a format converge owns, so there
  is nothing to protect there.
- **An existing specification is written as it was read**, with
  `implementation`, `negate`, `required` and the named `fields` values
  changed; `id`, `implementationName`, `infoLink` and a `select` field's
  option list travel back untouched. A **new** one is built from the spec
  alone (`{name, implementation, negate, required, fields: [{name, value}]}`),
  which is what the service fills the rest in from.
- **A `fields` name the specification does not have is an error before any
  write** (`the answer has no such field: custom format AV1: specification
  AV1: fields.regex`). Without it the value would be dropped silently, and the
  run would end green on a format that matches nothing.
- **`fields` have no schema**, as for providers (§12): `Field.value` is
  untyped in the description. So `schema-check` compares the format's switch
  against `CustomFormatResource` and each specification's four named fields
  against `CustomFormatSpecificationSchema` (with `check_objects`, §6), and
  the `fields` entries are checked against the answer at runtime only.
- **The switch is required in the spec.** Leaving it out would write the
  service's default, and which one that is nobody would have decided -- the
  same reason Ghostfolio's `ENABLE_FEATURE_AUTH_TOKEN` is written out rather
  than left absent.

### Scores, in the profile

A format's score does not live on the format; it lives in each quality
profile's `formatItems`, one entry per format, with the format's `id` in
`format` and its name in `name`. `desired.format_scores` on a
`quality-profiles` spec names formats and points:

```json
{ "service": "radarr", "task": "quality-profiles", "…": "…",
  "desired": { "allow_in_every_profile": ["Unknown"],
               "format_scores": { "3D": -10000 } } }
```

- **Every profile, or every kept one.** Without `keep` the score must hold in
  every profile; with `exactly` and `keep` (§39) only in the profiles that
  survive -- a profile on its way out is not measured and not written, exactly
  as for `allow_in_every_profile`.
- **Found by name, written by id.** `format` is the service's id and stays
  where it is; only `score` is ever changed, and every other `formatItems`
  entry travels back as Recyclarr left it.
- **A format the service does not have is an error before anything is
  written**, naming the profile and the format: `not found on the service:
  Dual Language, sonst Deutsch (1080p): custom format Dolby Vision HDR10+`.
  `custom-formats` is what creates it, and **the order of the two specs is the
  host's to arrange** -- converge runs the specs it is given, in order, and a
  failing one does not stop the next (§13).
- `QualityProfileResource` grew `formatItems` as a typed field and
  `ProfileFormatItemResource` beside it, so `schema-check` descends into the
  list (§5) instead of letting it pass as "an array".

### What the recordings say, and what they cost

`tests/fixtures/{radarr,sonarr}-*/customformat.json` are **reduced, not
masked**: the answers held 78 and 60 formats and about 700 KB, most of it the
`selectOptions` list every `select` field repeats. Five formats are kept byte
for byte, chosen to cover one specification and two, a regular expression and
a `select` value, and `negate` both ways. `SOURCE.md` says so, and says what
the count means when a test reads "5 other formats" where the service would
say 78.

The profile recording gave one finding worth keeping: **not one of Radarr's
78 formats carries the same score in all five profiles, except fifteen that
are 0 everywhere, and Sonarr's 60 carry no uniform score at all.** That is
what two families of profiles over one library look like, and it is why the
"nothing to do" test names `AMZN` and `0` rather than a format one would have
guessed.

## 42. Lidarr: quality and metadata profiles, declared (2026-09-22)

A Lidarr root folder names its default profiles by name (§11), and that is
all the host ever said about them: `Standard` for both. What a name *means* is
whatever Lidarr shipped — the same shape of hole as Ghostfolio's
`ENABLE_FEATURE_AUTH_TOKEN` on 2026-09-10, where a check demanded the
*absence* of a setting and so pinned an open door. These two tasks write the
contents down, as a guard: the values as they are, so a change to them is a
change somebody made.

| task | read | write | desired |
|---|---|---|---|
| `quality-profiles` (Lidarr) | `GET /api/v1/qualityprofile` | `PUT /api/v1/qualityprofile/{id}` | per profile: `upgrade_allowed`, `cutoff` by name, `allowed` qualities |
| `metadata-profiles` | `GET /api/v1/metadataprofile` | `PUT /api/v1/metadataprofile/{id}` | per profile: the primary and secondary album types and the release statuses allowed |

```json
{
  "service": "lidarr",
  "base_url": "http://10.0.30.10:8686",
  "api_key_credential": "lidarr-api-key",
  "task": "quality-profiles",
  "desired": { "profiles": {
    "Standard": {
      "upgrade_allowed": false,
      "cutoff": "Low Quality Lossy",
      "allowed": ["MP3-192", "OGG Vorbis Q6", "AAC-192", "WMA", "MP3-224"]
    } } }
}
```

### One task name, two tasks

`quality-profiles` already exists for Radarr and Sonarr (§5), and it means
something else there: *allow this one quality in every profile*, because the
profiles themselves belong to Recyclarr. Lidarr has no Recyclarr; its three
profiles are the ones it shipped with, and the spec names them one by one.
The parser tells the two apart by service, as it already does for `settings`
(bindery, authentik) and `indexers` (Prowlarr, bindery). Radarr and Sonarr
have no metadata profiles at all, so `metadata-profiles` belongs to Lidarr
alone.

### The ladder, and why `allowed` names only qualities

Lidarr's `items` is two levels deep: a rung is either a quality
(`quality.name`, no `name`) or a **group** with `items` of its own (`name`
and `id`, no `quality`). `allowed` names qualities, never groups, and a group
follows the qualities it holds: it is allowed exactly when one of them is.
There is no way to say "the group yes, one of its qualities no" — Lidarr's
own interface cannot say it either, and a spec that could would have two ways
to write the same state.

### The cutoff is a name in the spec and an id on the wire

`cutoff` holds an **id**, and which id depends on what the rung is: a
quality's `quality.id` (`Any` holds 0, which is `Unknown`) or a group's own
`id` (`Standard` holds 1002, which is `Low Quality Lossy`). Ids are database
rows; a rebuilt instance hands out different ones, so the spec says the name
and converge resolves it on every run. A name that matches no rung is an
error, and so is one that matches two — the cutoff would then be a guess.

A cutoff the profile does not allow is refused before the `PUT`: Lidarr
refuses it as well, and a spec that allows a set of qualities and cuts off
outside it says two things at once.

### What fails before anything is written

- **A quality the profile does not have**, named: `profile Standard: it has
  no quality "MP3-321"`. Lidarr's ladder holds every quality it knows, so
  this is a typo or a version that renamed one.
- **A profile Lidarr does not have** — `metadata profile Bootlegs, which
  converge does not create`. Adding one is a decision about what the service
  offers, not a reconciliation; and a name that is only *almost* right would
  otherwise be created next to the one it was meant to be.
- **An unknown album type or release status**, named with its list:
  `secondaryAlbumTypes has no entry "Bootleg"` — `Bootleg` is a release
  status, and the three lists have names that read alike.

All of it is collected first and reported together, and **no profile is
written while anything at all is wrong**: half a list of profiles is worse
than none. Neither task takes `"exactly": true` (§39); converge removes no
profile.

### Writing

One `PUT` per profile that differs, carrying the **whole profile as the
service sent it** with the flags set — `formatItems`, the scores and every
id travel back untouched, groups keep their shape, and a quality gains no
`name` key nor a group a `quality` one. Servarr answers `202 Accepted`, so the
engine reads back. A profile that already agrees is not written.

Changes name the rung they are about, so a plan reads as the profile does:

```
lidarr quality-profiles: note: not in the spec: profile Any
lidarr quality-profiles: would change profile Standard: cutoff Low Quality Lossy -> MP3-320
lidarr quality-profiles: would change profile Standard: items.High Quality Lossy.allowed true -> false
lidarr metadata-profiles: would change metadata profile Standard: primaryAlbumTypes.EP.allowed false -> true
```

### The schema check, and the one field it does not follow

Both profiles are typed, so `schema-check` compares them with Lidarr's
description down to the nested components the metadata lists carry
(`ProfilePrimaryAlbumTypeItemResource.albumType` → `PrimaryAlbumType`). One
field is left out on purpose: `QualityProfileQualityItemResource.items`, a
group's qualities, is a list of **that same component**. Following it would
descend into itself forever, and everything it would compare down there is
what is compared one level up.

The two list endpoints are `servarr::V1`'s, which the root folder task
already declares; naming them again would check the same endpoint twice.
Lidarr's wire types for both components used to be the two fields the root
folder lookup needs (`id`, `name`) — those are gone from the list, because
`schema-check` finds a component by name and would have compared whichever of
two same-named types came first.

Recorded answers: `tests/fixtures/lidarr-3.1.0.4875/{qualityprofile,metadataprofile}.json`
(re-recorded 2026-09-22 and byte-identical to the ones from 2026-09-13 apart
from key order, so they stayed as they were).

## 43. Dispatcharr: a fixed channel number, in the override (v0.40.0, 2026-09-26)

Dispatcharr numbers the channels of an auto-synced group itself, in the order
the provider lists their streams (`fixed` mode, from the group's start), and
renumbers them on every refresh "to maintain sort order". A client tunes by
that number. Replacing the provider would renumber 71 channels. So
`channel-epg` takes, per channel, an optional `channel_number` and holds it
where the sync does not write: the channel's **override**, like the guide
entry, the name and the logo (§34, §35).

What Dispatcharr v0.31.0 does with it, read in its source (tag `v0.31.0`):

- **The override's number is the effective one.** `ChannelOverride` has a
  nullable `channel_number` (`FloatField`, `apps/channels/models.py`, line
  1039); `with_effective_values` coalesces every overridable field, the
  number among them, override first (`apps/channels/managers.py`, lines
  17–55). The M3U output reads `effective_channel_number` and writes it as
  `tvg-chno` (`apps/output/views.py`, lines 340–379), and sorts by it (line
  226); the Xtream Codes output reads the same annotation (`_xc_live_streams_setup`,
  lines 686–746).
- **The sync never writes the override.** It renumbers `Channel.channel_number`
  only (`apps/m3u/tasks.py`, lines 2545–2598), and the model says so
  ("Sync writes only to Channel.* fields and never to this table",
  `models.py`, line 1027).
- **The sync steps around a held number.** Before numbering, it adds every
  override's `channel_number` to the numbers it will not hand out ("Override
  pins are global reservations", `tasks.py`, lines 2107–2117) -- in each mode:
  `fixed` and `next_available` take the next free number, `provider` falls
  back into its range when the provider's number is held (`_pick_target_number`,
  lines 1933–1961). Compact numbering does the same (`compact_numbering.py`,
  `build_reserved_set`), and so does a channel created by hand
  (`Channel.get_next_available_channel_number`, `models.py`, line 428).
- **A number may be shown twice.** The override serializer's help text says so
  ("Duplicate channel_number values across channels are permitted",
  `serializers.py`, line 445), and nothing refuses it; only its lower bound is
  checked (`min_value=0.0001`, line 349). `Channel.clean` asks for a unique
  number per group, but neither the bulk edit nor the serializer calls it.
- **The write.** The bulk edit already used here takes `channel_number` in
  `override` as it is (`OVERRIDABLE_FIELDS`; `api_views.py`, `edit_bulk`,
  lines 1370–1432) and changes nothing else of the override. A plain
  `channel_number` next to `override` would be the channel's OWN number, the
  one the next refresh renumbers.

So the collision resolves itself only for the channels of an account that
syncs: its next refresh moves them off every held number. A channel created
by hand, or one another override holds on that number, stays where it is. A
number held here that another channel shows is therefore a **note**, not a
refusal -- once this task has written, so a channel the same run moves away
does not count. Two channels of the same spec with the same number are
refused when the spec is read, and so is a number below 0.0001.

The number is compared as `effective_channel_number` -- a number in the
answer (recorded: `313.0`), although the description declares every
`effective_*` field a string -- and a change line writes it the way the
output does: `300 -> 71`, `71.5`. `schema-check` checks a spec's numbers
against `ChannelOverride.channel_number`; the other fields of the task are
names, or aliases (`epg_data`, `logo`) the bulk edit maps onto the override's
`*_id` columns and that no component carries.

Not measured on the host yet: that the next refresh of the account leaves
the effective number where it is and moves the channel that showed it.

## 44. Dispatcharr: the profiles of an account (v0.41.0, 2026-09-26)

`m3u-profiles` sets the profiles of M3U accounts: account name -> profile
name -> `max_streams`, `is_active`, `search_pattern`, `replace_pattern`. A
missing profile is added; one the spec does not name is a note, never a
removal.

**Why the host needs it.** An Xtream provider that reports three
connections counts "devices" by address AND user agent: a second stream from
the same address with the same agent throws the first one out (measured on
the host, 2026-09-26). Dispatcharr sends ONE agent per account
(`M3UAccount.user_agent`). The host therefore runs a small forwarder with one
local port per agent, and the account gets one profile per port, each
limited to one stream, each rewriting the stream URL onto its port.

What Dispatcharr v0.31.0 does with a profile, read in its source (tag
`v0.31.0`):

- **Which profile a stream gets.** `Channel.get_stream` takes the account's
  ACTIVE profiles, the default one first and the others after it in the
  order the database answers, and reserves the first with a free slot
  (`apps/channels/models.py`, lines 694–800). Without an active default
  profile the account is skipped altogether ("has no active default
  profile"). A profile's limit is its own `max_streams`; 0 is unlimited.
- **How the URL is rewritten.** For an Xtream Codes account the pattern is
  not applied to a stream's URL but to a synthetic one,
  `<server_url>/live/<user>/<password>/1234.ts`
  (`get_transformed_credentials`, `apps/m3u/credentials.py`, lines 31–150);
  from the result it takes user and password from the path and scheme, host
  and any base path as the server, and builds the playback URL from those
  (`_resolve_live_stream_url`, `apps/proxy/live_proxy/url_utils.py`,
  line 21). A pattern that does not match fails the stream. So
  `^https?://[^/]+` -> `http://10.88.0.1:9201` moves the server and keeps
  the login -- no back-reference needed. Where one is, the replacement is
  JavaScript-style: `$1` and `$<name>` become `\1` and `\g<name>`
  (`_js_replace_to_python`, line 25). The same transform serves the account
  information of each active profile (`refresh_account_profiles`,
  `apps/m3u/tasks.py`, line 3127), so that request takes the rewritten way
  too.
- **Redirects.** The live proxy fetches with `requests` and its default,
  `allow_redirects=True` (`http_streamer.py`, line 67). A forwarder that
  passed a provider's `302` on would see the stream leave past it, with
  Dispatcharr's own agent; it has to follow the redirect itself.
- **The default profile is not the spec's to limit.** Dispatcharr makes it
  with the account (`<account> Default`, `create_profile_for_m3u_account`,
  `apps/m3u/models.py`, line 377) and copies the account's `max_streams` to
  it on every save that changes it; the serializer refuses everything but
  name, custom properties, expiry and the two patterns on it
  (`M3UAccountProfileSerializer.update`, `apps/m3u/serializers.py`, line
  94). A spec that names `max_streams` or `is_active` for it is refused
  before anything is written -- it would be an endless difference -- and
  the refusal says to set the limit on the account (`m3u-accounts`).
- **A new profile needs both patterns** (`validate`, line 74); a spec that
  would add one without them is refused before the write, with the field it
  lacks.

Read through `GET /api/m3u/accounts/`, where every account carries its
profiles (recorded); written by `POST /api/m3u/accounts/{account_id}/profiles/`
with the name and the spec's fields (`perform_create` takes the account from
the path) and `PATCH .../profiles/{id}/` with the spec's fields only.
Readiness waits until every named account exists; its default profile exists
with it, made in the same transaction.

Only the four fields are compared and shown. A profile's
`custom_properties` hold what the provider answered about the account
(`user_info`, with the login) and are never read here. `schema-check` checks
the fields against `M3UAccountProfile` and `PatchedM3UAccountProfile`.

Not measured on the host yet: three streams at once, one per profile, and a
fourth refused.

## 20. Not in the pilot


- Other services and tasks.
- TLS, JSON output, a NixOS module.
- Deleting things, **except where a spec asks for it** (§39, v0.36.0): with
  `"exactly": true`, Trailarr's connections (§9), Koel's radio stations (§18)
  and Radarr's and Sonarr's quality profiles (§5) lose what the spec does not
  name. `custom-formats` (§41) refuses it on purpose: its collection has a
  second writer, and "the rest" there is Recyclarr's seventy formats.
  Without that switch -- and for every other task, which refuses it --
  `converge` only sets what the spec names, and appends
  list entries (§8), connections (§9), subscriptions (§10), root folders (§11, §14), providers (§12, §13), tags (§13), bindery's entries (§14), Seerr's servers
  (§17), Koel's radio stations (§18) and Audiobookshelf's libraries (§38) it is responsible for. SuggestArr's
  configuration (§19) is a document, not a list: converge sets the named
  fields and carries every other one back unchanged.
