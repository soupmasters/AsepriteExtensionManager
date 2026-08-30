# Development and release

## Prerequisites

- Rust 1.88 or later with `rustfmt` and `clippy`
- Lua 5.4 for the fake-Aseprite tests
- Xcode command-line tools for macOS universal builds
- `musl-tools` for the Linux static target
- Aseprite 1.3.15 or later for manual testing

Run the default checks:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked
lua5.4 tests/lua/run.lua
```

## Local extension work

For the normal edit-and-test loop, quit Aseprite and run:

```sh
cargo deploy-local
```

This builds the native debug helper and updates the installed extension in the
current Aseprite profile. It preserves Aseprite's install metadata,
preferences, and helpers for other platforms. Reopen Aseprite after it finishes;
reloading the Scripts folder is not enough because the extension's Lua modules
are cached for the lifetime of the app. The command refuses to deploy while
Aseprite is running, even when a different profile is selected.

The command uses the standard Aseprite profile, or `ASEPRITE_USER_FOLDER` when
set. A different profile can be selected explicitly:

```sh
cargo deploy-local --user-config "/path/to/Aseprite"
```

## Command-line client

Install the development CLI into Cargo's binary directory:

```sh
cargo install --locked --path cli
aem doctor
```

The main commands are:

```sh
aem install unity-importer-plugin-for-unity
aem install https://github.com/owner/repository
aem install ./my-local-extension
aem list
aem search animation
aem doctor
```

`install` resolves, downloads, validates, and stages the package. It then opens
the package with Aseprite, waits for the native confirmation, verifies every
installed package file, and writes the managed receipt. Use `--asset` when a
GitHub release has several extension assets. Use `--prepare-only` to validate
and stage a package without opening Aseprite.

The CLI uses `--user-config`, then `ASEPRITE_USER_FOLDER`, then the normal
profile path for the current platform. An install into a custom or portable
profile must also pass `--aseprite PATH`. The CLI starts that executable with
`ASEPRITE_USER_FOLDER` set to the selected profile, so Aseprite and post-install
verification use the same location. `--aseprite` accepts the executable or a
macOS `.app` bundle. The installed manager supplies the trusted catalog. The CLI
and the in-app helper take an exclusive per-profile lock while changing manager
state, so close the manager window before running `aem install`.

The checked-in `extension/` directory intentionally contains no helper
binaries. Build helpers and stage a complete package before validation.

On an Apple Silicon Mac:

```sh
rustup target add aarch64-apple-darwin x86_64-apple-darwin
cargo build --locked --release -p aem-helper --target aarch64-apple-darwin
cargo build --locked --release -p aem-helper --target x86_64-apple-darwin

mkdir -p .build/helpers
lipo -create \
  target/aarch64-apple-darwin/release/aem-helper \
  target/x86_64-apple-darwin/release/aem-helper \
  -output .build/helpers/aem-helper-macos
```

Windows and Linux helpers are normally supplied by their native CI jobs.
Given all three files, stage and package:

```sh
cargo run --locked -p xtask -- stage \
  --extension extension \
  --registry registry/bundled \
  --macos-helper .build/helpers/aem-helper-macos \
  --windows-helper .build/helpers/aem-helper.exe \
  --linux-helper .build/helpers/aem-helper-linux \
  --output .build/package-input

cargo run --locked -p xtask -- package \
  --input .build/package-input \
  --output dist

cargo run --locked -p xtask -- validate-extension \
  dist/aseprite-extension-manager-0.1.0.aseprite-extension
```

`stage` refuses an existing output directory so stale files cannot leak into a
package. `package` sorts paths, uses a fixed ZIP timestamp, assigns mode `0755`
only to the macOS and Linux helpers, and assigns `0644` to all other files. It
also refuses an existing output archive. Building the same staged tree twice
must produce identical bytes.

## Version changes

A release version must be changed together in:

- workspace package metadata;
- `extension/package.json`;
- release notes and any versioned documentation examples;
- registry entries that publish the manager itself, when public.

Run both schema and package validation after any change. Direct legacy
extension sources may keep opaque versions; catalog entries cannot.

## Continuous integration

`CI` runs formatting, Clippy, workspace tests on macOS/Windows/Linux, Lua tests,
catalog authentication and reproducibility, dependency license policy, and
security advisory checks.

`Alpha package` builds:

- a macOS universal arm64 and x86_64 helper;
- a Windows x86_64 helper;
- a Linux x86_64 musl helper using the rustls network stack.

It also builds the standalone `aem` CLI as a universal macOS archive, a Windows
x86_64 zip, and a static Linux x86_64 musl archive.

Each native job smoke-runs its helper. The assembly job stages all three,
creates the archive twice, compares bytes, validates content and modes, confirms
fixture key material is absent, and uploads one alpha artifact.

A stable tag named `vMAJOR.MINOR.PATCH` must exactly match both
`extension/package.json` and the helper's Cargo version. The tag workflow builds
one canonical artifact named
`aseprite-extension-manager-MAJOR.MINOR.PATCH.aseprite-extension`, but it does
not publish an unsigned GitHub release. After the release gates below are met,
publish that exact artifact as the tag's only manager package asset. The running
manager's dedicated updater depends on that tag and filename contract; a draft,
prerelease, mismatched tag, duplicate asset, or renamed asset is intentionally
not installable as a manager update.

## Release gates

Private alpha artifacts are unsigned and for controlled testing.

A public release must not be created until all of these are true:

- the macOS helper and final package are signed with the intended Developer ID
  identity and notarized, with a successful Gatekeeper assessment;
- the Windows helper is Authenticode-signed and verifies on a clean system;
- the registry ceremony in [registry.md](registry.md) is complete;
- dependency, advisory, license, Rust, Lua, packaging, and platform jobs pass;
- the macOS acceptance run in [acceptance.md](acceptance.md) passes;
- release notes describe compatibility, unsigned-to-signed migration, known
  limitations, restart behavior, and manager recovery procedure.

Manager updates are explicit rather than silent. The manager checks the latest
stable release in the canonical repository, validates its exact universal
package, prepares a recovery archive, and asks the user before handing the
candidate to Aseprite. The helper must stop before replacement, and Aseprite
must be restarted before the new Lua and helper code are considered active.
