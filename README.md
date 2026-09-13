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

Early. **v0.1.0** — one task: the quality size limits of **Radarr** and
**Sonarr** (API v3). Written for one host, where it replaces a shell unit.
More tasks come only after this one has been evaluated.

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

## Commands

| command | does | exit |
|---|---|---|
| `converge apply [--deadline <s>] <spec>...` | reconcile, write, read back | 0 done, 1 any spec failed |
| `converge plan [--deadline <s>] <spec>...` | show what `apply` would change | 0 equal, 2 differs, 1 error |
| `converge schema-check --service <radarr\|sonarr> --openapi <file>` | compare the wire types with an OpenAPI file | 0 / 1 |

Several specs are processed in order; one failing does not skip the next.

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
