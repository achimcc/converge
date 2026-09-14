# Source of these fixtures

Koel 9.11.3 (nixpkgs package with the host's patches, NixOS 26.05) inside a
systemd-nspawn container. Recorded 2026-09-14 around 18:15 CEST with **GET
requests only**, through the container's own web server, with a Sanctum
token created for the library account (an administrator) for this recording
and deleted right after it (`tokens_nachher=0`). The token never left the PHP
process that made the requests.

## Recorded

- `radio-stations.json` — `GET /api/radio/stations` with
  `Authorization: Bearer <token>` and `Accept: application/json`: HTTP 200,
  `Content-Type: application/json`, the body `[]`. The instance had no radio
  station at the time, so the recording shows the **shape** of the list only:
  a bare JSON array, no `data` wrapper (`JsonResource::withoutWrapping()` in
  `app/Providers/AppServiceProvider.php`).
- `unauthenticated.json` — the same request without a token, with
  `Accept: application/json`: HTTP 401, `{"error":"Unauthenticated."}`.

Also measured, not kept as a file: `GET /api/ping` answers HTTP 200 with an
empty body and `Content-Type: text/html` (no token needed); without
`Accept: application/json`, a request without a token is answered
**302 to `/`** (measured by the analysis before, same day), which an HTTP
client that follows redirects turns into the web page with HTTP 200.

## Constructed

- `constructed-radio-stations.json` — three stations, built from
  `app/Http/Resources/RadioStationResource.php` (`toArray`: `type`, `name`,
  `id`, `url`, `homepage_url`, `logo`, `description`, `is_public`,
  `created_at`, `favorite`, `permissions.edit`, `permissions.delete`), since no
  station existed to record and creating one on the live instance was out of
  scope. Values follow the code: `id` is a ULID (`HasUlids`), `logo` is
  `image_storage_url()` of the stored file name (a URL under
  `storage/images/`) or `null`, `description` is `''` when a station was
  created through the API without one (`$this->string('description')`) and
  may be `null` for rows created otherwise, `homepage_url` is `null` when not
  set. The third station stands for a person's public station that the
  library account sees with `include_public_media` (its default, measured
  `true`).
- `constructed-validation-error.json` — Laravel's JSON answer to a failed
  form request (`Illuminate/Foundation/Exceptions/Handler.php::invalidJson`:
  `message`, `errors` by field), with messages from Laravel's `url` rule and
  `app/Rules/ValidImageData.php`. Provoking a real one needs a `POST`, which
  this recording did not make.
