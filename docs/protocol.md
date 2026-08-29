# Helper protocol

The extension and helper communicate with versioned JSON over a loopback-only
WebSocket. Protocol version 1 is intentionally small and does not expose a
general command runner, URL fetcher, or unrestricted filesystem interface.

## Session

Lua starts:

```text
aem-helper launch --user-config <path> --extension-root <path> [--idle-seconds 120]
```

The launcher binds the helper to `127.0.0.1` on an operating-system-selected
port and creates a random 256-bit token. Its single-line handshake is:

```json
{
  "protocol": 1,
  "port": 49152,
  "token": "<base64url session token>",
  "path": "/v1/<token>",
  "pid": 12345,
  "version": "0.1.0"
}
```

The client connects to `ws://127.0.0.1:<port><path>`. The token is compared
before the connection is accepted.

Only one client is accepted. Authentication failure, a second client, a
protocol mismatch, shutdown, disconnect, or idle timeout closes the session.
The helper never binds to a wildcard or non-loopback address.

## Messages

Request:

```json
{
  "protocol": 1,
  "id": "request-17",
  "method": "scanInstalled",
  "params": {}
}
```

Successful response:

```json
{
  "protocol": 1,
  "id": "request-17",
  "ok": true,
  "result": {}
}
```

Error response:

```json
{
  "protocol": 1,
  "id": "request-17",
  "ok": false,
  "error": {
    "code": "INVALID_PARAMS",
    "message": "A concise user-safe explanation.",
    "retryable": false
  }
}
```

Progress event:

```json
{
  "protocol": 1,
  "event": "progress",
  "operationId": "operation-4",
  "phase": "download",
  "message": "Downloading package",
  "current": 4096,
  "total": 8192
}
```

Unknown request fields and methods are rejected. Every response repeats the
request identifier. Errors can include structured `details`, but must not
include session tokens or secrets.

For request-backed work, `operationId` equals the request `id`, allowing
progress to be routed without another mutable identifier.

## Method allowlist

| Method | Purpose |
| --- | --- |
| `ping` | Confirm protocol and helper liveness. |
| `scanInstalled` | Read installed user-extension manifests and join managed receipts. |
| `refreshRegistry` | Refresh and authenticate catalog metadata, or load last-known-good cache. |
| `resolveGitHub` | Resolve a supported public GitHub URL to explicit candidates or immutable source. |
| `preparePackage` | Download, validate, normalize, hash, stage, and cache a resolved artifact. |
| `prepareSelfUpdate` | With `{}`, prepare a newer manager release from the canonical repository together with a recovery archive and pending journal. |
| `prepareSelfRollback` | With `{}`, prepare the manager's verified previous release through the same recovery-safe transaction. |
| `syncLocal` | Snapshot and package a selected local extension folder. |
| `verifyInstall` | Rescan after Aseprite installation and atomically commit a matching receipt. |
| `listUpdates` | Compare linked local folders, tracked GitHub sources, catalog releases, and the canonical manager release with installed state. |
| `prepareRollback` | With `{ "name": "<manifest name>" }`, return the previously cached artifact after validating it again. |
| `cacheStatus` | Report current and previous cache entries and byte counts. |
| `clearCache` | With `{ "preserveRestorePoints": true }` by default, clear disposable cache data without removing restore artifacts. |
| `diagnostics` | Return local version, paths, protocol, and non-sensitive health information. |
| `shutdown` | End the authenticated session. |

`resolveGitHub` accepts only supported public GitHub forms. If a stable release
contains multiple matching extension assets, its result contains candidates
and requires a new user-confirmed request. Repository snapshots record both the
tracked ref and its resolved commit SHA. A later check resolves the ref again;
changed snapshot bytes can be an update even when the manifest version is the
same.

`syncLocal` is rooted at the folder containing the user-selected
`package.json`. It reads that tree only, honors `.aemignore`, skips fixed manager
and operating-system exclusions, and never modifies the source. A verified
installation links the receipt to the canonical `package.json` path and content
hash, not the installed extension directory. Later checks resnapshot that path
and report content changes without requiring a manifest-version change.

`listUpdates` returns the refreshed installed `packages`, an `updates` list,
and an `updateErrors` list. Each affected installed package also receives an
`updateError` field. A source identity change, missing local folder, ambiguous
saved release asset, network failure, or validation failure is therefore
distinct from a successful check that found no update. One package's error
does not prevent other packages from being checked.

`prepareSelfUpdate` does not accept a caller-supplied repository, URL, release,
asset, or expected version. Its result has `selfUpdate: true`,
`restartRequired: true`, and absolute candidate and recovery paths. The caller
must stop the current helper before invoking Aseprite's installer. The new
helper reconciles the pending journal after restart. `prepareSelfRollback` uses
only the manager's cached, verified previous archive. Generic
`resolveGitHub`, `preparePackage`, and `syncLocal` requests cannot install a
package whose manifest identifies it as Aseprite Extension Manager.

## Compatibility

The Lua client and helper reject any message whose `protocol` is not exactly 1.
Adding an optional result field is compatible. Changing request meaning,
removing fields, or changing security checks requires a new protocol version
and coordinated Lua/helper release.
