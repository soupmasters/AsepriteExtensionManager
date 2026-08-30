# Security model

## Trust boundaries

The Aseprite Lua process controls user interaction and invokes Aseprite's
supported installer. The bundled helper is a separate, short-lived process for
operations that require HTTPS, archive parsing, hashing, or local packaging.
Downloaded extensions and local development folders are untrusted input.

The helper:

- binds only to `127.0.0.1`;
- authenticates one client with a random 256-bit session token;
- enforces protocol version 1 and a closed method allowlist;
- exits on shutdown, disconnect, or idle timeout;
- does not expose a shell, arbitrary command, arbitrary URL, or arbitrary
  filesystem method.

The extension requests only Aseprite's command-execution and localhost
WebSocket permissions. Full or blanket access is not part of the design.

## Source policy

Version 1 supports public repositories through direct HTTPS and private
repositories through an installed, signed-in GitHub CLI. Accepted URLs remain
restricted to canonical `https://github.com/owner/repository` repository,
tree, and release-asset forms. GitHub Enterprise, credentials in URLs, custom
registries, install hooks, dependencies, and third-party native code are out
of scope.

Authenticated access invokes `gh api` directly with fixed arguments. It does
not run a shell, request or copy an authentication token, or put credentials in
Lua, RPC messages, receipts, logs, or error text. CLI prompts and update
notifiers are disabled, command output is bounded, and every command has a hard
timeout. The downloaded bytes still pass through the same size limits,
normalization, manifest validation, hashing, and staging used for public
downloads.

The GitHub tab is shown only when both Git and GitHub CLI are installed.
Repository discovery uses fixed, cursor-paged GraphQL queries with bounded
output and checks at most 100 raw repositories per response. Search text,
cursors, and immutable Blob IDs are passed as individual process arguments,
not through a shell. The helper filters discovery results to repositories with
an Aseprite-shaped root manifest or a stable `.aseprite-extension` release
asset. Root manifests are read in bounded batches and are never returned to
Lua, logged, or cached. These signals are discovery hints only; full archive
validation remains mandatory before installation. Private repository metadata
is displayed only in the local manager window and is never cached or logged.

Release resolution prefers one stable `.aseprite-extension` asset. Multiple
matching assets require explicit selection. Repository branches and tags are
resolved to a full commit SHA before download. Managed GitHub receipts also
retain the selected asset or tracked ref so later checks do not silently switch
lineage. A changed package identity is rejected and surfaced as an update-check
error.

Local snapshots begin at the folder containing the selected `package.json`.
They do not modify the source and do not follow links. `.git`, operating-system
metadata, Aseprite internal files, `.aem-*`, and `.aemignore` matches are
excluded. The canonical manifest path and a deterministic content hash are kept
for later checks. This is a linked source record, not a live filesystem link in
Aseprite's extensions directory.

## Archive policy

Before Aseprite receives an artifact, validation rejects:

- absolute paths, parent traversal, invalid path encodings, and links or device
  entries;
- duplicate paths and case-folding collisions;
- encrypted entries, multiple manifests, or mismatched source/manifest
  identity;
- missing contribution files and unsupported native executables or libraries;
- excessive entry counts, compressed or expanded sizes, compression ratios,
  and total package sizes.

Downloads are bounded, streamed, hashed, and moved from staging only after
validation. The exact SHA-256 and byte length enter the receipt.

## Manager update policy

Ordinary package paths reject the manager's manifest identity, and their
third-party native-code prohibition remains unchanged. Manager update and
rollback are available only through dedicated protocol methods; callers cannot
substitute a repository or arbitrary archive.

The update method trusts only the latest non-draft, non-prerelease
`vMAJOR.MINOR.PATCH` release of
`soupmasters/AsepriteExtensionManager`. It requires exactly one asset named
`aseprite-extension-manager-MAJOR.MINOR.PATCH.aseprite-extension`, a canonical
download URL for that repository and tag, the GitHub-reported byte length, and
the GitHub-reported SHA-256 digest, and a version newer than the installed
helper. Both digest and length are checked again after download.

The manager-specific validator applies the normal path, collision, size, and
contribution checks, then requires the manager identity, stable version, full
runtime and registry layout, expected file modes, and a hash-prefixed catalog
target. Native content is allowed only at the three fixed bundled-helper paths,
with the expected Mach universal, PE, or ELF format. Any other executable name
or native magic remains rejected, as does private registry material.

A recovery archive made from a local macOS deployment may contain one supported
64-bit arm64 or x86_64 Mach-O helper at the exact macOS helper path. That narrow
exception never applies to downloaded releases: their fat header and bounded
arm64 and x86_64 slices are both verified before installation and again after
restart.

## Catalog trust

Catalog metadata follows TUF 1.0 with a pinned root, role separation,
consistent snapshots, expiry enforcement, rollback protection, and
last-known-good caching. Expired cached metadata can explain existing state but
cannot authorize a catalog install or update.

The committed fixture keys are intentionally non-secret and cannot be reused
for public trust. Public signing requirements are in
[registry.md](registry.md).

## Lifecycle and recovery

Install, update, and restore pass through Aseprite's installer prompt. Enable
and disable hand off to native Extensions preferences so Aseprite can run the
extension lifecycle hooks. Aseprite exposes no scripting uninstall command, so
manager uninstall is explicitly restart-bound. The helper accepts only the
path, manifest name, and version returned by its own installed scan. It
canonicalizes the path, requires a real direct child of the active profile's
extensions directory, re-reads the manifest, and rejects the manager identity
and canonical manager path. It then atomically moves the complete folder to
manager-owned recovery storage. Only a uniquely matching receipt is archived
and removed, and a transaction record reconciles interrupted receipt cleanup
on the next helper start. No extension files are deleted. Loaded commands can
remain registered after the move, but resource access and shutdown hooks can
fail because the original folder is gone. The manager therefore blocks further
extension changes and requires an immediate Aseprite restart. The lock begins
when the request is dispatched and survives a closed dialog or lost helper
connection. It is cleared without a restart only when the helper explicitly
rejects the request before moving the folder.

Managed receipts are committed only after the installed manifest matches the
expected name and version. The cache retains current and previous exact
artifacts and their recorded digests. Staged content-addressed files are
repaired if their bytes do not match their filename, and rollback preparation
checks the cached artifact against its receipt. A failed non-atomic update can
offer an immediate, user-confirmed restore through Aseprite.

Manager replacement has an additional restart boundary. Before Aseprite sees
the candidate, the helper validates the installed manager, creates a recovery
archive, and atomically journals the candidate, recovery artifact, versions,
hashes, byte lengths, and source identities outside the extension tree. The
copied artifacts are rehashed before the journal is published. The
helper is shut down before installation so its executable can be replaced.
The replacement is considered complete only when the newly started helper
validates the installed tree and both journaled artifacts, then writes the
receipt and clears the journal.

Cancellation with an intact old installation clears the pending transaction.
An incomplete replacement, version mismatch, missing artifact, or hash mismatch
preserves the recovery archive and reports `SELF_UPDATE_RECOVERY_REQUIRED`.
Rollback is user-confirmed and passes the verified previous manager archive
through the same journaled installation flow.

Cache clearing affects only manager-owned cache files. It does not uninstall
extensions or remove receipts that represent installed state without a
separate explicit operation. Recovery copies from uninstalls are retained.

## Privacy and diagnostics

There is no telemetry or remote error reporting. Logs and diagnostics remain
under the active Aseprite profile. Diagnostics omit session tokens and any
sensitive URL material. Help can report whether Git and GitHub CLI are
installed and whether GitHub CLI has an active GitHub login. It exposes only
booleans and sanitized version text, never executable paths, raw command
output, or tokens. Logs rotate and are suitable for explicit user review before
sharing.

## Distribution gates

Private-alpha helpers are unsigned. Public distribution is blocked on macOS
Developer ID signing and notarization, Windows Authenticode signing, public
registry key separation, and the acceptance checklist. A signature verifies
publisher identity; it does not replace package validation or catalog
authentication.
