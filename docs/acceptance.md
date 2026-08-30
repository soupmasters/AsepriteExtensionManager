# Preview acceptance

Alpha completion requires passing automated CI on macOS, Windows, and Linux and
one full macOS Aseprite run. It does not claim manual Aseprite certification on
Windows or Linux.

## Automated evidence

The following must pass on the exact branch commit under test:

- Rust formatting, Clippy, dependency policy, licenses, and advisories;
- Rust tests on macOS, Windows, and Linux;
- fake-Aseprite Lua tests;
- TUF metadata regeneration, signature chain, expiry, rollback, freeze,
  corruption, and offline-cache cases;
- GitHub URL, stable release, ambiguous asset, immutable commit, ETag, rate
  limit, fixed `gh` arguments, missing CLI, signed-out CLI, and CLI error mapping,
  including explicit per-package update errors;
- malicious ZIP, path collision, Unicode and space-heavy names, limits,
  interrupted download, atomic receipt, eviction, and restore cases;
- local-folder exclusion, `.aemignore`, snapshot normalization, and repeated
  same-version sync cases;
- tracked GitHub snapshot content changes at an unchanged manifest version;
- canonical manager release selection, exact manager layout and helper-format
  validation, pending-update reconciliation, cancellation, recovery, and
  rollback cases;
- loopback binding, invalid token, protocol mismatch, concurrency, disconnect,
  shutdown, and idle timeout cases;
- native helper builds and smoke tests for every packaged platform;
- deterministic universal archive content, helper modes, and absence of
  fixture key material.

## Isolated macOS setup

Use Aseprite 1.3.18.3 and an empty temporary profile:

```sh
export AEM_ACCEPTANCE_PROFILE="$(mktemp -d)"
ASEPRITE_USER_FOLDER="$AEM_ACCEPTANCE_PROFILE" \
  /Applications/Aseprite.app/Contents/MacOS/aseprite
```

Record the tested commit and artifact SHA-256. Do not reuse a normal Aseprite
profile for destructive failure simulations.

## Manual run

1. Install `aseprite-extension-manager-0.1.0.aseprite-extension`.
2. Open **Aseprite Extension Manager…** and confirm first-run text explains
   exactly the command-execution and localhost WebSocket permissions. Confirm
   no blanket permission is requested.
3. Confirm the resizable nonmodal dialog uses native widgets, Browse shows the
   curated catalog, and Installed shows unmanaged extensions without modifying
   them.
4. Install one curated release asset and one curated repository snapshot.
   Confirm both exact source hashes are authenticated, the snapshot records its
   immutable commit, and each installed receipt records the correct source.
5. Install a direct release-style fixture. Exercise the multiple-asset chooser
   and cancel once before selecting explicitly. Open Help and confirm Git and
   GitHub CLI availability are reported without opening a terminal.
6. Update between two fixture versions. Confirm extension preferences survive,
   installed name/version verification succeeds, and current/previous cache
   artifacts are exact.
7. Attempt corrupt, traversal, colliding, mismatched-manifest, oversized, and
   native-code fixtures. Confirm Aseprite never receives their installer
   prompts.
8. Simulate update verification failure. Accept Restore and confirm the
   previous artifact is offered through Aseprite's installer and then verifies.
9. Select a local development `package.json` and install its snapshot. Confirm
   the receipt links the canonical source path. Edit the source without changing
   its manifest version, refresh, and confirm an update is offered. Sync it
   repeatedly and confirm the source tree is untouched.
10. Install a tracked GitHub repository snapshot, advance the tracked ref while
    keeping the manifest version unchanged, and refresh. Confirm changed
    snapshot bytes are offered once, while identical bytes and downgrades are
    not.
11. Sign in with GitHub CLI and install one private repository, testing both a
    release asset and a repository snapshot when available. Advance its saved
    release or tracked ref and confirm the update is found. Sign out or hide the
    CLI and confirm the package receives a clear update error without losing its
    managed receipt.
12. Make one linked source unavailable and make another fail validation.
    Confirm each package displays its update error, other update checks still
    complete, and neither failure is presented as "up to date."
13. Confirm enable and disable open native Extensions preferences. Uninstall a
    managed extension and a manually copied extension without `__info.json`.
    Confirm each disappears from the manager, its exact folder is moved to
    recovery storage, a uniquely matching receipt is archived, its source
    folder is untouched, and Aseprite requests an immediate restart. Before
    restarting, confirm further install, update, restore, and uninstall actions
    are blocked. Repeat once while closing the manager before the helper replies,
    then reopen it and confirm the restart lock remains. Confirm uninstall cannot
    start during a refresh. Confirm the manager itself has no uninstall action
    and the helper rejects a direct manager uninstall request.
14. Trigger one managed update and verify the startup check runs no more than
    once in 24 hours. Confirm an update uses an eight-second status tip while
    success and transient network failure remain silent.
15. In the isolated profile, start from the previous stable manager release and
    update to the newer stable release published by the canonical repository.
    Confirm the UI requires explicit approval, the recovery archive and pending
    journal exist outside the installed extension, the helper exits before
    Aseprite installs the candidate, and the UI requires an Aseprite restart.
16. Restart Aseprite and open the manager. Confirm the new helper reconciles the
    pending transaction, validates the installed tree and saved hashes, writes
    current/previous manager cache entries and a receipt, and removes the
    pending files. Then explicitly restore the previous manager release and
    repeat the restart/reconciliation check.
17. Repeat the manager update once with installation cancelled and confirm the
    intact old installation clears the pending transaction. In a disposable
    copy of the isolated profile, simulate an incomplete replacement and a
    changed recovery archive; confirm `SELF_UPDATE_RECOVERY_REQUIRED` is shown
    and the recovery archive is retained.
18. Disconnect networking. Confirm installed/cache state remains usable,
    expired catalog data cannot authorize a catalog operation, and direct local
    sync remains available.
19. Close and reopen the dialog, restart Aseprite, and verify receipts,
    preferences, and cache state persist under the isolated profile.
20. Confirm `shutdown`, dialog close, disconnect, and idle timeout leave no
    helper process listening.

## Completion record

Store the acceptance date, tester, macOS version, Aseprite version, commit,
artifact SHA-256, CI run links, and any accepted limitations in the private
release notes. A failed required item blocks alpha completion rather than being
recorded as a known issue.
