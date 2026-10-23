# Source of these fixtures

Recorded 2026-10-23 around 17:00 CEST from Whisparr Eros 3.6.4.1865 -- the
release asset `Whisparr.eros.3.6.4-release.1865.linux-x64.tar.gz` of
github.com/Whisparr/Whisparr-Eros, tag `v3.6.4-release.1865` (commit
`61cf11c943fbc4c2556c502b2dd0e738ec9f5b66`, archive sha256
`9258dd3cfb2abc2ee113c7c54b090e316f687a835adffb63443ef015812f30fd`), run on
the workstation with a fresh data directory, bound to 127.0.0.1, with a
made-up API key that travelled in a curl config file, never in `argv`.

Before recording, converge itself (this branch, design §59) applied the
specs the host will use -- naming, media management, download-client config,
one root folder, SABnzbd and qBittorrent as download clients and one ntfy
notification -- and a second `plan` answered 0. The download clients and the
notification were created **disabled** (`enable: false`, no `on*` event):
Eros tests an enabled provider on every create, `forceSave=true` or not, and
no SABnzbd or qBittorrent was listening.

Every file is `GET /api/v3/<path>`, verbatim unless said otherwise:

- `system-status.json` -- `system/status`. `version` is `3.6.4.1865`,
  `branch` `eros`; `startupPath` and `appData` are the workstation's.
- `rootfolder.json` -- `rootfolder`, the one folder converge added (a
  directory under the workstation's scratch space).
- `downloadclient.json` -- `downloadclient`, the two clients converge added.
  SABnzbd's `apiKey` and qBittorrent's `password` are answered `********`;
  the values sent were made up for this run.
- `downloadclient-schema.json` -- `downloadclient/schema`, **cut down** to
  the two templates the host uses:
  `jq '[.[] | select(.implementation=="Sabnzbd" or .implementation=="QBittorrent")]'`.
  The answer holds eighteen templates and 68 kB.
- `notification.json` -- `notification`, the ntfy entry converge added.
- `notification-schema.json` -- `notification/schema`, **cut down** to
  `jq '[.[] | select(.implementation=="Ntfy")]'` (28 templates, 93 kB).
- `config-downloadclient.json` -- `config/downloadclient`.
- `mediamanagement.json` -- `config/mediamanagement`, after converge set
  `deleteEmptyFolders`.
- `naming.json` -- `config/naming`, after converge set `renameMovies`,
  `renameScenes` and `colonReplacementFormat`. Eros has six naming fields
  Radarr lacks (`renameScenes`, `standardSceneFormat`, `sceneFolderFormat`,
  `sceneImportFolderFormat`, `maxFilePathLength`, `maxFolderPathLength`).

None of them carries a credential; each was looked at as a shape before it
was copied here.
