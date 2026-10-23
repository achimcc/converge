# Source of these fixtures

**Every file here is constructed, none is recorded.** No Velvarr instance was
running, and a setup needs a Jellyfin behind it. The answers are built from
Velvarr's source at the release `v0.39.3`, commit
`0b94af625c31e6fa7be03d90cafd0596ae76dade` of github.com/Skare69/velvarr
(`package.json` line 3: `"version": "0.39.3"`). Ids are made up in the forms
Velvarr checks (32 lower-case hex digits, `JELLYFIN_ID` in
`src/app/api/[...path]/parse.ts` line 20; `normalizeItemId`,
`src/server/jellyfin.ts` line 121). Paths below are relative to `src/`.

Every answer goes through `json()` (`app/api/[...path]/admission.ts`
line 32): `JSON.stringify` of the object, `content-type: application/json`.
A refusal is `{"error":{"code","message"}}` (`errorResponse`,
`app/api/[...path]/route.ts` line 71); an `AppError` with status 401 whose
code is not `unauthenticated` or `session_revoked` leaves as **502**
(line 78).

| file | route | built from |
|---|---|---|
| `constructed-health.json` | `GET /api/health` | `app/api/[...path]/routes/auth.ts` line 377 |
| `constructed-status-fresh.json`, `-set-up.json`, `-not-ready.json` | `GET /api/status` | `routes/auth.ts` line 365, `setupReady` line 46 |
| `constructed-setup-inspect.json` | `POST /api/setup/inspect` | `routes/auth.ts` lines 197-200; `Library`, `lib/contracts.ts` line 648 |
| `constructed-setup.json` | `POST /api/setup` | `routes/auth.ts` lines 255-258 (`{account}`, `Set-Cookie`); `Account`, `lib/contracts.ts` lines 615-626; the owner row, `server/storage.ts` `bootstrap` line 1236 |
| `constructed-login.json` | `POST /api/login` | `routes/auth.ts` lines 298-301 |
| `constructed-error-upstream-auth.json` | a refused password, HTTP 502 | `server/jellyfin.ts` lines 249-253 and `route.ts` line 78 |
| `constructed-error-origin-mismatch.json` | a wrong `Origin`, HTTP 403 | `server/security.ts` lines 150-155 |
| `constructed-error-too-many-attempts.json` | the login limiter, HTTP 429 | `server/security.ts` lines 55-61 |
| `constructed-error-already-initialized.json` | a second setup, HTTP 409 | `routes/auth.ts` lines 204-209 |
| `constructed-integrations.json`, `-delivery.json`, `-no-whisparr.json` | `GET /api/admin/integrations` | `integrationsShape`, `routes/admin.ts` lines 45-75; `judgmentConfiguration`, `server/judgment.ts` lines 44-54; `WhisparrDelivery`, `WhisparrPathMapping`, `lib/contracts.ts` lines 552-562 |
| `constructed-whisparr-status.json`, `-error.json` | `GET /api/admin/whisparr` | `getWhisparrStatus`, `server/whisparr.ts` lines 46-91; the error form, `routes/admin.ts` lines 482-487 |
| `constructed-users.json`, `-before-import.json` | `GET /api/admin/users` | `adminUsers`, `routes/admin.ts` lines 91-112; `AdminAccount`, `lib/contracts.ts` line 641; `rowToAccount`, `server/storage.ts` line 831 |
| `constructed-users-import.json` | `POST /api/admin/users/import` | `adminImport`, `routes/admin.ts` lines 133-140; a new row is a disabled `requester` without libraries, `server/storage.ts` lines 1285-1293 |
| `constructed-user-updated.json` | `PATCH /api/admin/users/:id` | `adminUpdateUser`, `routes/admin.ts` lines 142-172 |

The session cookie the tests set is the line `sessionCookie` writes
(`server/security.ts` line 275):
`velvarr_session=<token>; Path=/; HttpOnly; SameSite=Strict; Max-Age=<s>`.

**To be replaced by recordings** after the first run against a real
Velvarr 0.39.3: every file above. What the source cannot settle and a
recording must: the key order of each answer (irrelevant to converge, which
reads by name), whether a proxy in front adds headers, and the exact
`Set-Cookie` line Next.js sends.
