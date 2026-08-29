# Changelog

## 0.1.0 — 2026-07-31

Private alpha.

### Included

- Native Aseprite extension browser and installed-extension view.
- Public GitHub release and immutable repository-snapshot resolution.
- Linked local development folders with repeatable Snapshot and Sync.
- Update checks for linked local folders and tracked GitHub sources, including
  same-version repository-snapshot changes and per-package check errors.
- Strict archive, manifest, contribution, size, and native-content validation.
- Managed receipts with current and previous artifact caching.
- User-confirmed Restore through Aseprite's extension installer.
- User-confirmed manager updates from the canonical GitHub release and cached
  rollback, with a restart boundary and a verified recovery package.
- Authenticated loopback helper protocol with an idle timeout.
- TUF-authenticated empty catalog and offline last-known-good handling.
- macOS universal, Windows x86_64, and Linux x86_64 helper builds.

### Alpha limitations

- The bundled catalog intentionally contains no third-party packages.
- Private GitHub repositories and GitHub Enterprise are not supported.
- Dependencies, install hooks, silent updates, and bulk updates are not
  supported.
- Enable, disable, and uninstall are completed in Aseprite's native
  Preferences window.
- Alpha helper executables are unsigned. Public distribution is blocked on
  macOS signing and notarization and Windows Authenticode signing.
