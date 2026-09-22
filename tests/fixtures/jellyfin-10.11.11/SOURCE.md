# Source of these fixtures

Recorded 2026-09-13 around 13:15 CEST from a running Jellyfin 10.11.11
(nixpkgs package, NixOS 26.05) inside a systemd-nspawn container, verbatim:

- `system-info.json` — `GET /System/Info`
- `system-configuration.json` — `GET /System/Configuration`
- `virtual-folders.json` — `GET /Library/VirtualFolders`
- `scheduled-tasks.json` — `GET /ScheduledTasks`

each with

```
systemd-run --machine=<container> --wait --pipe --quiet --collect -- \
  /run/current-system/sw/bin/bash -c \
  'k=$(< <key file>); exec curl -sS -H "X-Emby-Token: $k" http://localhost:8096/<endpoint>'
```

None of these endpoints carries credentials. Checked before committing: no
field name suggesting a password, secret or token; the long values are item
and task ids, the `Key` fields are task names, the rest are paths.

At recording time the service already held the state the host wants:
trickplay extraction on for `Filme` and `Serien`, key-frame-only and hardware
extraction on, and both Merge Versions tasks triggered daily at 05:30
(`TimeOfDayTicks` 198000000000).

## Plugins (recorded 2026-09-13 around 13:50 CEST)

- `plugins.json` — `GET /Plugins`
- `plugins/<id>.json` — `GET /Plugins/<id>/Configuration` for Jellyfin
  Oscars, MDBList Ratings, Ratings, Bazarr, Media Bar, Mediathek Downloader
  and Auto Collections.

**These are masked, not verbatim.** Plugin configurations carry API keys.
Before the answers left the host, every non-empty string field whose name
matches `key|token|secret|passw|auth` (case-insensitive) was replaced with
`"<masked>"` by `jq` on the host itself — four fields in total (`OmdbApiKey`,
`MdbListApiKey`, `TmdbApiToken`, `BazarrApiKey`). The remaining long values
were checked there too: plugin ids and download paths only. Everything else,
including empty strings as `""` (the JSON API does not omit them; the XML
file on disk does), is as recorded.

## SkinManager and JavaScript Injector (recorded 2026-09-13 around 14:16 CEST)

- `plugins/e10fb9d4c9414c6e82602641031c2618.json` — SkinManager
- `plugins/f5a34f7b2e8a4e6aa7223a216a81b374.json` — JavaScript Injector

A second run over every plugin, masked on the host the same way (the pattern
additionally covered `cookie`, `smtp.*user` and `username`) and written with
`jq -S`, so keys are sorted. **No field of these two was masked.** They carry
a theme choice and scripts: the host's own `Skin-Manager-Vorgabe` script
(public in the host repository) and the scripts other plugins register. The
long values were checked before committing: GUIDs, field names and
JavaScript identifiers; every `Token` is a variable name in a script.

GetAvatar was recorded too and deliberately left out: its `UserAvatars` says
which person picked which picture.

## Named configurations (recorded 2026-09-14 around 09:30 CEST)

- `system-configuration-network.json` — `GET /System/Configuration/network`
- `system-configuration-branding.json` — `GET /System/Configuration/branding`

Verbatim, recorded the same way (the key passed to curl with `-K` from a
process substitution instead of `-H`, so it is not in `argv`). The only field
whose name suggests a secret is `CertificatePassword`; it is the empty string,
checked on the host before the answer left it. `LoginDisclaimer` is the host's
own sign-in button (public in the host repository).

Note what the branding answer does NOT carry: `CustomCss` is null on this
instance, and Jellyfin omits null values — the answer has exactly
`LoginDisclaimer` and `SplashscreenEnabled`.

## Live TV (recorded 2026-09-18 around 18:45 CEST)

- `system-configuration-livetv.json` — `GET /System/Configuration/livetv`

Verbatim, recorded the same way, before the host configured any tuner: both
lists are empty, and Jellyfin omits every null field (`GuideDays`, the
recording paths). No field carries a credential; the one string is
`RecordingPostProcessorArguments`, Jellyfin's default.

## LDAP-Auth and SSO-Auth (recorded 2026-09-14 around 14:20 CEST)

- `plugins/958aad6637844d2ab89aa7b6fab6e25c.json` — LDAP-Auth 23.0.0.0
- `plugins/505ce9d1d91642fa86ca673ef241d7df.json` — SSO-Auth 4.0.0.4

**Masked more strictly than the others.** Both carry a password and a
directory layout, so on the host `jq` replaced EVERY string with
`"<masked>"`, except the library ids in `EnabledFolders` and `Folders`
(the same ids as `virtual-folders.json`, checked equal at recording time), the
group names in `Roles`, `AdminRoles` and `FolderRoleMapping[].Role`, and the
three `OidScopes`. The keys of `CanonicalLinks` are account names; they were
renamed `user-1` … `user-7`. Numbers, booleans and the shape are as recorded,
keys sorted (`jq -S`).

Two things these answers show that the XML files on disk do not:
`OidConfigs` is an object keyed by provider name (`authentik`), and the role
mapping is `FolderRoleMapping` (singular, the property name) — the XML file
calls it `FolderRoleMappings`. `PortOverride` is null and therefore absent.

## Accounts and display preferences (recorded 2026-09-22)

- `users.json` — `GET /Users`
- `displaypreferences-usersettings.json` —
  `GET /DisplayPreferences/usersettings?userId=<the account of users.json[0]>&client=emby`
- `auth-providers.json` — `GET /Auth/Providers`

Recorded the same way as the answers above (the key passed to `curl` with
`-K` from a process substitution instead of `-H`, so it is not in `argv`) and
written with `jq -S`, so keys are sorted.

**`users.json` is masked, the other two are verbatim.** `GET /Users` names the
people who use the server: on the host every `Name` was replaced with
`konto1` … `konto9`, and the fields that say when somebody last watched
something or which picture they picked (`LastLoginDate`, `LastActivityDate`,
`PrimaryImageTag`, `PrimaryImageAspectRatio`) were removed. What the committed
file carries is eight keys per account — `Name`, `Id`, `Policy`,
`Configuration`, `EnableAutoLogin`, `HasPassword`, `HasConfiguredPassword`,
`HasConfiguredEasyPassword` — the last four booleans, the `Policy` and the
`Configuration` as recorded. The answer holds no password, key or token;
checked on the host before it left: the long values are account ids and the
library ids of `EnabledFolders` (the same ids as `virtual-folders.json`), and
the two provider ids are class names.

Nine accounts, each with the same 42 policy fields. What they show is what the
host's three shell units set by hand, per account: `AuthenticationProviderId`
on the LDAP plugin for all nine, `EnableAllFolders` false,
`EnableSubtitleManagement` true, `EnableLiveTvAccess` true,
`EnableLiveTvManagement` false. `IsAdministrator` is true for `konto1` alone —
and no unit sets it. `auth-providers.json` is what makes that provider id a
name (`LDAP-Authentication`) rather than a string to be remembered.

`displaypreferences-usersettings.json` is the first account's. `CustomPrefs`
is a sparse string map: it carries `livetv-favoritechannelsattop` as the
string `"false"`, two keys whose value is `null` (`dashboardTheme`, `tvhome`),
and three whose key is a library id and a view name. No key of it names a
credential.
