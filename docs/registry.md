# Registry

## Bundled curated catalog

The manager ships a curated preview catalog. The one human-edited source is
`registry/catalog-v1.json`; authenticated generated copies are stored under
`registry/bundled` and included in each manager package. Direct GitHub and
local-folder operations remain available.

Catalog inclusion is version-specific. It means the exact package bytes or
repository commit were reviewed and pinned; it is not a blanket endorsement of
future versions or every action performed by an extension.

The package contains only:

```text
registry/
├── root.json
├── metadata/
│   ├── 1.root.json
│   ├── timestamp.json
│   ├── 5.snapshot.json
│   ├── snapshot.json
│   ├── 5.targets.json
│   └── targets.json
└── targets/
    ├── <sha256>.catalog-v1.json
    └── catalog-v1.json
```

The versioned metadata and hash-prefixed target implement consistent
snapshots. Unversioned copies are byte-identical compatibility aliases. The
packaging validator verifies the complete signed chain and exact file set, and
rejects fixture keys, seeds, and private fixture paths.

The committed signing seeds under `registry/fixtures/keys` are deliberately
public. They make the bundled preview metadata reproducible, but they do not
establish trust for a network-updated public catalog. The catalog currently
changes only as part of a reviewed manager release.

## Catalog schema version 1

The authenticated target is validated against
`registry/schema/catalog-v1.schema.json`. Package identifiers are the
case-folded Aseprite manifest `name`. Each package records:

- manifest identity, display name, author, license, homepage, and repository;
- stable releases with strict `MAJOR.MINOR.PATCH` versions;
- minimum and optional maximum Aseprite and scripting API compatibility;
- an immutable HTTPS source URL, SHA-256, and byte length;
- publication time, release notes, and yanked state;
- either release-asset provenance or an immutable GitHub commit identity.

A release asset must already be a valid root-flat `.aseprite-extension` file.
A repository snapshot must use the exact canonical GitHub codeload URL for its
declared repository and 40-character commit. The helper authenticates the
downloaded source bytes, removes the GitHub wrapper directory, creates a clean
extension archive, and validates its manifest and contribution paths. Release
asset metadata and snapshot metadata cannot be mixed.

Yanked releases remain visible to explain existing receipts but cannot be
selected for new installs or updates.

## Client rules

The client pins the trusted root distributed in the extension. Refresh order
is timestamp, snapshot, targets, then catalog. Every role signature, threshold,
version, expiry, hash, and byte length is checked before new state is committed.

Metadata rollback and freeze attacks are rejected by persisting the highest
accepted versions and enforcing timestamp expiry. A valid last-known-good cache
can be displayed while offline. Expired metadata can also be displayed with a
clear stale state, but it cannot authorize catalog installs or updates. Direct
GitHub and local-folder operations do not depend on catalog freshness.
Expiry also does not block signature/hash validation of an already-installed
manager when creating a recovery package or updating the manager itself.

## Suggesting an extension

Anyone can use the repository's
[extension suggestion form](https://github.com/soupmasters/AsepriteExtensionManager/issues/new?template=extension-suggestion.yml).
Candidates must be public, open-source Aseprite extensions with a root manifest,
a stable semantic version, valid contribution paths, and no native executable
content. A published `.aseprite-extension` release asset is preferred; a tagged
repository snapshot can be accepted after the same source and archive review.

Maintainers verify identity, ownership, license, compatibility, source history,
archive contents, SHA-256, and byte length before editing
`registry/catalog-v1.json`. Submissions do not receive automatic inclusion, and
security reports must use GitHub's private advisory form rather than a public
catalog issue.

## Bundled metadata maintenance

Validate and regenerate the reproducible bundled chain in a fresh directory:

```sh
cargo run --locked -p xtask -- validate-catalog \
  --schema registry/schema/catalog-v1.schema.json \
  --catalog registry/catalog-v1.json

fixture_output="$(mktemp -d)"
cargo run --locked -p xtask -- registry-fixtures \
  --keys registry/fixtures/keys \
  --catalog registry/catalog-v1.json \
  --root-version 1 \
  --version 5 \
  --output "$fixture_output"

cargo run --locked -p xtask -- verify-registry-fixtures \
  --root "$fixture_output/root.json" \
  --metadata "$fixture_output/metadata" \
  --targets "$fixture_output/targets"

diff --recursive --unified registry/bundled "$fixture_output"
```

Review the complete diff before replacing `registry/bundled` as a unit, so
obsolete versioned metadata or hash-prefixed targets cannot survive. Keep the
root version stable unless its keys, roles, thresholds, or expiry change, and
increment timestamp, snapshot, and targets versions whenever their signed
bytes change. `--expired` creates correctly signed expiry fixtures for tests.

## Network catalog signing ceremony

Network-updated catalog publication is blocked until a separate signing
ceremony:

1. Use an offline, freshly prepared system and record the participants,
   software versions, date, and checksums.
2. Generate independent root and targets keys. Use threshold policies that
   tolerate one unavailable key without allowing one holder to publish alone.
3. Generate independent online snapshot and timestamp keys. Put only those
   online private keys in a protected GitHub environment with required
   reviewers; do not add them to Git.
4. Add all public keys and role thresholds to the initial root. Sign the root
   and targets metadata offline. Verify every key identifier and signature on a
   second system.
5. Preserve encrypted offline backups in separate physical locations. Record
   recovery and revocation procedures before publication.
6. Publish versioned metadata and hash-prefixed targets through GitHub Pages
   from this repository. Retain old root versions required for sequential root
   rotation.
7. Replace the extension's fixture root with the reviewed public root, pin its
   checksum in source, and change the default catalog transport from bundled to
   the Pages origin.
8. Exercise root rotation, targets rotation, key compromise, expiry, rollback,
   freeze, corrupted metadata, and offline-cache tests against a staging
   origin.

Root rotation increments by exactly one and is signed by thresholds from both
the previous and new roots. A compromised online timestamp or snapshot key is
rotated promptly with an offline targets/root update as appropriate.
