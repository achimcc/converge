# Source of these fixtures

Recorded 2026-09-13 from a running ntfy 2.26.0 (nixpkgs package) on the host
this program was written for:

- `account.json` — `GET /v1/account` for a user account (role `user`).

**This is masked, not verbatim.** Topic names grant read access, and the
account object carries tokens. Before the answer left the host:

- the two subscribed topics were replaced by `<masked-topic-1>` and
  `<masked-topic-2>`;
- `sync_topic` and `username` were replaced by `<masked>`;
- every string field of `tokens` was replaced by `<masked>`.

Keys are in sorted order. Everything else — limits, stats, role,
`base_url` and `display_name` of the subscriptions — is as recorded.

Tests use a topics credential whose lines are `<masked-topic-1>` and
`<masked-topic-2>`, so the recorded state is "already desired".

ntfy publishes no OpenAPI description; this recording and the runtime checks
are all that stand behind the field names (`docs/design.md` §10).

## Constructed

- `constructed-health.json` — `{"healthy": true}`, the shape of
  `GET /v1/health`, which needs no token. Built by hand to match the live
  answer rather than recorded.
