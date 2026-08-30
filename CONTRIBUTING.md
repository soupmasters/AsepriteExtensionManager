# Contributing

Bug fixes and focused improvements are welcome through pull requests.

## Suggest an extension

Anyone can propose a public Aseprite extension for the bundled catalog by
opening the [extension suggestion form](https://github.com/soupmasters/AsepriteExtensionManager/issues/new?template=extension-suggestion.yml).

Catalog candidates must have:

- a public GitHub repository with one Aseprite `package.json` at its root;
- a stable `MAJOR.MINOR.PATCH` version and a clear open-source license;
- valid Aseprite contribution paths and no native executable content;
- either a stable `.aseprite-extension` release asset or a reviewable tagged
  repository snapshot.

Inclusion is reviewed manually and is never automatic. Maintainers verify the
manifest, license, source history, compatibility, archive contents, exact byte
length, and SHA-256 before updating `registry/catalog-v1.json`.
