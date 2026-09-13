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
- **A flake check in the host** fetches `openapi.json` for the *deployed*
  package's version (`radarr.version`, `sonarr.version`) from the release
  tag, by a hash kept in a table keyed by version. A version without an
  entry is an evaluation error naming the command that produces the hash —
  so a package bump cannot pass without the schema being checked again.
- **Acceptance on the running host**, not on the build: `converge plan`
  against both instances reports no difference after the deploy, and a
  deliberate one-field change in the table shows up as exactly one changed
  line in `apply`, then as no difference in `plan`.

## 5. Not in the pilot

- Other services and tasks (Jellyfin, Authentik, Seerr, …).
- TLS, JSON output, a NixOS module.
- Deleting things. `converge` only sets what the spec names.
