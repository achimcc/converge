# Source of these fixtures

Recorded 2026-10-23 around 01:35 CEST from a running Jellyfin 12.2.0 (nixpkgs
`nixos-unstable`, NixOS 26.05) inside a systemd-nspawn container, with the
`Authorization: MediaBrowser Token="…"` header (12 no longer takes
`X-Emby-Token` without `EnableLegacyAuthorization`):

- `system-configuration-branding.json` — `GET /System/Configuration/Branding`,
  recorded while a stylesheet WAS set. The answer has `LoginDisclaimer` and
  `SplashscreenEnabled` only: Jellyfin 12 never returns `CustomCss` here.
  The disclaimer text is shortened to a neutral sentence.
- `branding-css.css` — `GET /Branding/Css`, the stylesheet Jellyfin serves
  (an `@import` of the host's own theme; the URL is shortened to a path).

Neither endpoint carries credentials.
