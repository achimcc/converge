# CLAUDE.md

Working rules for this repository. `README.md` says what the program does,
`docs/design.md` why it is shaped the way it is.

## Language

**This repository is English** — identifiers, comments, commit messages,
README. Its author usually writes German; a tool other people could use is
unusable for most of them in German.

## The test cycle

1. **Write the test first** and see it red for the reason you expect.
2. **Implement the smallest thing that passes.**
3. **Run all three, every time** (inside `nix develop`): `cargo test`,
   `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`. A green
   test run with clippy or fmt skipped is not a green build.
4. **Commit**, staging files by name — never `git add -A`.

`nix flake check` builds the package and runs clippy and fmt in the sandbox;
run it before tagging.

## Rules that hold everywhere

- **Ask for the result, not the exit code.** `202 Accepted` is not "saved";
  the engine reads back until the value is there. A test that asserts only a
  status code proves nothing about the state.
- **No secret in an error, a log line or `argv`.** The API key lives in
  `Secret`, which has no `Debug` and no `Display`, and travels only as the
  `X-Api-Key` header. Error bodies from a service are never printed whole —
  only named validation fields.
- **Field names come from a recorded answer or the OpenAPI file, never from
  memory.** Wire types declare only what the program reads or writes;
  everything else is kept in a flattened map and written back untouched.
  A new wire type is named exactly like its OpenAPI component, so
  `schema-check` finds it.
- **A nullable field may also be absent.** Radarr omits null values; absent
  and `null` are the same value here.
- **An empty answer is an error, not "nothing to do".** A check over an empty
  set is green for the wrong reason.
- **Fixtures say where they come from.** Recorded answers carry a
  `SOURCE.md`; anything built by hand is named `constructed-…`.
