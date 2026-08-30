use std::fs;
use std::path::Path;

use aem_helper::github::{GitHubClient, ResolveOptions, ResolveResult};
use aem_helper::registry::Catalog;
use aem_helper::state::State;

#[tokio::test]
#[ignore = "requires access to the public GitHub API and download hosts"]
async fn resolves_a_real_public_repository_snapshot() {
    let temporary = tempfile::tempdir().expect("tempdir");
    let state = State::new(temporary.path()).expect("state");
    let client = GitHubClient::new(state).expect("GitHub client");
    let result = client
        .resolve(ResolveOptions {
            url: "https://github.com/matsagad/color-ramp-sort/tree/main".to_owned(),
            selection: None,
        })
        .await
        .expect("resolve public repository");

    let ResolveResult::Ready { package, source } = result else {
        panic!("repository snapshot unexpectedly required an asset selection");
    };
    assert_eq!(package.name, "color-ramp-sort");
    assert_eq!(source.kind, "github-snapshot");
    assert_eq!(source.tracked_ref.as_deref(), Some("main"));
    assert!(source.commit.as_deref().is_some_and(|commit| {
        commit.len() == 40 && commit.bytes().all(|byte| byte.is_ascii_hexdigit())
    }));
    assert!(package.artifact_path.is_file());
}

#[tokio::test]
#[ignore = "requires access to the pinned public GitHub download hosts"]
async fn prepares_pinned_curated_catalog_sources() {
    let temporary = tempfile::tempdir().expect("tempdir");
    let state = State::new(temporary.path()).expect("state");
    let client = GitHubClient::new(state).expect("GitHub client");
    let catalog_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("registry")
        .join("catalog-v1.json");
    let catalog: Catalog = serde_json::from_slice(
        &fs::read(&catalog_path)
            .unwrap_or_else(|error| panic!("read {}: {error}", catalog_path.display())),
    )
    .unwrap_or_else(|error| panic!("parse {}: {error}", catalog_path.display()));

    let mut verified = 0_usize;
    for catalog_package in &catalog.packages {
        for release in catalog_package
            .releases
            .iter()
            .filter(|release| !release.yanked)
        {
            let prepared = client
                .prepare_authenticated_asset(
                    &release.asset.url,
                    &release.asset.sha256,
                    release.asset.byte_length,
                    &catalog_package.manifest_name,
                    &release.version,
                    release.asset.commit.as_deref(),
                )
                .await
                .unwrap_or_else(|error| {
                    panic!(
                        "prepare catalog package {} {}: {error}",
                        catalog_package.id, release.version
                    )
                });

            assert_eq!(prepared.name, catalog_package.manifest_name);
            assert_eq!(prepared.version, release.version);
            assert!(prepared.artifact_path.is_file());
            verified += 1;
        }
    }
    assert!(verified > 0, "catalog has no non-yanked releases to verify");
}
