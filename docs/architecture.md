# Architecture

## Scope

Aseprite Extension Manager manages extensions for the profile of the running
Aseprite process. The profile root comes from `app.fs.userConfigPath`; the
location of the Aseprite application and its store or distribution channel are
irrelevant.

The manager has three runtime parts:

1. `extension/` contains the Lua entry point, native dialogs, preferences, and
   the WebSocket client.
2. `aem-helper` performs network, archive, hashing, cache, receipt, catalog, and
   local-folder operations.
3. `registry/bundled` contains a pinned root and the authenticated curated
   catalog generated from `registry/catalog-v1.json`.

The packaged helpers are selected by the Lua layer:

| Platform | Package path |
| --- | --- |
| macOS arm64 or x86_64 | `bin/macos/aem-helper` |
| Windows x86_64 | `bin/windows/aem-helper.exe` |
| Linux x86_64 | `bin/linux/aem-helper` |

The macOS file is a universal binary. No external Git client, shell utility, or
application-location discovery is required at runtime.

## Runtime lifecycle

1. Aseprite loads `main.lua`. Unsupported Aseprite or scripting API versions
   receive a compatibility message before any helper starts.
2. The user opens **Aseprite Extension Manager…** from the Scripts menu.
3. On first use, the Lua layer explains the command-execution and localhost
   WebSocket permission requests.
4. The launcher starts the matching helper. It returns a random loopback port
   and a 256-bit session token, then exits.
5. One authenticated WebSocket client connects. Requests use protocol version
   1 and the closed method set in [protocol.md](protocol.md).
6. The helper exits after `shutdown`, when the client disconnects, or after its
   idle timeout.

The helper prepares and validates installation artifacts. Lua gives a prepared
absolute path to:

```lua
app.command.Options { installExtension = absolutePath }
```

After Aseprite's prompt closes, the manager rescans the installed manifest. A
receipt is written only when the expected package name and version are present.
Enable and disable actions open Aseprite's native Extensions preferences.
Uninstall is restart-bound because Aseprite exposes no scripting command for
unloading an extension. After confirmation, the helper revalidates the exact
scanned folder, refuses the manager itself, and atomically moves that folder to
manager-owned recovery storage. A matching receipt is archived and removed
through the same journaled transaction, with cleanup reconciled on the next
helper start if it was interrupted. The manager removes the entry from its
current view and blocks further extension changes for the rest of the session.
Aseprite must be restarted immediately because loaded commands can remain
registered while file and resource access can already fail. The restart lock is
set when the uninstall request is sent. A definite helper rejection clears it,
but an interrupted request keeps it because the move may already have finished.

A manager update is the one exception to same-session verification. Before
Aseprite installs it, the extension shuts down the helper so the helper binary
can be replaced safely on every platform. Aseprite unloads and replaces the
extension, and the new helper reconciles the pending transaction the next time
the manager starts. Restarting Aseprite is required because loaded Lua modules
remain cached for the lifetime of the process.

## Installation and restore

Package preparation is separate from installation:

1. Resolve a catalog, GitHub, or local-folder source to immutable input.
2. Download or snapshot into staging.
3. Validate archive paths, entry types, manifest identity, contribution paths,
   size limits, and unsupported native content.
4. Hash the exact staged artifact.
5. Ask Aseprite to install it.
6. Verify the installed manifest, rotate the managed current/previous cache,
   and atomically write the receipt.

Each managed package retains exact `current` and `previous` artifacts. If
Aseprite's update does not verify, the UI can offer the previous artifact
through the same native installer. Restore is always explicit.

## Linked sources and update checks

Selecting a local folder's `package.json` creates a normalized snapshot; it
does not install a filesystem link into Aseprite. Once installation verifies,
the receipt keeps the canonical `package.json` path and a hash of the snapshot
contents. Later update checks rebuild a read-only snapshot from that folder.
A changed content hash is an update even when the manifest version is unchanged.
The package name must remain the same.

A GitHub repository URL prefers one extension asset from its latest stable
release. If no matching release asset exists, the selected branch or tag is
resolved to a full commit and installed as a repository snapshot. Receipts keep
the repository plus either the selected release asset or the tracked ref and
resolved commit. Update checks resolve that saved source again. A newer release
version is an update; a tracked repository snapshot is also an update when its
artifact changes without a version change. Downgrades and identical artifacts
are ignored.

Public GitHub sources use the helper's restricted HTTPS client. A source that
is not publicly visible can be retried through a signed-in GitHub CLI using
`gh api`. This is a transport choice only. Private downloads retain the same
canonical source identity and pass through the same validation and receipt
flow, so later update checks work without persisting credentials.

Failure to resolve, read, or validate one linked source is attached to that
installed package as `updateError` and returned in the top-level `updateErrors`
list. It is not treated as proof that the package is current, and it does not
hide updates found for other packages.

## Manager update transaction

The manager has a dedicated update path because its package contains the
native helper that performs ordinary validation. Update checks consult only
the latest stable release of
`https://github.com/soupmasters/AsepriteExtensionManager`. Preparation requires
an exact, canonical release asset named for the release version and a newer
stable semantic version.

After the user confirms the update, the helper validates the currently
installed manager, builds an exact recovery archive from it, validates the
candidate with the manager-specific layout rules, and atomically records both
archives and their hashes in a pending journal. The Lua side then stops the
helper, waits for it to exit, and passes the candidate to Aseprite's installer.

On the next helper start, reconciliation checks the installed manager and
helper version against the journal, revalidates and rehashes both saved
archives, rotates the manager cache, writes the receipt, and removes the
pending files. If installation was cancelled and the old manager remains
intact, the journal is discarded safely. Any incomplete or inconsistent state
becomes `SELF_UPDATE_RECOVERY_REQUIRED` and keeps the recovery archive
available. A later user-confirmed manager rollback uses that verified previous
archive and the same restart-safe transaction.

Normal release candidates must contain a valid universal macOS helper before
and after installation. Recovery snapshots from a local development deploy may
retain a valid thin arm64 or x86_64 helper, but only at the fixed macOS helper
path and only through the recovery transaction.

## State layout

All mutable state is beneath:

```text
<userConfigPath>/extension-manager/
├── cache/
│   └── <package-id>/
│       ├── current.aseprite-extension
│       └── previous.aseprite-extension
├── receipts/
│   └── <package-id>.json
├── staging/
├── http/
├── tuf/
├── self-update/
│   ├── pending.json
│   ├── candidate.aseprite-extension
│   └── recovery.aseprite-extension
└── logs/
```

The three `self-update` files exist only while a manager update or rollback is
pending. They live outside the installed extension tree so Aseprite cannot
delete them while replacing the manager.

Files are written through a temporary file, synchronized, and renamed in the
same directory where atomic replacement is available. Logs rotate locally.
Nothing is sent for analytics or remote error reporting.

## Receipt schema

Receipts use schema version 1. Optional source-specific fields are omitted when
they do not apply.

```json
{
  "schemaVersion": 1,
  "packageName": "example-extension",
  "sourceKind": "github-release",
  "source": {
    "kind": "github-release",
    "repository": "https://github.com/owner/repository",
    "immutableUrl": "https://github.com/owner/repository/releases/download/v1.2.3/example-extension.aseprite-extension",
    "release": "v1.2.3",
    "assetId": 5678,
    "assetName": "example-extension.aseprite-extension"
  },
  "installedVersion": "1.2.3",
  "release": "v1.2.3",
  "asset": "example-extension.aseprite-extension",
  "artifactSha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
  "artifactByteLength": 12345,
  "installedAt": "2026-07-31T12:00:00Z",
  "previousArtifact": "<userConfigPath>/extension-manager/cache/example-extension/previous.aseprite-extension",
  "previousSource": {
    "kind": "github-release",
    "repository": "https://github.com/owner/repository",
    "release": "v1.1.0"
  },
  "previousVersion": "1.1.0"
}
```

`sourceKind` is one of the implemented catalog, GitHub release, GitHub
snapshot, direct artifact, or local snapshot forms. `source` records the
immutable identity required to reproduce update checks. Legacy opaque manifest
versions can be installed directly, but only an unambiguous saved release or
commit source can authorize their updates.
