# Registry

## Private-alpha catalog

The private alpha ships an authenticated catalog with zero third-party
packages. Direct public GitHub and local-folder operations remain available.

The package contains only:

```text
registry/
├── root.json
├── metadata/
│   ├── timestamp.json
│   ├── 1.snapshot.json
│   ├── snapshot.json
│   ├── 1.targets.json
│   └── targets.json
└── targets/
    ├── <sha256>.catalog-v1.json
    └── catalog-v1.json
```

The versioned metadata and hash-prefixed target implement consistent
snapshots. Unversioned copies are byte-identical compatibility aliases. The
packaging validator rejects fixture keys, seeds, and private fixture paths.

The committed signing seeds under `registry/fixtures/keys` are deliberately
public and restricted to tests and private-alpha fixtures. They establish no
trust for a public catalog.

## Catalog schema version 1

The authenticated target is validated against
`registry/schema/catalog-v1.schema.json`. Package identifiers are the
case-folded Aseprite manifest `name`. Each package records:

- manifest identity, display name, author, license, homepage, and repository;
- stable releases with strict `MAJOR.MINOR.PATCH` versions;
- minimum and optional maximum Aseprite and scripting API compatibility;
- an immutable HTTPS asset URL, SHA-256, and byte length;
- publication time, release notes, and yanked state;
- optional release tag, asset identifier, or immutable commit identity.

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

## Fixture maintenance

Validate and regenerate the reproducible private-alpha chain with:

```sh
cargo run --locked -p xtask -- validate-catalog \
  --schema registry/schema/catalog-v1.schema.json \
  --catalog registry/bundled/targets/catalog-v1.json

cargo run --locked -p xtask -- registry-fixtures \
  --keys registry/fixtures/keys \
  --catalog registry/bundled/targets/catalog-v1.json \
  --output registry/bundled

cargo run --locked -p xtask -- verify-registry-fixtures \
  --root registry/bundled/root.json \
  --metadata registry/bundled/metadata \
  --targets registry/bundled/targets
```

CI regenerates the chain in a temporary directory and compares every byte.
`--version 2` can create a rotated metadata set in a temporary directory, and
`--expired` can create a correctly signed expiry fixture. These options are for
rotation, rollback, freeze, and expiry tests; they are never used for the
bundled catalog.

## Public signing ceremony

Public catalog publication is blocked until a separate signing ceremony:

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
