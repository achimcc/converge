# Source of these fixtures

Recorded 2026-09-19 around 05:50 CEST from a throwaway container of
`ghcr.io/dispatcharr/dispatcharr:0.31.0@sha256:f81924fa3dbfeb463b3908be7e086bf58aacbd2ba56062bfb555ee3a471acf8f`
(all-in-one image, `--cap-drop=ALL` plus SETUID, SETGID, CHOWN, DAC_OVERRIDE
and FOWNER, published on 127.0.0.1 only), verbatim:

- `core-version.json` — `GET /api/core/version/`
- `core-streamprofiles.json` — `GET /api/core/streamprofiles/`
- `core-settings.json` — `GET /api/core/settings/`
- `m3u-accounts.json` — `GET /api/m3u/accounts/`
- `channels-groups.json` — `GET /api/channels/groups/`
- `epg-sources.json` — `GET /api/epg/sources/`

each with a bearer token from `POST /api/accounts/token/` passed in a curl
config file, not in `argv`.

At recording time the instance had been set up by hand the way the host's
specs describe it: the default stream profile `streamlink`, an M3U account
`Oeffentlich-rechtlich` reading `/data/sender.m3u` with its group
`Öffentlich-rechtlich` enabled and auto-synced to channels 1 to 99, and the
EPG source `epgshare01-de` (XMLTV). Both refresh intervals were still 0.

Checked before committing: the instance held no credential of anybody's. The
account and the source have `username` null and `password` empty; the only
field whose name looks like one, `m3u_hash_key`, is the name of the stream
field Dispatcharr hashes (`url`).

## Recorded later from the host (2026-09-19, around 12:00 CEST)

From the host's running instance (same image digest), logged in as the
service account `converge`, the token in a curl config file:

- `channels-channels.json` — `GET /api/channels/channels/` without
  `page_size` (a plain list; with `page_size` the answer is paginated),
  verbatim: 78 auto-created channels, 18 from the file account and 60 from an
  Xtream Codes account. Names, numbers, tvg-ids and stream ids only; the
  channel list carries no stream URL and no credential.
- `epg-epgdata-trimmed.json` — `GET /api/epg/epgdata/`, **trimmed**: the
  answer held 4687 entries (745 KB). Kept are all 266 entries of source 1
  (`epgshare01-de`) and the entries of source 2 (the Xtream provider's guide)
  that a channel above points to; nothing inside an entry was changed.

`GET /api/epg/sources/` was NOT recorded again: the Xtream guide's source
carries the provider's user name and password in its `url`. The tests use
`epg-sources.json` above, with source 2 added in the test code.
- `channels-logos-page1.json` — `GET /api/channels/logos/` without
  parameters (recorded 2026-09-19, 12:10 CEST, verbatim): the FIRST page of a
  paginated answer (50 of 192, `next` set) -- the logo list is paginated
  whatever is asked. Logo names and image URLs only (the public broadcasters'
  logos from tv-logo/tv-logos and the provider's channel pictures). The
  second page the tests need is constructed in the test code.
- `channels-streams-trimmed.json` — `GET /api/channels/streams/?page_size=1000`
  (recorded 2026-09-19, 18:12 CEST), **trimmed**: every stream of the answer
  (265, one page, `next` null), but only `id`, `name`, `m3u_account` and
  `channel_group` of each. Everything else is dropped because an Xtream
  stream's `url` carries the provider's user name and password; `count`,
  `next` and `previous` are kept as answered.

## Constructed (2026-09-29)

- `constructed-vod-categories.json` — what `GET /api/vod/categories/?m3u_account=2`
  answers, **built by hand**: no instance with an Xtream account and VOD
  switched on was at hand to record from, and the host's categories are the
  provider's. The shape is `VODCategorySerializer` of Dispatcharr v0.31.0
  (`apps/vod/serializers.py`, lines 110-131): `id`, `name`,
  `category_type`, `category_type_display` and `m3u_accounts`, the latter
  `M3UVODCategoryRelationSerializer` (`category`, `m3u_account`,
  `enabled`) over EVERY account's relation to the category -- the list
  filter picks categories, not relations, so a category shared with account 3
  carries that account's entry too. The view has no pagination
  (`VODCategoryViewSet`, `apps/vod/api_views.py`, lines 740-820) and orders by
  name. The two `Uncategorized` entries are the ones the list request itself
  creates for an XC account with VOD on (lines 769-818). Names and ids are
  invented.
