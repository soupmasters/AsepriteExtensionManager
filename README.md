<p align="center">
  <img src="assets/aseprite-extension-manager-banner.png" alt="Aseprite Extension Manager">
</p>

# Aseprite Extension Manager

A small, unofficial extension manager for installing and updating Aseprite
extensions from a bundled catalog, GitHub, or a local folder.

[Suggest an extension for the catalog](https://github.com/soupmasters/AsepriteExtensionManager/issues/new?template=extension-suggestion.yml).

The project is still in development.

## CLI

```sh
cargo install --locked --path cli
aem search animation
aem install unity-importer-plugin-for-unity
```

`aem install` also accepts a GitHub URL or local extension folder. Aseprite
shows the final install confirmation.

## Development

Quit Aseprite and deploy your local changes with:

```sh
cargo deploy-local
```

```sh
cargo test --workspace --all-features --locked
lua5.4 tests/lua/run.lua
```

## License

[MIT](LICENSE)
