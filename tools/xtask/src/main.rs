mod catalog;
mod deploy;
mod fixtures;
mod package;

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "xtask",
    version,
    about = "Repository build and registry operations"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Build and deploy to the locally installed Aseprite profile.
    DeployLocal {
        /// Aseprite user configuration directory.
        #[arg(long)]
        user_config: Option<PathBuf>,
    },
    /// Assemble extension sources, registry metadata, and native helpers.
    Stage {
        #[arg(long)]
        extension: PathBuf,
        #[arg(long)]
        registry: PathBuf,
        #[arg(long)]
        macos_helper: PathBuf,
        #[arg(long)]
        windows_helper: PathBuf,
        #[arg(long)]
        linux_helper: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Create a deterministic .aseprite-extension archive.
    Package {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Validate a staged directory or .aseprite-extension archive.
    ValidateExtension { path: PathBuf },
    /// Validate a catalog against its JSON Schema.
    ValidateCatalog {
        #[arg(long)]
        schema: PathBuf,
        #[arg(long)]
        catalog: PathBuf,
    },
    /// Generate reproducible metadata for the bundled preview catalog.
    RegistryFixtures {
        #[arg(long)]
        keys: PathBuf,
        #[arg(long)]
        catalog: PathBuf,
        #[arg(long)]
        output: PathBuf,
        /// Timestamp, snapshot, and targets metadata version.
        #[arg(long, default_value_t = 1)]
        version: u64,
        /// Trusted root metadata version. Keep this stable unless the root changes.
        #[arg(long, default_value_t = 1)]
        root_version: u64,
        /// Produce correctly signed but expired top-level metadata.
        #[arg(long)]
        expired: bool,
    },
    /// Verify fixture signatures and the complete metadata chain.
    VerifyRegistryFixtures {
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        metadata: PathBuf,
        #[arg(long)]
        targets: PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::DeployLocal { user_config } => {
            deploy::run(&workspace_root()?, user_config.as_deref())
        }
        Command::Stage {
            extension,
            registry,
            macos_helper,
            windows_helper,
            linux_helper,
            output,
        } => package::stage(
            &extension,
            &registry,
            &macos_helper,
            &windows_helper,
            &linux_helper,
            &output,
        ),
        Command::Package { input, output } => package::create(&input, &output),
        Command::ValidateExtension { path } => package::validate(&path),
        Command::ValidateCatalog { schema, catalog } => catalog::validate_files(&schema, &catalog),
        Command::RegistryFixtures {
            keys,
            catalog,
            output,
            version,
            root_version,
            expired,
        } => fixtures::generate(&keys, &catalog, &output, root_version, version, expired),
        Command::VerifyRegistryFixtures {
            root,
            metadata,
            targets,
        } => fixtures::verify(&root, &metadata, &targets),
    }
}

fn workspace_root() -> Result<PathBuf> {
    std::fs::canonicalize(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."))
        .context("resolve workspace root")
}
