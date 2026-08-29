use aem_helper::github::{GitHubClient, ResolveOptions, ResolveResult};
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
