use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use futures_util::future::try_join_all;
use percent_encoding::{percent_decode_str, utf8_percent_encode, NON_ALPHANUMERIC};
use reqwest::header::{HeaderMap, ETAG, IF_NONE_MATCH};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use url::Url;

use crate::package::{
    package_repository_archive, validate_and_stage, validate_manager_and_stage, ExpectedManifest,
    PreparedPackage, MAX_ARCHIVE_BYTES,
};
use crate::protocol::{RpcError, RpcResult};
use crate::state::{atomic_write, package_id_is_safe, State};
use crate::tooling::{self, Tool};
use crate::VERSION;

const API_ROOT: &str = "https://api.github.com";
const GH_JSON_BYTES: u64 = 4 * 1024 * 1024;
const GH_JSON_TIMEOUT: Duration = Duration::from_secs(30);
const GH_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(120);
const GH_REPOSITORY_JSON_BYTES: u64 = 8 * 1024 * 1024;
const GH_REPOSITORY_PAGE_SIZE: usize = 6;
const GH_REPOSITORY_SCAN_PAGE_SIZE: u32 = 50;
const GH_REPOSITORY_SCAN_LIMIT: usize = 500;
const GH_REPOSITORY_SCAN_TIMEOUT: Duration = Duration::from_secs(60);
const GH_REPOSITORY_MANIFEST_BYTES: u64 = 64 * 1024;
const GH_REPOSITORY_MANIFEST_BATCH_SIZE: usize = 16;
const GH_REPOSITORY_QUERY: &str = r#"query($first:Int!,$after:String){viewer{repositories(first:$first,after:$after,affiliations:[OWNER,COLLABORATOR,ORGANIZATION_MEMBER],orderBy:{field:UPDATED_AT,direction:DESC}){totalCount pageInfo{hasNextPage endCursor} edges{cursor node{name owner{login} description isPrivate isArchived isFork updatedAt viewerPermission manifest:object(expression:"HEAD:package.json"){... on Blob{id oid byteSize isBinary isTruncated}} latestRelease{isDraft isPrerelease releaseAssets(first:100){nodes{name}}}}}}}}"#;
const GH_REPOSITORY_SEARCH_QUERY: &str = r#"query($first:Int!,$after:String,$search:String!){search(type:REPOSITORY,query:$search,first:$first,after:$after){repositoryCount pageInfo{hasNextPage endCursor} edges{cursor node{... on Repository{name owner{login} description isPrivate isArchived isFork updatedAt viewerPermission manifest:object(expression:"HEAD:package.json"){... on Blob{id oid byteSize isBinary isTruncated}} latestRelease{isDraft isPrerelease releaseAssets(first:100){nodes{name}}}}}}}}"#;
const GH_REPOSITORY_MANIFEST_QUERY: &str = r#"query($ids:[ID!]!){nodes(ids:$ids){... on Blob{id oid byteSize isBinary isTruncated text}}}"#;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GitHubAccess {
    Public,
    GitHubCli,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GitHubTarget {
    Repository {
        owner: String,
        repository: String,
        requested_ref: Option<String>,
    },
    ReleaseAsset {
        owner: String,
        repository: String,
        tag: String,
        asset_name: String,
        url: Url,
    },
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResolveOptions {
    pub url: String,
    #[serde(default)]
    pub selection: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ListRepositoriesOptions {
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub cursor: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitHubRepository {
    pub name_with_owner: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub is_private: bool,
    pub is_archived: bool,
    pub is_fork: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub viewer_permission: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitHubRepositoryPage {
    pub repositories: Vec<GitHubRepository>,
    pub total_count: u64,
    pub has_next_page: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_cursor: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetChoice {
    pub id: u64,
    pub name: String,
    pub byte_length: u64,
    pub download_url: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagerRelease {
    pub version: String,
    pub tag: String,
    pub repository: String,
    pub asset_id: u64,
    pub asset_name: String,
    pub byte_length: u64,
    pub sha256: String,
    pub download_url: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase", tag = "status")]
pub enum ResolveResult {
    #[serde(rename = "ready")]
    Ready {
        #[serde(flatten)]
        package: Box<PreparedPackage>,
        source: Box<GitHubSource>,
    },
    #[serde(rename = "selectionRequired")]
    SelectionRequired {
        repository: String,
        release: String,
        choices: Vec<AssetChoice>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitHubSource {
    pub kind: String,
    pub repository: String,
    pub immutable_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asset_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asset_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tracked_ref: Option<String>,
}

#[derive(Clone)]
pub struct GitHubClient {
    client: Client,
    state: State,
}

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Deserialize)]
struct ReleaseAsset {
    id: u64,
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    digest: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Repository {
    default_branch: String,
}

#[derive(Debug, Deserialize)]
struct Commit {
    sha: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct HttpCache {
    #[serde(default)]
    etag: Option<String>,
    body: Vec<u8>,
}

impl GitHubClient {
    pub fn new(state: State) -> RpcResult<Self> {
        let policy = reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 5 {
                return attempt.error("too many redirects");
            }
            if is_allowed_download_host(attempt.url().host_str().unwrap_or_default()) {
                attempt.follow()
            } else {
                attempt.error("redirect left the GitHub download service")
            }
        });
        let client = Client::builder()
            .redirect(policy)
            .timeout(Duration::from_secs(30))
            .user_agent(format!("aseprite-extension-manager/{VERSION}"))
            .https_only(true)
            .build()
            .map_err(|error| RpcError::internal(error.to_string()))?;
        Ok(Self { client, state })
    }

    pub async fn resolve(&self, options: ResolveOptions) -> RpcResult<ResolveResult> {
        match parse_github_url(&options.url)? {
            GitHubTarget::ReleaseAsset {
                owner,
                repository,
                tag,
                asset_name,
                url,
            } => {
                if !asset_name
                    .to_ascii_lowercase()
                    .ends_with(".aseprite-extension")
                {
                    return Err(RpcError::invalid(
                        "UNSUPPORTED_ASSET",
                        "direct GitHub URL must identify an .aseprite-extension asset",
                    ));
                }
                let (downloaded, asset_id) = match self.download(url.clone()).await {
                    Ok(downloaded) => (downloaded, None),
                    Err(public_error) if public_request_can_fallback(&public_error) => {
                        let encoded_tag = encode_github_path_value(&tag, "release tag")?;
                        let endpoint = format!(
                            "{API_ROOT}/repos/{owner}/{repository}/releases/tags/{encoded_tag}"
                        );
                        let api_path =
                            format!("repos/{owner}/{repository}/releases/tags/{encoded_tag}");
                        let (release, _) = self
                            .get_json_with_gh::<Release>(&endpoint, &api_path)
                            .await?;
                        let mut matching = release
                            .assets
                            .into_iter()
                            .filter(|asset| asset.name == asset_name);
                        let selected = matching.next().ok_or_else(|| {
                            RpcError::invalid(
                                "NOT_FOUND",
                                "the private GitHub release does not contain that asset",
                            )
                        })?;
                        if matching.next().is_some() {
                            return Err(RpcError::invalid(
                                "INVALID_GITHUB_RESPONSE",
                                "GitHub returned duplicate release asset names",
                            ));
                        }
                        let fallback = self
                            .download_release_asset_with_gh(&owner, &repository, selected.id)
                            .await;
                        let downloaded = preserve_public_rate_limit(public_error, fallback)?;
                        (downloaded, Some(selected.id))
                    }
                    Err(error) => return Err(error),
                };
                let expected_version = expected_version_from_tag(&tag);
                let package = validate_and_stage(
                    &self.state,
                    &downloaded,
                    ExpectedManifest {
                        name: None,
                        version: expected_version.as_deref(),
                    },
                )?;
                Ok(ResolveResult::Ready {
                    package: Box::new(package),
                    source: Box::new(GitHubSource {
                        kind: "github-release".to_owned(),
                        repository: format!("https://github.com/{owner}/{repository}"),
                        immutable_url: url.to_string(),
                        release: Some(tag),
                        asset_id,
                        asset_name: Some(asset_name),
                        commit: None,
                        tracked_ref: None,
                    }),
                })
            }
            GitHubTarget::Repository {
                owner,
                repository,
                requested_ref,
            } => {
                self.resolve_repository(
                    &owner,
                    &repository,
                    requested_ref.as_deref(),
                    options.selection,
                )
                .await
            }
        }
    }

    pub async fn list_repositories(
        &self,
        options: ListRepositoriesOptions,
    ) -> RpcResult<GitHubRepositoryPage> {
        let query = normalize_repository_query(options.query)?;
        let cursor = normalize_repository_cursor(options.cursor)?;
        let (git, gh) = tokio::join!(tooling::find(Tool::Git), tooling::find(Tool::Gh));
        if git.is_none() || gh.is_none() {
            return Err(RpcError::invalid(
                "GITHUB_TOOLS_UNAVAILABLE",
                "Git and GitHub CLI are required to browse GitHub repositories",
            ));
        }
        let Some(executable) = gh else {
            return Err(RpcError::invalid(
                "GITHUB_TOOLS_UNAVAILABLE",
                "Git and GitHub CLI are required to browse GitHub repositories",
            ));
        };
        list_repositories_with_executable(&executable, query.as_deref(), cursor.as_deref()).await
    }

    pub async fn latest_manager_release(
        &self,
        owner: &str,
        repository: &str,
    ) -> RpcResult<Option<ManagerRelease>> {
        let owner = validate_identifier(owner, "owner")?;
        let repository = validate_identifier(repository, "repository")?;
        let endpoint = format!("{API_ROOT}/repos/{owner}/{repository}/releases/latest");
        match self.get_json::<Release>(&endpoint).await {
            Ok(release) => select_manager_release(&owner, &repository, &release),
            Err(error) if error.code == "NOT_FOUND" => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub async fn prepare_manager_release(
        &self,
        owner: &str,
        repository: &str,
    ) -> RpcResult<(PreparedPackage, ManagerRelease)> {
        let release = self
            .latest_manager_release(owner, repository)
            .await?
            .ok_or_else(|| {
                RpcError::invalid(
                    "MANAGER_RELEASE_NOT_FOUND",
                    "the canonical repository has no installable manager release",
                )
            })?;
        let url = Url::parse(&release.download_url)
            .map_err(|error| RpcError::invalid("INVALID_MANAGER_RELEASE_URL", error.to_string()))?;
        let downloaded = self.download(url).await?;
        let (actual_hash, actual_length) = crate::package::artifact_hash(&downloaded)?;
        if actual_length != release.byte_length
            || !actual_hash.eq_ignore_ascii_case(&release.sha256)
        {
            return Err(RpcError::invalid(
                "MANAGER_RELEASE_INTEGRITY_MISMATCH",
                "download hash or length differs from the canonical GitHub release metadata",
            )
            .with_details(serde_json::json!({
                "expectedSha256": release.sha256,
                "actualSha256": actual_hash,
                "expectedByteLength": release.byte_length,
                "actualByteLength": actual_length
            })));
        }
        let package = validate_manager_and_stage(&self.state, &downloaded, &release.version)?;
        if package.byte_length != release.byte_length
            || !package.sha256.eq_ignore_ascii_case(&release.sha256)
        {
            return Err(RpcError::invalid(
                "MANAGER_RELEASE_INTEGRITY_MISMATCH",
                "staged package hash or length differs from the canonical GitHub release metadata",
            )
            .with_details(serde_json::json!({
                "expectedSha256": release.sha256,
                "actualSha256": package.sha256,
                "expectedByteLength": release.byte_length,
                "actualByteLength": package.byte_length
            })));
        }
        Ok((package, release))
    }

    pub async fn prepare_authenticated_asset(
        &self,
        url: &str,
        expected_sha256: &str,
        expected_length: u64,
        expected_name: &str,
        expected_version: &str,
        repository_commit: Option<&str>,
    ) -> RpcResult<PreparedPackage> {
        let url = Url::parse(url)
            .map_err(|error| RpcError::invalid("INVALID_ASSET_URL", error.to_string()))?;
        let downloaded = self.download(url).await?;
        self.prepare_authenticated_download(
            &downloaded,
            expected_sha256,
            expected_length,
            expected_name,
            expected_version,
            repository_commit,
        )
    }

    fn prepare_authenticated_download(
        &self,
        downloaded: &Path,
        expected_sha256: &str,
        expected_length: u64,
        expected_name: &str,
        expected_version: &str,
        repository_commit: Option<&str>,
    ) -> RpcResult<PreparedPackage> {
        let (actual_hash, actual_length) = crate::package::artifact_hash(downloaded)?;
        if actual_length != expected_length || !actual_hash.eq_ignore_ascii_case(expected_sha256) {
            return Err(RpcError::invalid(
                "AUTHENTICATED_ASSET_MISMATCH",
                "download length or SHA-256 differs from registry metadata",
            )
            .with_details(serde_json::json!({
                "expectedSha256": expected_sha256,
                "actualSha256": actual_hash,
                "expectedByteLength": expected_length,
                "actualByteLength": actual_length
            })));
        }

        if let Some(commit) = repository_commit {
            if commit.len() != 40
                || !commit
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            {
                return Err(RpcError::invalid(
                    "INVALID_CATALOG_SNAPSHOT",
                    "catalog repository snapshots require a lowercase 40-character commit",
                ));
            }
            let package = package_repository_archive(&self.state, downloaded)?;
            if !package.name.eq_ignore_ascii_case(expected_name)
                || package.version != expected_version
            {
                return Err(RpcError::invalid(
                    "MANIFEST_MISMATCH",
                    "repository snapshot manifest differs from the authenticated catalog",
                )
                .with_details(serde_json::json!({
                    "expectedName": expected_name,
                    "actualName": package.name,
                    "expectedVersion": expected_version,
                    "actualVersion": package.version
                })));
            }
            return Ok(package);
        }

        let package = validate_and_stage(
            &self.state,
            downloaded,
            ExpectedManifest {
                name: Some(expected_name),
                version: Some(expected_version),
            },
        )?;
        if package.byte_length != expected_length
            || !package.sha256.eq_ignore_ascii_case(expected_sha256)
        {
            return Err(RpcError::invalid(
                "AUTHENTICATED_ASSET_MISMATCH",
                "staged package hash or length differs from registry metadata",
            )
            .with_details(serde_json::json!({
                "expectedSha256": expected_sha256,
                "actualSha256": package.sha256,
                "expectedByteLength": expected_length,
                "actualByteLength": package.byte_length
            })));
        }
        Ok(package)
    }

    async fn resolve_repository(
        &self,
        owner: &str,
        repository: &str,
        requested_ref: Option<&str>,
        selection: Option<String>,
    ) -> RpcResult<ResolveResult> {
        let repository_url = format!("https://github.com/{owner}/{repository}");
        let saved_asset_selection = selection.is_some();
        let (default_reference, mut access) = if requested_ref.is_none() {
            let endpoint = format!("{API_ROOT}/repos/{owner}/{repository}");
            let api_path = format!("repos/{owner}/{repository}");
            let (metadata, resolved_access) = self
                .get_json_with_gh::<Repository>(&endpoint, &api_path)
                .await?;
            (Some(metadata.default_branch), resolved_access)
        } else {
            (None, GitHubAccess::Public)
        };
        if requested_ref.is_none() {
            let endpoint = format!("{API_ROOT}/repos/{owner}/{repository}/releases/latest");
            let api_path = format!("repos/{owner}/{repository}/releases/latest");
            match self
                .get_optional_json_for_access::<Release>(&endpoint, &api_path, access)
                .await?
            {
                Some((release, release_access)) if !release.draft && !release.prerelease => {
                    let mut assets: Vec<_> = release
                        .assets
                        .into_iter()
                        .filter(|asset| {
                            asset
                                .name
                                .to_ascii_lowercase()
                                .ends_with(".aseprite-extension")
                        })
                        .collect();
                    assets.sort_by(|left, right| left.name.cmp(&right.name));
                    if !assets.is_empty() {
                        if assets.len() > 1 && selection.is_none() {
                            return Ok(ResolveResult::SelectionRequired {
                                repository: repository_url,
                                release: release.tag_name,
                                choices: assets
                                    .into_iter()
                                    .map(|asset| AssetChoice {
                                        id: asset.id,
                                        name: asset.name,
                                        byte_length: asset.size,
                                        download_url: asset.browser_download_url,
                                    })
                                    .collect(),
                            });
                        }
                        let selected = if let Some(selection) = selection {
                            assets
                                .into_iter()
                                .find(|asset| {
                                    asset.id.to_string() == selection || asset.name == selection
                                })
                                .ok_or_else(|| {
                                    RpcError::invalid(
                                        "INVALID_ASSET_SELECTION",
                                        "selected release asset is unavailable",
                                    )
                                })?
                        } else {
                            assets.remove(0)
                        };
                        let url = Url::parse(&selected.browser_download_url).map_err(|error| {
                            RpcError::invalid("INVALID_GITHUB_RESPONSE", error.to_string())
                        })?;
                        let downloaded = self
                            .download_release_asset(
                                owner,
                                repository,
                                selected.id,
                                url.clone(),
                                release_access,
                            )
                            .await?;
                        let expected_version = expected_version_from_tag(&release.tag_name);
                        let package = validate_and_stage(
                            &self.state,
                            &downloaded,
                            ExpectedManifest {
                                name: None,
                                version: expected_version.as_deref(),
                            },
                        )?;
                        return Ok(ResolveResult::Ready {
                            package: Box::new(package),
                            source: Box::new(GitHubSource {
                                kind: "github-release".to_owned(),
                                repository: repository_url,
                                immutable_url: url.to_string(),
                                release: Some(release.tag_name),
                                asset_id: Some(selected.id),
                                asset_name: Some(selected.name),
                                commit: None,
                                tracked_ref: None,
                            }),
                        });
                    }
                    let _ = release.html_url;
                }
                _ => {}
            }
            allow_snapshot_after_release_lookup(saved_asset_selection)?;
        }

        let reference = requested_ref
            .map(ToOwned::to_owned)
            .or(default_reference)
            .ok_or_else(|| RpcError::internal("GitHub repository has no reference to resolve"))?;
        let encoded_ref = encode_github_path_value(&reference, "repository ref")?;
        let endpoint = format!("{API_ROOT}/repos/{owner}/{repository}/commits/{encoded_ref}");
        let api_path = format!("repos/{owner}/{repository}/commits/{encoded_ref}");
        let (commit_response, commit_access) = match access {
            GitHubAccess::Public => {
                self.get_json_with_gh::<Commit>(&endpoint, &api_path)
                    .await?
            }
            GitHubAccess::GitHubCli => (
                self.gh_api_json::<Commit>(&api_path).await?,
                GitHubAccess::GitHubCli,
            ),
        };
        if commit_access == GitHubAccess::GitHubCli {
            access = GitHubAccess::GitHubCli;
        }
        let commit = commit_response.sha;
        if commit.len() != 40
            || !commit
                .chars()
                .all(|character| character.is_ascii_hexdigit())
        {
            return Err(RpcError::invalid(
                "INVALID_GITHUB_RESPONSE",
                "GitHub returned an invalid commit identity",
            ));
        }
        let url = Url::parse(&format!(
            "https://codeload.github.com/{owner}/{repository}/zip/{commit}"
        ))
        .map_err(|error| RpcError::internal(error.to_string()))?;
        let downloaded = match access {
            GitHubAccess::Public => match self.download(url.clone()).await {
                Ok(downloaded) => downloaded,
                Err(public_error) if public_request_can_fallback(&public_error) => {
                    let fallback = self
                        .download_snapshot_with_gh(owner, repository, &commit)
                        .await;
                    preserve_public_rate_limit(public_error, fallback)?
                }
                Err(error) => return Err(error),
            },
            GitHubAccess::GitHubCli => {
                self.download_snapshot_with_gh(owner, repository, &commit)
                    .await?
            }
        };
        let package = package_repository_archive(&self.state, &downloaded)?;
        Ok(ResolveResult::Ready {
            package: Box::new(package),
            source: Box::new(GitHubSource {
                kind: "github-snapshot".to_owned(),
                repository: repository_url,
                immutable_url: url.to_string(),
                release: None,
                asset_id: None,
                asset_name: None,
                commit: Some(commit),
                tracked_ref: Some(reference),
            }),
        })
    }

    async fn get_json_with_gh<T>(
        &self,
        public_endpoint: &str,
        gh_endpoint: &str,
    ) -> RpcResult<(T, GitHubAccess)>
    where
        T: for<'de> Deserialize<'de>,
    {
        match self.get_json(public_endpoint).await {
            Ok(value) => Ok((value, GitHubAccess::Public)),
            Err(public_error) if public_request_can_fallback(&public_error) => {
                let fallback = self
                    .gh_api_json(gh_endpoint)
                    .await
                    .map(|value| (value, GitHubAccess::GitHubCli));
                preserve_public_rate_limit(public_error, fallback)
            }
            Err(error) => Err(error),
        }
    }

    async fn get_optional_json_for_access<T>(
        &self,
        public_endpoint: &str,
        gh_endpoint: &str,
        access: GitHubAccess,
    ) -> RpcResult<Option<(T, GitHubAccess)>>
    where
        T: for<'de> Deserialize<'de>,
    {
        match access {
            GitHubAccess::Public => match self.get_json(public_endpoint).await {
                Ok(value) => Ok(Some((value, GitHubAccess::Public))),
                Err(public_error) if public_error.code == "GITHUB_RATE_LIMITED" => {
                    let fallback = self
                        .gh_api_json(gh_endpoint)
                        .await
                        .map(|value| Some((value, GitHubAccess::GitHubCli)))
                        .or_else(|error| {
                            if error.code == "NOT_FOUND" {
                                Ok(None)
                            } else {
                                Err(error)
                            }
                        });
                    preserve_public_rate_limit(public_error, fallback)
                }
                Err(error) if error.code == "NOT_FOUND" => Ok(None),
                Err(error) => Err(error),
            },
            GitHubAccess::GitHubCli => match self.gh_api_json(gh_endpoint).await {
                Ok(value) => Ok(Some((value, GitHubAccess::GitHubCli))),
                Err(error) if error.code == "NOT_FOUND" => Ok(None),
                Err(error) => Err(error),
            },
        }
    }

    async fn download_release_asset(
        &self,
        owner: &str,
        repository: &str,
        asset_id: u64,
        public_url: Url,
        access: GitHubAccess,
    ) -> RpcResult<PathBuf> {
        let mut public_error = None;
        if access == GitHubAccess::Public {
            match self.download(public_url).await {
                Ok(downloaded) => return Ok(downloaded),
                Err(error) if public_request_can_fallback(&error) => public_error = Some(error),
                Err(error) => return Err(error),
            }
        }
        let fallback = self
            .download_release_asset_with_gh(owner, repository, asset_id)
            .await;
        match public_error {
            Some(public_error) => preserve_public_rate_limit(public_error, fallback),
            None => fallback,
        }
    }

    async fn download_release_asset_with_gh(
        &self,
        owner: &str,
        repository: &str,
        asset_id: u64,
    ) -> RpcResult<PathBuf> {
        let endpoint = format!("repos/{owner}/{repository}/releases/assets/{asset_id}");
        self.gh_api_download(&endpoint, "application/octet-stream")
            .await
    }

    async fn download_snapshot_with_gh(
        &self,
        owner: &str,
        repository: &str,
        commit: &str,
    ) -> RpcResult<PathBuf> {
        if commit.len() != 40 || !commit.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(RpcError::invalid(
                "INVALID_GITHUB_RESPONSE",
                "GitHub returned an invalid commit identity",
            ));
        }
        let endpoint = format!("repos/{owner}/{repository}/zipball/{commit}");
        self.gh_api_download(&endpoint, "application/vnd.github+json")
            .await
    }

    async fn gh_api_json<T>(&self, endpoint: &str) -> RpcResult<T>
    where
        T: for<'de> Deserialize<'de>,
    {
        let body = run_gh_api(
            endpoint,
            "application/vnd.github+json",
            GH_JSON_BYTES,
            GH_JSON_TIMEOUT,
            GhOutputKind::Json,
        )
        .await?;
        serde_json::from_slice(&body)
            .map_err(|error| RpcError::invalid("INVALID_GITHUB_RESPONSE", error.to_string()))
    }

    async fn gh_api_download(&self, endpoint: &str, accept: &str) -> RpcResult<PathBuf> {
        let body = run_gh_api(
            endpoint,
            accept,
            MAX_ARCHIVE_BYTES,
            GH_DOWNLOAD_TIMEOUT,
            GhOutputKind::Archive,
        )
        .await?;
        let mut temporary = tempfile::NamedTempFile::new_in(self.state.root().join("staging"))
            .map_err(RpcError::io)?;
        temporary.write_all(&body).map_err(RpcError::io)?;
        temporary.as_file_mut().sync_all().map_err(RpcError::io)?;
        let (path, _, _) = self.state.stage_file(temporary.path())?;
        Ok(path)
    }

    async fn get_json<T>(&self, endpoint: &str) -> RpcResult<T>
    where
        T: for<'de> Deserialize<'de>,
    {
        let key = format!("{}.json", hex::encode(Sha256::digest(endpoint.as_bytes())));
        let cache_path = self.state.http_cache_path(&key);
        let cached = read_http_cache(&cache_path)?;
        let mut request = self
            .client
            .get(endpoint)
            .header("Accept", "application/vnd.github+json");
        if let Some(etag) = cached.as_ref().and_then(|cache| cache.etag.as_deref()) {
            request = request.header(IF_NONE_MATCH, etag);
        }
        let response = match request.send().await {
            Ok(response) => response,
            Err(error) => {
                if let Some(cache) = cached {
                    return serde_json::from_slice(&cache.body).map_err(|parse_error| {
                        RpcError::state(format!("cached GitHub response is invalid: {parse_error}"))
                    });
                }
                return Err(RpcError::network(error.to_string()));
            }
        };
        if response.status() == StatusCode::NOT_MODIFIED {
            let cache = cached.ok_or_else(|| {
                RpcError::state("GitHub returned not-modified without a cached response")
            })?;
            return serde_json::from_slice(&cache.body)
                .map_err(|error| RpcError::state(format!("cached response is invalid: {error}")));
        }
        if response.status() == StatusCode::NOT_FOUND {
            return Err(RpcError::new(
                "NOT_FOUND",
                "GitHub resource was not found",
                false,
            ));
        }
        check_rate_limit(response.status(), response.headers())?;
        if !response.status().is_success() {
            return Err(RpcError::network(format!(
                "GitHub returned HTTP {}",
                response.status()
            )));
        }
        let etag = response
            .headers()
            .get(ETAG)
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned);
        let body = response
            .bytes()
            .await
            .map_err(|error| RpcError::network(error.to_string()))?
            .to_vec();
        let cached = HttpCache {
            etag,
            body: body.clone(),
        };
        let encoded =
            serde_json::to_vec(&cached).map_err(|error| RpcError::internal(error.to_string()))?;
        atomic_write(&cache_path, &encoded).map_err(RpcError::io)?;
        self.state.enforce_http_cache_limit(Some(&cache_path))?;
        serde_json::from_slice(&body)
            .map_err(|error| RpcError::invalid("INVALID_GITHUB_RESPONSE", error.to_string()))
    }

    async fn download(&self, url: Url) -> RpcResult<PathBuf> {
        if url.scheme() != "https" || !is_allowed_download_host(url.host_str().unwrap_or_default())
        {
            return Err(RpcError::invalid(
                "UNTRUSTED_DOWNLOAD_URL",
                "download URL is outside GitHub's public download service",
            ));
        }
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|error| RpcError::network(error.to_string()))?;
        check_rate_limit(response.status(), response.headers())?;
        if matches!(
            response.status(),
            StatusCode::NOT_FOUND | StatusCode::UNAUTHORIZED
        ) || is_github_sign_in_url(response.url())
        {
            return Err(RpcError::new(
                "NOT_FOUND",
                "GitHub download was not found publicly",
                false,
            ));
        }
        if !response.status().is_success() {
            return Err(RpcError::network(format!(
                "download returned HTTP {}",
                response.status()
            )));
        }
        if response.content_length().unwrap_or_default() > MAX_ARCHIVE_BYTES {
            return Err(RpcError::invalid(
                "ARCHIVE_TOO_LARGE",
                "download exceeds the 64 MiB limit",
            ));
        }
        self.stream_download(response).await
    }

    async fn stream_download(&self, mut response: reqwest::Response) -> RpcResult<PathBuf> {
        let mut temporary = tempfile::NamedTempFile::new_in(self.state.root().join("staging"))
            .map_err(RpcError::io)?;
        let mut total = 0_u64;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| RpcError::network(error.to_string()))?
        {
            append_download_chunk(temporary.as_file_mut(), &mut total, &chunk)?;
        }
        temporary.as_file_mut().sync_all().map_err(RpcError::io)?;
        let (path, _, _) = self.state.stage_file(temporary.path())?;
        Ok(path)
    }
}

#[derive(Debug, Deserialize)]
struct GraphQlResponse<T> {
    data: Option<T>,
    #[serde(default)]
    errors: Vec<GraphQlError>,
}

#[derive(Debug, Deserialize)]
struct GraphQlError {
    #[serde(default)]
    message: String,
    #[serde(default)]
    extensions: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct ViewerRepositoryData {
    viewer: ViewerRepositories,
}

#[derive(Debug, Deserialize)]
struct ViewerRepositories {
    repositories: RepositoryConnection,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchRepositoryData {
    search: SearchRepositoryConnection,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RepositoryConnection {
    total_count: u64,
    page_info: RepositoryPageInfo,
    #[serde(default)]
    edges: Vec<RepositoryEdge>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchRepositoryConnection {
    repository_count: u64,
    page_info: RepositoryPageInfo,
    #[serde(default)]
    edges: Vec<RepositoryEdge>,
}

#[derive(Debug, Deserialize)]
struct RepositoryEdge {
    cursor: String,
    node: Option<RepositoryNode>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RepositoryPageInfo {
    has_next_page: bool,
    end_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RepositoryNode {
    name: String,
    owner: RepositoryOwner,
    description: Option<String>,
    is_private: bool,
    is_archived: bool,
    is_fork: bool,
    updated_at: Option<String>,
    viewer_permission: Option<String>,
    manifest: Option<RepositoryManifestProbe>,
    latest_release: Option<RepositoryLatestRelease>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RepositoryManifestProbe {
    id: Option<String>,
    oid: Option<String>,
    #[serde(default)]
    byte_size: u64,
    #[serde(default)]
    is_binary: bool,
    #[serde(default)]
    is_truncated: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RepositoryManifestBlob {
    id: String,
    oid: String,
    #[serde(default)]
    byte_size: u64,
    #[serde(default)]
    is_binary: bool,
    #[serde(default)]
    is_truncated: bool,
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RepositoryLatestRelease {
    #[serde(default)]
    is_draft: bool,
    #[serde(default)]
    is_prerelease: bool,
    release_assets: RepositoryReleaseAssets,
}

#[derive(Debug, Deserialize)]
struct RepositoryReleaseAssets {
    #[serde(default)]
    nodes: Vec<Option<RepositoryReleaseAsset>>,
}

#[derive(Debug, Deserialize)]
struct RepositoryReleaseAsset {
    name: String,
}

#[derive(Debug, Deserialize)]
struct RepositoryManifestData {
    #[serde(default)]
    nodes: Vec<Option<RepositoryManifestBlob>>,
}

#[derive(Debug)]
struct RepositoryBatch {
    edges: Vec<RepositoryEdge>,
    page_info: RepositoryPageInfo,
}

#[derive(Debug, Deserialize)]
struct RepositoryOwner {
    login: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GhOutputKind {
    Json,
    Archive,
}

fn normalize_repository_query(value: Option<String>) -> RpcResult<Option<String>> {
    let value = value.unwrap_or_default();
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if value.len() > 256
        || value.chars().count() > 120
        || value.chars().any(|character| character.is_control())
    {
        return Err(RpcError::invalid(
            "INVALID_GITHUB_QUERY",
            "GitHub repository search must be 120 characters or fewer",
        ));
    }
    Ok(Some(value.to_owned()))
}

fn normalize_repository_cursor(value: Option<String>) -> RpcResult<Option<String>> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_empty()
        || value.len() > 512
        || !value.is_ascii()
        || value
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
    {
        return Err(RpcError::invalid(
            "INVALID_GITHUB_CURSOR",
            "GitHub repository cursor is invalid",
        ));
    }
    Ok(Some(value))
}

fn github_repository_arguments(query: Option<&str>, cursor: Option<&str>) -> Vec<OsString> {
    let graph_query = if query.is_some() {
        GH_REPOSITORY_SEARCH_QUERY
    } else {
        GH_REPOSITORY_QUERY
    };
    let mut arguments = vec![
        OsString::from("api"),
        OsString::from("--hostname"),
        OsString::from("github.com"),
        OsString::from("--method"),
        OsString::from("POST"),
        OsString::from("-H"),
        OsString::from("Accept: application/vnd.github+json"),
        OsString::from("-H"),
        OsString::from("X-GitHub-Api-Version: 2022-11-28"),
        OsString::from("graphql"),
        OsString::from("-f"),
        OsString::from(format!("query={graph_query}")),
        OsString::from("-F"),
        OsString::from(format!("first={GH_REPOSITORY_SCAN_PAGE_SIZE}")),
    ];
    if let Some(cursor) = cursor {
        arguments.push(OsString::from("-f"));
        arguments.push(OsString::from(format!("after={cursor}")));
    }
    if let Some(query) = query {
        arguments.push(OsString::from("-f"));
        arguments.push(OsString::from(format!(
            "search={query} in:name,description"
        )));
    }
    arguments
}

fn github_repository_manifest_arguments(ids: &[String]) -> RpcResult<Vec<OsString>> {
    if ids.is_empty() || ids.len() > GH_REPOSITORY_MANIFEST_BATCH_SIZE {
        return Err(RpcError::invalid(
            "INVALID_GITHUB_RESPONSE",
            "GitHub returned an invalid extension manifest batch",
        ));
    }
    let mut arguments = vec![
        OsString::from("api"),
        OsString::from("--hostname"),
        OsString::from("github.com"),
        OsString::from("--method"),
        OsString::from("POST"),
        OsString::from("-H"),
        OsString::from("Accept: application/vnd.github+json"),
        OsString::from("-H"),
        OsString::from("X-GitHub-Api-Version: 2022-11-28"),
        OsString::from("graphql"),
        OsString::from("-f"),
        OsString::from(format!("query={GH_REPOSITORY_MANIFEST_QUERY}")),
    ];
    for id in ids {
        validate_graph_ql_node_id(id)?;
        arguments.push(OsString::from("-f"));
        arguments.push(OsString::from(format!("ids[]={id}")));
    }
    Ok(arguments)
}

fn validate_graph_ql_node_id(id: &str) -> RpcResult<()> {
    if id.is_empty()
        || id.len() > 512
        || !id.is_ascii()
        || id
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
    {
        return Err(invalid_graph_ql_response());
    }
    Ok(())
}

async fn run_gh_graphql_with_executable(
    executable: &Path,
    arguments: &[OsString],
) -> RpcResult<Vec<u8>> {
    let mut command = Command::new(executable);
    command
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "Never")
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_NO_UPDATE_NOTIFIER", "1")
        .env("GH_NO_EXTENSION_UPDATE_NOTIFIER", "1")
        .env("GH_PAGER", "cat")
        .env("PAGER", "cat")
        .env("NO_COLOR", "1")
        .env("CLICOLOR", "0")
        .kill_on_drop(true);
    configure_hidden_command(&mut command);

    let mut child = command.spawn().map_err(|_| {
        RpcError::invalid(
            "GITHUB_CLI_UNAVAILABLE",
            "GitHub CLI could not be started; reinstall it or check its permissions",
        )
    })?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| RpcError::internal("GitHub CLI stdout was unavailable"))?;
    let operation = async {
        let bytes =
            read_bounded_gh_output(&mut stdout, GH_REPOSITORY_JSON_BYTES, GhOutputKind::Json)
                .await?;
        let status = child.wait().await.map_err(RpcError::io)?;
        Ok::<_, RpcError>((status, bytes))
    };
    let (status, body) = match tokio::time::timeout(GH_JSON_TIMEOUT, operation).await {
        Ok(Ok(outcome)) => outcome,
        Ok(Err(error)) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err(error);
        }
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err(RpcError::new(
                "GITHUB_CLI_TIMEOUT",
                "GitHub CLI did not finish in time",
                true,
            ));
        }
    };
    if status.success() {
        return Ok(body);
    }
    if graph_ql_is_rate_limited(&body) {
        return Err(RpcError::new(
            "GITHUB_RATE_LIMITED",
            "GitHub rate limit was reached",
            true,
        ));
    }
    if status.code() == Some(4) {
        return Err(RpcError::invalid(
            "GITHUB_CLI_AUTH_REQUIRED",
            "GitHub CLI is not signed in; run gh auth login for github.com",
        ));
    }
    Err(RpcError::new(
        "GITHUB_CLI_REQUEST_FAILED",
        "GitHub CLI could not list repositories; check gh auth status and repository access",
        false,
    ))
}

async fn list_repositories_with_executable(
    executable: &Path,
    query: Option<&str>,
    cursor: Option<&str>,
) -> RpcResult<GitHubRepositoryPage> {
    match tokio::time::timeout(
        GH_REPOSITORY_SCAN_TIMEOUT,
        scan_repositories_with_executable(executable, query, cursor),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => Err(RpcError::new(
            "GITHUB_CLI_TIMEOUT",
            "GitHub repository scan did not finish in time",
            true,
        )),
    }
}

async fn scan_repositories_with_executable(
    executable: &Path,
    query: Option<&str>,
    cursor: Option<&str>,
) -> RpcResult<GitHubRepositoryPage> {
    let mut scan_cursor = cursor.map(ToOwned::to_owned);
    let mut scanned = 0_usize;
    let mut matches = Vec::new();
    let mut last_scanned_cursor = None;
    let mut raw_has_next_page = true;

    while matches.len() <= GH_REPOSITORY_PAGE_SIZE
        && scanned < GH_REPOSITORY_SCAN_LIMIT
        && raw_has_next_page
    {
        let arguments = github_repository_arguments(query, scan_cursor.as_deref());
        let body = run_gh_graphql_with_executable(executable, &arguments).await?;
        let batch = parse_repository_batch(&body, query.is_some())?;
        let manifest_texts = load_repository_manifests(executable, &batch.edges).await?;
        raw_has_next_page = batch.page_info.has_next_page;

        for edge in batch.edges {
            let edge_cursor = normalize_repository_cursor(Some(edge.cursor))?
                .ok_or_else(invalid_graph_ql_response)?;
            scanned += 1;
            last_scanned_cursor = Some(edge_cursor.clone());
            if let Some(node) = edge
                .node
                .filter(|node| is_aseprite_repository(node, &manifest_texts))
            {
                matches.push((normalize_repository(node)?, edge_cursor));
                if matches.len() > GH_REPOSITORY_PAGE_SIZE {
                    break;
                }
            }
            if scanned >= GH_REPOSITORY_SCAN_LIMIT {
                break;
            }
        }

        if matches.len() > GH_REPOSITORY_PAGE_SIZE || scanned >= GH_REPOSITORY_SCAN_LIMIT {
            break;
        }
        if raw_has_next_page {
            scan_cursor = normalize_repository_cursor(batch.page_info.end_cursor)?;
            if scan_cursor.is_none() {
                return Err(invalid_graph_ql_response());
            }
        }
    }

    let found_another_match = matches.len() > GH_REPOSITORY_PAGE_SIZE;
    let scan_was_bounded = scanned >= GH_REPOSITORY_SCAN_LIMIT && raw_has_next_page;
    let has_next_page = found_another_match || scan_was_bounded;
    let end_cursor = if found_another_match {
        matches
            .get(GH_REPOSITORY_PAGE_SIZE - 1)
            .map(|(_, cursor)| cursor.clone())
    } else if scan_was_bounded {
        last_scanned_cursor
    } else {
        None
    };
    let repositories = matches
        .into_iter()
        .take(GH_REPOSITORY_PAGE_SIZE)
        .map(|(repository, _)| repository)
        .collect::<Vec<_>>();
    let total_count = repositories.len() as u64 + u64::from(has_next_page);

    Ok(GitHubRepositoryPage {
        repositories,
        total_count,
        has_next_page,
        end_cursor,
    })
}

async fn load_repository_manifests(
    executable: &Path,
    edges: &[RepositoryEdge],
) -> RpcResult<HashMap<String, String>> {
    let expected = edges
        .iter()
        .filter_map(|edge| edge.node.as_ref())
        .filter(|node| !has_aseprite_release_asset(node))
        .filter_map(|node| node.manifest.as_ref())
        .filter(|probe| valid_manifest_probe(probe))
        .filter_map(|probe| Some((probe.id.as_ref()?.clone(), probe)))
        .collect::<HashMap<_, _>>();
    let mut texts = HashMap::new();
    let ids = expected.keys().cloned().collect::<Vec<_>>();
    let requests = ids
        .chunks(GH_REPOSITORY_MANIFEST_BATCH_SIZE)
        .map(|ids| async move {
            let arguments = github_repository_manifest_arguments(ids)?;
            run_gh_graphql_with_executable(executable, &arguments).await
        });
    for body in try_join_all(requests).await? {
        let response: GraphQlResponse<RepositoryManifestData> = parse_graph_ql_response(&body)?;
        let data = response.data.ok_or_else(invalid_graph_ql_response)?;
        for blob in data.nodes.into_iter().flatten() {
            let Some(probe) = expected.get(&blob.id) else {
                return Err(invalid_graph_ql_response());
            };
            if probe.oid.as_deref() != Some(blob.oid.as_str())
                || blob.byte_size != probe.byte_size
                || blob.is_binary != probe.is_binary
                || blob.is_truncated != probe.is_truncated
            {
                return Err(invalid_graph_ql_response());
            }
            let Some(text) = blob.text else {
                continue;
            };
            if text.len() as u64 == blob.byte_size {
                texts.insert(blob.id, text);
            }
        }
    }
    Ok(texts)
}

fn valid_manifest_probe(probe: &RepositoryManifestProbe) -> bool {
    let Some(id) = probe.id.as_deref() else {
        return false;
    };
    let Some(oid) = probe.oid.as_deref() else {
        return false;
    };
    validate_graph_ql_node_id(id).is_ok()
        && (oid.len() == 40 || oid.len() == 64)
        && oid.bytes().all(|byte| byte.is_ascii_hexdigit())
        && !probe.is_binary
        && !probe.is_truncated
        && probe.byte_size > 0
        && probe.byte_size <= GH_REPOSITORY_MANIFEST_BYTES
}

fn parse_repository_batch(body: &[u8], searched: bool) -> RpcResult<RepositoryBatch> {
    if searched {
        let response: GraphQlResponse<SearchRepositoryData> = parse_graph_ql_response(body)?;
        let connection = response.data.ok_or_else(invalid_graph_ql_response)?.search;
        let _ = connection.repository_count;
        normalize_repository_batch(connection.edges, connection.page_info)
    } else {
        let response: GraphQlResponse<ViewerRepositoryData> = parse_graph_ql_response(body)?;
        let connection = response
            .data
            .ok_or_else(invalid_graph_ql_response)?
            .viewer
            .repositories;
        let _ = connection.total_count;
        normalize_repository_batch(connection.edges, connection.page_info)
    }
}

fn parse_graph_ql_response<T>(body: &[u8]) -> RpcResult<GraphQlResponse<T>>
where
    T: for<'de> Deserialize<'de>,
{
    let response: GraphQlResponse<T> =
        serde_json::from_slice(body).map_err(|_| invalid_graph_ql_response())?;
    if graph_ql_errors_are_rate_limited(&response.errors) {
        return Err(RpcError::new(
            "GITHUB_RATE_LIMITED",
            "GitHub rate limit was reached",
            true,
        ));
    }
    if !response.errors.is_empty() {
        return Err(RpcError::invalid(
            "GITHUB_CLI_REQUEST_FAILED",
            "GitHub could not list repositories for the signed-in account",
        ));
    }
    Ok(response)
}

fn invalid_graph_ql_response() -> RpcError {
    RpcError::invalid(
        "INVALID_GITHUB_RESPONSE",
        "GitHub returned an invalid repository list",
    )
}

fn graph_ql_is_rate_limited(body: &[u8]) -> bool {
    serde_json::from_slice::<GraphQlResponse<serde_json::Value>>(body)
        .ok()
        .is_some_and(|response| graph_ql_errors_are_rate_limited(&response.errors))
}

fn graph_ql_errors_are_rate_limited(errors: &[GraphQlError]) -> bool {
    errors.iter().any(|error| {
        error
            .extensions
            .get("type")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|kind| kind.eq_ignore_ascii_case("RATE_LIMITED"))
            || error.message.to_ascii_lowercase().contains("rate limit")
    })
}

fn normalize_repository_batch(
    edges: Vec<RepositoryEdge>,
    page_info: RepositoryPageInfo,
) -> RpcResult<RepositoryBatch> {
    let end_cursor = normalize_repository_cursor(page_info.end_cursor)?;
    if page_info.has_next_page && (end_cursor.is_none() || edges.is_empty()) {
        return Err(invalid_graph_ql_response());
    }
    Ok(RepositoryBatch {
        edges,
        page_info: RepositoryPageInfo {
            has_next_page: page_info.has_next_page,
            end_cursor,
        },
    })
}

fn is_aseprite_repository(node: &RepositoryNode, manifest_texts: &HashMap<String, String>) -> bool {
    has_aseprite_release_asset(node) || has_aseprite_manifest(node, manifest_texts)
}

fn has_aseprite_release_asset(node: &RepositoryNode) -> bool {
    node.latest_release.as_ref().is_some_and(|release| {
        !release.is_draft
            && !release.is_prerelease
            && release.release_assets.nodes.iter().flatten().any(|asset| {
                asset
                    .name
                    .to_ascii_lowercase()
                    .ends_with(".aseprite-extension")
            })
    })
}

fn has_aseprite_manifest(node: &RepositoryNode, manifest_texts: &HashMap<String, String>) -> bool {
    let Some(probe) = node
        .manifest
        .as_ref()
        .filter(|probe| valid_manifest_probe(probe))
    else {
        return false;
    };
    let Some(id) = probe.id.as_deref() else {
        return false;
    };
    let Some(text) = manifest_texts.get(id) else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return false;
    };
    let Some(object) = value.as_object() else {
        return false;
    };
    let valid_name = object
        .get("name")
        .and_then(serde_json::Value::as_str)
        .is_some_and(package_id_is_safe);
    let valid_version = object
        .get("version")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|version| !version.trim().is_empty() && version.len() <= 128);
    if !valid_name || !valid_version {
        return false;
    }
    let Some(contributes) = object
        .get("contributes")
        .and_then(serde_json::Value::as_object)
    else {
        return false;
    };
    contributes.iter().any(|(kind, value)| match kind.as_str() {
        "scripts" => valid_script_contribution(value),
        "keys" | "languages" | "themes" | "palettes" | "ditheringMatrices" => {
            valid_resource_contribution(value)
        }
        _ => false,
    })
}

fn valid_script_contribution(value: &serde_json::Value) -> bool {
    if value.as_str().is_some_and(|path| !path.trim().is_empty()) {
        return true;
    }
    value.as_array().is_some_and(|entries| {
        entries.iter().any(|entry| {
            entry
                .get("path")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|path| !path.trim().is_empty())
        })
    })
}

fn valid_resource_contribution(value: &serde_json::Value) -> bool {
    value.as_array().is_some_and(|entries| {
        entries.iter().any(|entry| {
            let identifier = entry
                .get("id")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|identifier| !identifier.trim().is_empty());
            let path = entry
                .get("path")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|path| !path.trim().is_empty());
            identifier && path
        })
    })
}

fn normalize_repository(node: RepositoryNode) -> RpcResult<GitHubRepository> {
    let owner = validate_identifier(&node.owner.login, "owner")?;
    let repository = validate_identifier(&node.name, "repository")?;
    let name_with_owner = format!("{owner}/{repository}");
    let description = sanitize_repository_text(node.description, 240);
    let updated_at = sanitize_repository_text(node.updated_at, 64);
    let viewer_permission = sanitize_repository_text(node.viewer_permission, 32);
    Ok(GitHubRepository {
        url: format!("https://github.com/{name_with_owner}"),
        name_with_owner,
        description,
        is_private: node.is_private,
        is_archived: node.is_archived,
        is_fork: node.is_fork,
        updated_at,
        viewer_permission,
    })
}

fn sanitize_repository_text(value: Option<String>, maximum_bytes: usize) -> Option<String> {
    let value = value?;
    let mut result = String::new();
    let mut last_was_space = false;
    for character in value.trim().chars() {
        let character = if character.is_control() {
            ' '
        } else {
            character
        };
        let is_space = character.is_whitespace();
        if is_space && last_was_space {
            continue;
        }
        let character = if is_space { ' ' } else { character };
        if result.len() + character.len_utf8() > maximum_bytes {
            break;
        }
        result.push(character);
        last_was_space = is_space;
    }
    let result = result.trim().to_owned();
    (!result.is_empty()).then_some(result)
}

async fn run_gh_api(
    endpoint: &str,
    accept: &str,
    maximum_bytes: u64,
    deadline: Duration,
    output_kind: GhOutputKind,
) -> RpcResult<Vec<u8>> {
    let executable = tooling::find(Tool::Gh).await.ok_or_else(|| {
        RpcError::invalid(
            "GITHUB_CLI_UNAVAILABLE",
            "this repository is not public; install GitHub CLI and run gh auth login",
        )
    })?;
    run_gh_api_with_executable(
        &executable,
        endpoint,
        accept,
        maximum_bytes,
        deadline,
        output_kind,
    )
    .await
}

async fn run_gh_api_with_executable(
    executable: &Path,
    endpoint: &str,
    accept: &str,
    maximum_bytes: u64,
    deadline: Duration,
    output_kind: GhOutputKind,
) -> RpcResult<Vec<u8>> {
    validate_gh_api_endpoint(endpoint)?;
    let mut command = Command::new(executable);
    command
        .arg("api")
        .arg("--hostname")
        .arg("github.com")
        .arg("--method")
        .arg("GET")
        .arg("-H")
        .arg(format!("Accept: {accept}"))
        .arg("-H")
        .arg("X-GitHub-Api-Version: 2022-11-28")
        .arg(endpoint)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_NO_UPDATE_NOTIFIER", "1")
        .env("GH_NO_EXTENSION_UPDATE_NOTIFIER", "1")
        .env("GH_PAGER", "cat")
        .env("PAGER", "cat")
        .env("NO_COLOR", "1")
        .env("CLICOLOR", "0")
        .kill_on_drop(true);
    configure_hidden_command(&mut command);

    let mut child = command.spawn().map_err(|_| {
        RpcError::invalid(
            "GITHUB_CLI_UNAVAILABLE",
            "GitHub CLI could not be started; reinstall it or check its permissions",
        )
    })?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| RpcError::internal("GitHub CLI stdout was unavailable"))?;
    let operation = async {
        let bytes = read_bounded_gh_output(&mut stdout, maximum_bytes, output_kind).await?;
        let status = child.wait().await.map_err(RpcError::io)?;
        Ok::<_, RpcError>((status, bytes))
    };
    let outcome = match tokio::time::timeout(deadline, operation).await {
        Ok(Ok(outcome)) => outcome,
        Ok(Err(error)) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err(error);
        }
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err(RpcError::new(
                "GITHUB_CLI_TIMEOUT",
                "GitHub CLI did not finish in time",
                true,
            ));
        }
    };
    if outcome.0.success() {
        return Ok(outcome.1);
    }
    if gh_json_is_not_found(&outcome.1) {
        return Err(RpcError::new(
            "NOT_FOUND",
            "GitHub resource was not found",
            false,
        ));
    }
    if gh_json_is_rate_limited(&outcome.1) {
        return Err(RpcError::new(
            "GITHUB_RATE_LIMITED",
            "GitHub rate limit was reached",
            true,
        ));
    }
    if outcome.0.code() == Some(4) {
        return Err(RpcError::invalid(
            "GITHUB_CLI_AUTH_REQUIRED",
            "GitHub CLI is not signed in; run gh auth login for github.com",
        ));
    }
    Err(RpcError::new(
        "GITHUB_CLI_REQUEST_FAILED",
        "GitHub CLI could not access this repository; check gh auth status and repository access",
        false,
    ))
}

fn gh_json_is_not_found(body: &[u8]) -> bool {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(body) else {
        return false;
    };
    match value.get("status") {
        Some(serde_json::Value::Number(status)) => status.as_u64() == Some(404),
        Some(serde_json::Value::String(status)) => status == "404",
        _ => false,
    }
}

fn gh_json_is_rate_limited(body: &[u8]) -> bool {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(body) else {
        return false;
    };
    let status = match value.get("status") {
        Some(serde_json::Value::Number(status)) => status.as_u64(),
        Some(serde_json::Value::String(status)) => status.parse().ok(),
        _ => None,
    };
    if status == Some(429) {
        return true;
    }
    status == Some(403)
        && value
            .get("message")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|message| message.to_ascii_lowercase().contains("rate limit"))
}

fn public_request_can_fallback(error: &RpcError) -> bool {
    matches!(error.code.as_str(), "NOT_FOUND" | "GITHUB_RATE_LIMITED")
}

fn allow_snapshot_after_release_lookup(saved_asset_selection: bool) -> RpcResult<()> {
    if saved_asset_selection {
        return Err(RpcError::invalid(
            "SAVED_RELEASE_ASSET_UNAVAILABLE",
            "the saved GitHub release asset is not available in the latest stable release",
        ));
    }
    Ok(())
}

fn preserve_public_rate_limit<T>(public_error: RpcError, fallback: RpcResult<T>) -> RpcResult<T> {
    match fallback {
        Err(error)
            if public_error.code == "GITHUB_RATE_LIMITED"
                && error.code == "GITHUB_CLI_UNAVAILABLE" =>
        {
            Err(public_error)
        }
        result => result,
    }
}

async fn read_bounded_gh_output<R>(
    reader: &mut R,
    maximum_bytes: u64,
    output_kind: GhOutputKind,
) -> RpcResult<Vec<u8>>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut bytes = Vec::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let count = reader.read(&mut buffer).await.map_err(RpcError::io)?;
        if count == 0 {
            return Ok(bytes);
        }
        if (bytes.len() as u64).saturating_add(count as u64) > maximum_bytes {
            let (code, message) = match output_kind {
                GhOutputKind::Json => (
                    "INVALID_GITHUB_RESPONSE",
                    "GitHub CLI returned an unexpectedly large API response",
                ),
                GhOutputKind::Archive => ("ARCHIVE_TOO_LARGE", "download exceeds the 64 MiB limit"),
            };
            return Err(RpcError::invalid(code, message));
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
}

fn validate_gh_api_endpoint(endpoint: &str) -> RpcResult<()> {
    let valid_percent_encoding = endpoint.as_bytes().iter().enumerate().all(|(index, byte)| {
        *byte != b'%'
            || endpoint
                .as_bytes()
                .get(index + 1..index + 3)
                .map(|digits| digits.iter().all(u8::is_ascii_hexdigit))
                .unwrap_or(false)
    });
    if endpoint.len() > 1024
        || !endpoint.starts_with("repos/")
        || endpoint
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..")
        || !endpoint.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'.' | b'_' | b'~' | b'%')
        })
        || !valid_percent_encoding
    {
        return Err(RpcError::invalid(
            "INVALID_GITHUB_URL",
            "GitHub API endpoint is invalid",
        ));
    }
    Ok(())
}

fn encode_github_path_value(value: &str, label: &str) -> RpcResult<String> {
    if value.is_empty()
        || value.len() > 255
        || value
            .chars()
            .any(|character| character.is_control() || character == '\0')
    {
        return Err(RpcError::invalid(
            "INVALID_GITHUB_URL",
            format!("invalid GitHub {label}"),
        ));
    }
    Ok(utf8_percent_encode(value, NON_ALPHANUMERIC).to_string())
}

fn configure_hidden_command(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

        command.as_std_mut().creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = command;
}

fn select_manager_release(
    owner: &str,
    repository: &str,
    release: &Release,
) -> RpcResult<Option<ManagerRelease>> {
    if release.draft || release.prerelease {
        return Ok(None);
    }

    let version = manager_version_from_tag(&release.tag_name).ok_or_else(|| {
        RpcError::invalid(
            "INVALID_MANAGER_RELEASE_TAG",
            "manager releases must use a stable vMAJOR.MINOR.PATCH tag",
        )
    })?;
    if release.assets.is_empty() {
        return Ok(None);
    }

    let asset_name = format!("aseprite-extension-manager-{version}.aseprite-extension");
    let matching: Vec<_> = release
        .assets
        .iter()
        .filter(|asset| asset.name == asset_name)
        .collect();
    let asset = match matching.as_slice() {
        [] => {
            return Err(RpcError::invalid(
                "MANAGER_RELEASE_ASSET_MISSING",
                "manager release does not contain the expected extension asset",
            ));
        }
        [asset] => *asset,
        _ => {
            return Err(RpcError::invalid(
                "MANAGER_RELEASE_ASSET_AMBIGUOUS",
                "manager release contains more than one expected extension asset",
            ));
        }
    };
    if asset.size == 0 || asset.size > MAX_ARCHIVE_BYTES {
        return Err(RpcError::invalid(
            "MANAGER_RELEASE_ASSET_SIZE",
            "manager release asset has an invalid size",
        )
        .with_details(serde_json::json!({
            "byteLength": asset.size,
            "maximumByteLength": MAX_ARCHIVE_BYTES
        })));
    }
    let sha256 = asset
        .digest
        .as_deref()
        .and_then(|digest| digest.strip_prefix("sha256:"))
        .filter(|digest| {
            digest.len() == 64
                && digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
        .ok_or_else(|| {
            RpcError::invalid(
                "MANAGER_RELEASE_DIGEST_MISSING",
                "manager release asset is missing its canonical SHA-256 digest",
            )
        })?
        .to_owned();

    let expected_url = Url::parse(&format!(
        "https://github.com/{owner}/{repository}/releases/download/{}/{asset_name}",
        release.tag_name
    ))
    .map_err(|error| RpcError::internal(error.to_string()))?;
    let actual_url = Url::parse(&asset.browser_download_url)
        .map_err(|error| RpcError::invalid("INVALID_MANAGER_RELEASE_URL", error.to_string()))?;
    if actual_url != expected_url {
        return Err(RpcError::invalid(
            "INVALID_MANAGER_RELEASE_URL",
            "manager release asset URL is not canonical for this repository and tag",
        ));
    }

    Ok(Some(ManagerRelease {
        version,
        tag: release.tag_name.clone(),
        repository: format!("https://github.com/{owner}/{repository}"),
        asset_id: asset.id,
        asset_name,
        byte_length: asset.size,
        sha256,
        download_url: expected_url.to_string(),
    }))
}

fn manager_version_from_tag(tag: &str) -> Option<String> {
    let value = tag.strip_prefix('v')?;
    let version = semver::Version::parse(value).ok()?;
    if version.pre.is_empty() && version.build.is_empty() && version.to_string() == value {
        Some(value.to_owned())
    } else {
        None
    }
}

pub fn parse_github_url(value: &str) -> RpcResult<GitHubTarget> {
    let url = Url::parse(value)
        .map_err(|error| RpcError::invalid("INVALID_GITHUB_URL", error.to_string()))?;
    if url.scheme() != "https" || url.host_str() != Some("github.com") {
        return Err(RpcError::invalid(
            "INVALID_GITHUB_URL",
            "only https://github.com URLs are supported",
        ));
    }
    if url.username() != ""
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(RpcError::invalid(
            "INVALID_GITHUB_URL",
            "credentials, query strings, fragments, and custom ports are unsupported",
        ));
    }
    let segments: Vec<String> = url
        .path_segments()
        .map(|segments| {
            segments
                .filter(|segment| !segment.is_empty())
                .map(|segment| decode_github_path_segment(segment, "URL path"))
                .collect::<RpcResult<Vec<_>>>()
        })
        .transpose()?
        .unwrap_or_default();
    if segments.len() < 2 {
        return Err(RpcError::invalid(
            "INVALID_GITHUB_URL",
            "URL must identify a GitHub repository",
        ));
    }
    let owner = validate_identifier(&segments[0], "owner")?;
    let repository = validate_identifier(
        segments[1].strip_suffix(".git").unwrap_or(&segments[1]),
        "repository",
    )?;
    if segments.len() >= 6 && segments[2] == "releases" && segments[3] == "download" {
        let tag = segments[4].to_owned();
        let asset_name = segments[5..].join("/");
        return Ok(GitHubTarget::ReleaseAsset {
            owner,
            repository,
            tag,
            asset_name,
            url,
        });
    }
    if segments.len() >= 4 && segments[2] == "tree" {
        return Ok(GitHubTarget::Repository {
            owner,
            repository,
            requested_ref: Some(segments[3..].join("/")),
        });
    }
    if segments.len() > 2 {
        return Err(RpcError::invalid(
            "INVALID_GITHUB_URL",
            "URL must identify a repository or release asset",
        ));
    }
    Ok(GitHubTarget::Repository {
        owner,
        repository,
        requested_ref: None,
    })
}

fn validate_identifier(value: &str, label: &str) -> RpcResult<String> {
    if value.is_empty()
        || value.len() > 100
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "-._".contains(character))
    {
        return Err(RpcError::invalid(
            "INVALID_GITHUB_URL",
            format!("invalid GitHub {label}"),
        ));
    }
    Ok(value.to_owned())
}

fn decode_github_path_segment(value: &str, label: &str) -> RpcResult<String> {
    let bytes = value.as_bytes();
    let valid_percent_encoding = bytes.iter().enumerate().all(|(index, byte)| {
        *byte != b'%'
            || bytes
                .get(index + 1..index + 3)
                .map(|digits| digits.iter().all(u8::is_ascii_hexdigit))
                .unwrap_or(false)
    });
    if !valid_percent_encoding {
        return Err(RpcError::invalid(
            "INVALID_GITHUB_URL",
            format!("invalid GitHub {label}"),
        ));
    }
    let decoded = percent_decode_str(value).decode_utf8().map_err(|_| {
        RpcError::invalid(
            "INVALID_GITHUB_URL",
            format!("invalid UTF-8 in GitHub {label}"),
        )
    })?;
    if decoded
        .chars()
        .any(|character| character.is_control() || character == '\0')
    {
        return Err(RpcError::invalid(
            "INVALID_GITHUB_URL",
            format!("invalid GitHub {label}"),
        ));
    }
    Ok(decoded.into_owned())
}

fn is_allowed_download_host(host: &str) -> bool {
    matches!(
        host,
        "github.com"
            | "api.github.com"
            | "codeload.github.com"
            | "objects.githubusercontent.com"
            | "release-assets.githubusercontent.com"
    ) || host.ends_with(".githubusercontent.com")
}

fn is_github_sign_in_url(url: &Url) -> bool {
    url.host_str() == Some("github.com")
        && matches!(url.path(), "/login" | "/session" | "/sessions/two-factor")
}

fn check_rate_limit(status: StatusCode, headers: &HeaderMap) -> RpcResult<()> {
    let remaining = headers
        .get("x-ratelimit-remaining")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    if status == StatusCode::TOO_MANY_REQUESTS
        || ((status == StatusCode::FORBIDDEN || status == StatusCode::UNAUTHORIZED)
            && remaining == Some(0))
    {
        let reset = headers
            .get("x-ratelimit-reset")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok());
        return Err(
            RpcError::new("GITHUB_RATE_LIMITED", "GitHub rate limit was reached", true)
                .with_details(serde_json::json!({ "resetUnix": reset })),
        );
    }
    Ok(())
}

fn read_http_cache(path: &std::path::Path) -> RpcResult<Option<HttpCache>> {
    if !path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(path).map_err(RpcError::io)?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|error| RpcError::state(format!("invalid HTTP cache: {error}")))
}

fn append_download_chunk(output: &mut fs::File, total: &mut u64, chunk: &[u8]) -> RpcResult<()> {
    *total = total.saturating_add(chunk.len() as u64);
    if *total > MAX_ARCHIVE_BYTES {
        return Err(RpcError::invalid(
            "ARCHIVE_TOO_LARGE",
            "download exceeds the 64 MiB limit",
        ));
    }
    output.write_all(chunk).map_err(RpcError::io)
}

fn expected_version_from_tag(tag: &str) -> Option<String> {
    let value = tag.strip_prefix('v').unwrap_or(tag);
    let version = semver::Version::parse(value).ok()?;
    if version.pre.is_empty() && version.build.is_empty() && version.to_string() == value {
        Some(value.to_owned())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    const MANAGER_OWNER: &str = "soupmasters";
    const MANAGER_REPOSITORY: &str = "AsepriteExtensionManager";

    fn repository_node() -> RepositoryNode {
        RepositoryNode {
            name: "sample-extension".to_owned(),
            owner: RepositoryOwner {
                login: "example".to_owned(),
            },
            description: None,
            is_private: false,
            is_archived: false,
            is_fork: false,
            updated_at: None,
            viewer_permission: Some("READ".to_owned()),
            manifest: None,
            latest_release: None,
        }
    }

    fn repository_with_manifest(text: &str) -> (RepositoryNode, HashMap<String, String>) {
        let mut node = repository_node();
        node.manifest = Some(RepositoryManifestProbe {
            id: Some("manifest-node-id".to_owned()),
            oid: Some("1".repeat(40)),
            byte_size: text.len() as u64,
            is_binary: false,
            is_truncated: false,
        });
        let manifests = HashMap::from([("manifest-node-id".to_owned(), text.to_owned())]);
        (node, manifests)
    }

    fn repository_with_release(asset: &str, draft: bool, prerelease: bool) -> RepositoryNode {
        let mut node = repository_node();
        node.latest_release = Some(RepositoryLatestRelease {
            is_draft: draft,
            is_prerelease: prerelease,
            release_assets: RepositoryReleaseAssets {
                nodes: vec![Some(RepositoryReleaseAsset {
                    name: asset.to_owned(),
                })],
            },
        });
        node
    }

    fn write_repository_snapshot(path: &Path, name: &str, version: &str) {
        let output = fs::File::create(path).unwrap();
        let mut archive = zip::ZipWriter::new(output);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        archive.add_directory("sample-commit/", options).unwrap();
        archive
            .start_file("sample-commit/package.json", options)
            .unwrap();
        archive
            .write_all(
                serde_json::json!({
                    "name": name,
                    "displayName": "Sample",
                    "version": version,
                    "contributes": {
                        "scripts": [{"path": "./sample.lua"}]
                    }
                })
                .to_string()
                .as_bytes(),
            )
            .unwrap();
        archive
            .start_file("sample-commit/sample.lua", options)
            .unwrap();
        archive.write_all(b"return true\n").unwrap();
        archive.finish().unwrap();
    }

    #[test]
    fn authenticated_catalog_snapshot_is_normalized_after_source_verification() {
        let temporary = tempfile::tempdir().unwrap();
        let snapshot = temporary.path().join("snapshot.zip");
        write_repository_snapshot(&snapshot, "sample", "1.2.3");
        let (sha256, byte_length) = crate::package::artifact_hash(&snapshot).unwrap();
        let state = State::new(temporary.path().join("state")).unwrap();
        let github = GitHubClient::new(state).unwrap();
        let commit = "1".repeat(40);

        let package = github
            .prepare_authenticated_download(
                &snapshot,
                &sha256,
                byte_length,
                "sample",
                "1.2.3",
                Some(&commit),
            )
            .unwrap();

        assert_eq!(package.name, "sample");
        assert_eq!(package.version, "1.2.3");
        assert!(package.artifact_path.is_file());
        assert_ne!(
            package.sha256, sha256,
            "normalized archive has its own digest"
        );

        let mismatch = github
            .prepare_authenticated_download(
                &snapshot,
                &sha256,
                byte_length,
                "different",
                "1.2.3",
                Some(&commit),
            )
            .expect_err("authenticated manifest identity must match");
        assert_eq!(mismatch.code, "MANIFEST_MISMATCH");
    }

    fn manager_asset(version: &str, id: u64) -> ReleaseAsset {
        let name = format!("aseprite-extension-manager-{version}.aseprite-extension");
        ReleaseAsset {
            id,
            browser_download_url: format!(
                "https://github.com/{MANAGER_OWNER}/{MANAGER_REPOSITORY}/releases/download/v{version}/{name}"
            ),
            name,
            size: 1_024,
            digest: Some(format!("sha256:{}", "a".repeat(64))),
        }
    }

    fn manager_release(tag: &str, assets: Vec<ReleaseAsset>) -> Release {
        Release {
            tag_name: tag.to_owned(),
            html_url: format!(
                "https://github.com/{MANAGER_OWNER}/{MANAGER_REPOSITORY}/releases/tag/{tag}"
            ),
            draft: false,
            prerelease: false,
            assets,
        }
    }

    #[test]
    fn selects_canonical_stable_manager_release() {
        let release = manager_release("v1.2.3", vec![manager_asset("1.2.3", 42)]);

        let selected = select_manager_release(MANAGER_OWNER, MANAGER_REPOSITORY, &release)
            .unwrap()
            .expect("installable release");

        assert_eq!(selected.version, "1.2.3");
        assert_eq!(selected.tag, "v1.2.3");
        assert_eq!(
            selected.repository,
            "https://github.com/soupmasters/AsepriteExtensionManager"
        );
        assert_eq!(selected.asset_id, 42);
        assert_eq!(
            selected.asset_name,
            "aseprite-extension-manager-1.2.3.aseprite-extension"
        );
        assert_eq!(selected.byte_length, 1_024);
        assert_eq!(selected.sha256, "a".repeat(64));
        assert_eq!(
            selected.download_url,
            "https://github.com/soupmasters/AsepriteExtensionManager/releases/download/v1.2.3/aseprite-extension-manager-1.2.3.aseprite-extension"
        );
    }

    #[test]
    fn rejects_manager_release_without_exact_stable_tag() {
        for tag in ["1.2.3", "v1.2", "v1.2.3-beta.1", "v1.2.3+build.1"] {
            let release = manager_release(tag, vec![manager_asset("1.2.3", 42)]);
            let error = select_manager_release(MANAGER_OWNER, MANAGER_REPOSITORY, &release)
                .expect_err("tag must be canonical and stable");
            assert_eq!(error.code, "INVALID_MANAGER_RELEASE_TAG", "tag: {tag}");
        }
    }

    #[test]
    fn rejects_wrong_or_multiple_manager_assets() {
        let mut wrong = manager_asset("1.2.3", 42);
        wrong.name = "aseprite-extension-manager.aseprite-extension".to_owned();
        let wrong_release = manager_release("v1.2.3", vec![wrong]);
        let error = select_manager_release(MANAGER_OWNER, MANAGER_REPOSITORY, &wrong_release)
            .expect_err("wrong asset name must not be selected");
        assert_eq!(error.code, "MANAGER_RELEASE_ASSET_MISSING");

        let duplicate_release = manager_release(
            "v1.2.3",
            vec![manager_asset("1.2.3", 42), manager_asset("1.2.3", 43)],
        );
        let error = select_manager_release(MANAGER_OWNER, MANAGER_REPOSITORY, &duplicate_release)
            .expect_err("duplicate canonical assets are ambiguous");
        assert_eq!(error.code, "MANAGER_RELEASE_ASSET_AMBIGUOUS");
    }

    #[test]
    fn rejects_noncanonical_or_oversize_manager_asset() {
        let mut missing_digest = manager_asset("1.2.3", 42);
        missing_digest.digest = None;
        let release = manager_release("v1.2.3", vec![missing_digest]);
        let error = select_manager_release(MANAGER_OWNER, MANAGER_REPOSITORY, &release)
            .expect_err("release asset digest is required");
        assert_eq!(error.code, "MANAGER_RELEASE_DIGEST_MISSING");

        let mut noncanonical = manager_asset("1.2.3", 42);
        noncanonical.browser_download_url = format!(
            "https://github.com/other/{MANAGER_REPOSITORY}/releases/download/v1.2.3/{}",
            noncanonical.name
        );
        let release = manager_release("v1.2.3", vec![noncanonical]);
        let error = select_manager_release(MANAGER_OWNER, MANAGER_REPOSITORY, &release)
            .expect_err("asset URL must belong to the canonical repository");
        assert_eq!(error.code, "INVALID_MANAGER_RELEASE_URL");

        let mut oversize = manager_asset("1.2.3", 42);
        oversize.size = MAX_ARCHIVE_BYTES + 1;
        let release = manager_release("v1.2.3", vec![oversize]);
        let error = select_manager_release(MANAGER_OWNER, MANAGER_REPOSITORY, &release)
            .expect_err("oversize release asset must be rejected");
        assert_eq!(error.code, "MANAGER_RELEASE_ASSET_SIZE");
    }

    #[test]
    fn ignores_draft_prerelease_and_empty_manager_releases() {
        let mut prerelease = manager_release("v1.2.3", vec![manager_asset("1.2.3", 42)]);
        prerelease.prerelease = true;
        assert!(
            select_manager_release(MANAGER_OWNER, MANAGER_REPOSITORY, &prerelease)
                .unwrap()
                .is_none()
        );

        let mut draft = manager_release("v1.2.3", vec![manager_asset("1.2.3", 42)]);
        draft.draft = true;
        assert!(
            select_manager_release(MANAGER_OWNER, MANAGER_REPOSITORY, &draft)
                .unwrap()
                .is_none()
        );

        let empty = manager_release("v1.2.3", Vec::new());
        assert!(
            select_manager_release(MANAGER_OWNER, MANAGER_REPOSITORY, &empty)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn parses_repository_release_and_ref_urls() {
        assert_eq!(
            parse_github_url("https://github.com/aseprite/Attachment-System").unwrap(),
            GitHubTarget::Repository {
                owner: "aseprite".to_owned(),
                repository: "Attachment-System".to_owned(),
                requested_ref: None
            }
        );
        assert!(matches!(
            parse_github_url(
                "https://github.com/example/sample/releases/download/v1/sample.aseprite-extension"
            )
            .unwrap(),
            GitHubTarget::ReleaseAsset { tag, .. } if tag == "v1"
        ));
        assert!(matches!(
            parse_github_url("https://github.com/example/sample/tree/feature/test").unwrap(),
            GitHubTarget::Repository {
                requested_ref: Some(reference),
                ..
            } if reference == "feature/test"
        ));
        assert!(matches!(
            parse_github_url("https://github.com/example/sample/tree/main").unwrap(),
            GitHubTarget::Repository {
                requested_ref: Some(reference),
                ..
            } if reference == "main"
        ));
        assert!(matches!(
            parse_github_url(
                "https://github.com/example/sample/releases/download/release%2F1/My%20Tool-%E2%9C%93.aseprite-extension"
            )
            .unwrap(),
            GitHubTarget::ReleaseAsset {
                tag,
                asset_name,
                ..
            } if tag == "release/1" && asset_name == "My Tool-✓.aseprite-extension"
        ));
        assert!(matches!(
            parse_github_url("https://github.com/example/sample/tree/feature%2Fprivate").unwrap(),
            GitHubTarget::Repository {
                requested_ref: Some(reference),
                ..
            } if reference == "feature/private"
        ));
    }

    #[test]
    fn snapshot_source_serializes_its_tracked_ref() {
        let source = GitHubSource {
            kind: "github-snapshot".to_owned(),
            repository: "https://github.com/example/sample".to_owned(),
            immutable_url:
                "https://codeload.github.com/example/sample/zip/1111111111111111111111111111111111111111"
                    .to_owned(),
            release: None,
            asset_id: None,
            asset_name: None,
            commit: Some("1".repeat(40)),
            tracked_ref: Some("release/1.x".to_owned()),
        };
        let value = serde_json::to_value(source).unwrap();
        assert_eq!(value["trackedRef"], "release/1.x");
        assert_eq!(value["commit"], "1".repeat(40));
    }

    #[test]
    fn saved_release_asset_cannot_fall_through_to_a_snapshot() {
        let error = allow_snapshot_after_release_lookup(true)
            .expect_err("saved release lineage must not change to a branch snapshot");
        assert_eq!(error.code, "SAVED_RELEASE_ASSET_UNAVAILABLE");
        allow_snapshot_after_release_lookup(false).unwrap();
    }

    #[test]
    fn refuses_non_github_and_ambiguous_urls() {
        assert!(parse_github_url("http://github.com/example/sample").is_err());
        assert!(parse_github_url("https://git.example.com/example/sample").is_err());
        assert!(parse_github_url("https://github.com/example/sample/issues/1").is_err());
        assert!(parse_github_url("https://token@github.com/example/sample").is_err());
        assert!(parse_github_url("https://github.com/example/sample?token=secret").is_err());
        assert!(parse_github_url("https://github.com/example/sample#secret").is_err());
        assert!(parse_github_url("https://github.com/example/sample/tree/%FF").is_err());
        assert!(parse_github_url("https://github.com/example/sample/tree/%ZZ").is_err());
    }

    #[test]
    fn recognizes_github_sign_in_redirects_for_private_downloads() {
        assert!(is_github_sign_in_url(
            &Url::parse("https://github.com/login?return_to=%2Fprivate%2Frepo").unwrap()
        ));
        assert!(!is_github_sign_in_url(
            &Url::parse("https://github.com/example/sample/releases/download/v1/a.zip").unwrap()
        ));
        assert!(!is_github_sign_in_url(
            &Url::parse("https://example.com/login").unwrap()
        ));
    }

    #[test]
    fn recognizes_rate_limit_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("x-ratelimit-remaining", "0".parse().unwrap());
        headers.insert("x-ratelimit-reset", "123".parse().unwrap());
        let error = check_rate_limit(StatusCode::FORBIDDEN, &headers).expect_err("limited");
        assert_eq!(error.code, "GITHUB_RATE_LIMITED");
        assert!(error.retryable);
    }

    #[test]
    fn plain_release_tags_require_matching_versions() {
        assert_eq!(
            expected_version_from_tag("v1.2.3").as_deref(),
            Some("1.2.3")
        );
        assert_eq!(expected_version_from_tag("1.2.3").as_deref(), Some("1.2.3"));
        assert_eq!(expected_version_from_tag("release-1.2.3"), None);
        assert_eq!(expected_version_from_tag("v1.2.3-beta.1"), None);
    }

    #[test]
    fn chunked_download_is_bounded_before_writing_oversize_chunk() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("download");
        let mut output = fs::File::create(&path).unwrap();
        let mut total = MAX_ARCHIVE_BYTES - 2;
        append_download_chunk(&mut output, &mut total, b"ok").unwrap();
        let error =
            append_download_chunk(&mut output, &mut total, b"x").expect_err("oversize rejected");
        assert_eq!(error.code, "ARCHIVE_TOO_LARGE");
        assert_eq!(fs::metadata(path).unwrap().len(), 2);
    }

    #[test]
    fn gh_api_endpoints_are_strictly_scoped_to_repository_paths() {
        for endpoint in [
            "repos/example/sample",
            "repos/example/sample/releases/assets/42",
            "repos/example/sample/commits/feature%2Fprivate",
            "repos/example/sample/zipball/1111111111111111111111111111111111111111",
        ] {
            validate_gh_api_endpoint(endpoint).unwrap();
        }

        for endpoint in [
            "user",
            "repos/example/../other",
            "repos/example/sample?token=value",
            "repos/example/sample/%ZZ",
            "repos/example/sample\n--hostname",
            "repos//sample",
        ] {
            assert!(
                validate_gh_api_endpoint(endpoint).is_err(),
                "endpoint should be rejected: {endpoint:?}"
            );
        }
    }

    #[tokio::test]
    async fn missing_gh_executable_is_reported_without_a_shell_fallback() {
        let temporary = tempfile::tempdir().unwrap();
        let error = run_gh_api_with_executable(
            &temporary.path().join("missing-gh"),
            "repos/example/sample",
            "application/vnd.github+json",
            GH_JSON_BYTES,
            GH_JSON_TIMEOUT,
            GhOutputKind::Json,
        )
        .await
        .expect_err("a missing GitHub CLI executable must fail");
        assert_eq!(error.code, "GITHUB_CLI_UNAVAILABLE");
    }

    #[cfg(unix)]
    fn fake_gh(script: &str) -> (tempfile::TempDir, PathBuf) {
        use std::os::unix::fs::PermissionsExt;

        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("gh");
        fs::write(&path, script).unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&path, permissions).unwrap();
        (temporary, path)
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn gh_api_uses_fixed_arguments_and_maps_cli_failures() {
        let (success_directory, success) = fake_gh(
            r#"#!/bin/sh
test "$#" -eq 10 || exit 9
test "$1" = "api" || exit 9
test "$2" = "--hostname" || exit 9
test "$3" = "github.com" || exit 9
test "$4" = "--method" || exit 9
test "$5" = "GET" || exit 9
test "${10}" = "repos/example/sample" || exit 9
printf '%s' '{"ok":true}'
"#,
        );
        let output = run_gh_api_with_executable(
            &success,
            "repos/example/sample",
            "application/vnd.github+json",
            GH_JSON_BYTES,
            GH_JSON_TIMEOUT,
            GhOutputKind::Json,
        )
        .await
        .unwrap();
        assert_eq!(output, br#"{"ok":true}"#);
        drop(success_directory);

        let (_auth_directory, auth) = fake_gh("#!/bin/sh\nexit 4\n");
        let auth_error = run_gh_api_with_executable(
            &auth,
            "repos/example/sample",
            "application/vnd.github+json",
            GH_JSON_BYTES,
            GH_JSON_TIMEOUT,
            GhOutputKind::Json,
        )
        .await
        .expect_err("exit 4 means gh authentication is required");
        assert_eq!(auth_error.code, "GITHUB_CLI_AUTH_REQUIRED");

        let (_rate_directory, rate) = fake_gh(
            "#!/bin/sh\nprintf '%s' '{\"message\":\"API rate limit exceeded\",\"status\":\"403\"}'\nexit 1\n",
        );
        let rate_error = run_gh_api_with_executable(
            &rate,
            "repos/example/sample",
            "application/octet-stream",
            GH_JSON_BYTES,
            GH_JSON_TIMEOUT,
            GhOutputKind::Archive,
        )
        .await
        .expect_err("GitHub CLI rate limits must stay retryable");
        assert_eq!(rate_error.code, "GITHUB_RATE_LIMITED");
        assert!(rate_error.retryable);

        let (_missing_directory, missing) = fake_gh(
            "#!/bin/sh\nprintf '%s' '{\"message\":\"Not Found\",\"status\":404}'\nexit 1\n",
        );
        let missing_error = run_gh_api_with_executable(
            &missing,
            "repos/example/sample/releases/assets/42",
            "application/octet-stream",
            GH_JSON_BYTES,
            GH_JSON_TIMEOUT,
            GhOutputKind::Archive,
        )
        .await
        .expect_err("archive endpoint errors still contain GitHub JSON");
        assert_eq!(missing_error.code, "NOT_FOUND");
    }

    #[test]
    fn only_explicit_gh_json_404_responses_are_optional() {
        assert!(gh_json_is_not_found(
            br#"{"message":"Not Found","status":"404"}"#
        ));
        assert!(gh_json_is_not_found(
            br#"{"message":"Not Found","status":404}"#
        ));
        assert!(!gh_json_is_not_found(br#"{"message":"Not Found"}"#));
        assert!(!gh_json_is_not_found(br#"{"status":"401"}"#));
        assert!(!gh_json_is_not_found(b"not json"));
    }

    #[test]
    fn classifies_explicit_gh_rate_limit_responses() {
        assert!(gh_json_is_rate_limited(
            br#"{"message":"API rate limit exceeded","status":"403"}"#
        ));
        assert!(gh_json_is_rate_limited(
            br#"{"message":"Too many requests","status":429}"#
        ));
        assert!(!gh_json_is_rate_limited(
            br#"{"message":"Resource not accessible","status":"403"}"#
        ));
        assert!(!gh_json_is_rate_limited(b"not json"));

        let not_found = RpcError::new("NOT_FOUND", "missing", false);
        let limited = RpcError::new("GITHUB_RATE_LIMITED", "limited", true);
        let denied = RpcError::new("GITHUB_CLI_REQUEST_FAILED", "denied", false);
        assert!(public_request_can_fallback(&not_found));
        assert!(public_request_can_fallback(&limited));
        assert!(!public_request_can_fallback(&denied));

        let unavailable = RpcError::new("GITHUB_CLI_UNAVAILABLE", "missing", false);
        let preserved = preserve_public_rate_limit::<()>(limited, Err(unavailable))
            .expect_err("the original public rate limit should remain actionable");
        assert_eq!(preserved.code, "GITHUB_RATE_LIMITED");
    }

    #[test]
    fn github_refs_are_encoded_as_one_api_path_value() {
        assert_eq!(
            encode_github_path_value("feature/private build", "repository ref").unwrap(),
            "feature%2Fprivate%20build"
        );
        assert!(encode_github_path_value("bad\nref", "repository ref").is_err());
        assert!(encode_github_path_value(&"x".repeat(256), "repository ref").is_err());
    }

    #[test]
    fn repository_browser_parameters_are_bounded_and_normalized() {
        assert_eq!(
            normalize_repository_query(Some("  animation tools  ".to_owned())).unwrap(),
            Some("animation tools".to_owned())
        );
        assert_eq!(
            normalize_repository_query(Some("  ".to_owned())).unwrap(),
            None
        );
        assert!(normalize_repository_query(Some("x".repeat(121))).is_err());
        assert!(normalize_repository_query(Some("bad\nquery".to_owned())).is_err());

        assert_eq!(
            normalize_repository_cursor(Some("Y3Vyc29yOnYyOpHO".to_owned())).unwrap(),
            Some("Y3Vyc29yOnYyOpHO".to_owned())
        );
        assert!(normalize_repository_cursor(Some(String::new())).is_err());
        assert!(normalize_repository_cursor(Some("bad cursor".to_owned())).is_err());
        assert!(normalize_repository_cursor(Some("x".repeat(513))).is_err());
    }

    #[test]
    fn repository_browser_uses_only_fixed_graphql_arguments() {
        let plain = github_repository_arguments(None, None);
        assert_eq!(
            plain,
            vec![
                "api",
                "--hostname",
                "github.com",
                "--method",
                "POST",
                "-H",
                "Accept: application/vnd.github+json",
                "-H",
                "X-GitHub-Api-Version: 2022-11-28",
                "graphql",
                "-f",
                &format!("query={GH_REPOSITORY_QUERY}"),
                "-F",
                "first=50",
            ]
            .into_iter()
            .map(OsString::from)
            .collect::<Vec<_>>()
        );

        let searched = github_repository_arguments(Some("animation"), Some("cursor=="));
        assert!(searched.contains(&OsString::from(format!(
            "query={GH_REPOSITORY_SEARCH_QUERY}"
        ))));
        assert!(searched.contains(&OsString::from("after=cursor==")));
        assert!(searched.contains(&OsString::from("search=animation in:name,description")));
        for dynamic in ["after=cursor==", "search=animation in:name,description"] {
            let index = searched
                .iter()
                .position(|argument| argument == dynamic)
                .expect("dynamic argument");
            assert_eq!(searched[index - 1], "-f");
        }

        let manifest = github_repository_manifest_arguments(&[
            "MDQ6QmxvYjE=".to_owned(),
            "MDQ6QmxvYjI=".to_owned(),
        ])
        .unwrap();
        assert!(manifest.contains(&OsString::from(format!(
            "query={GH_REPOSITORY_MANIFEST_QUERY}"
        ))));
        assert!(manifest.contains(&OsString::from("ids[]=MDQ6QmxvYjE=")));
        assert!(manifest.contains(&OsString::from("ids[]=MDQ6QmxvYjI=")));
        assert!(github_repository_manifest_arguments(&[]).is_err());
    }

    #[test]
    fn repository_pages_are_normalized_without_trusting_urls() {
        let body = br#"{
          "data": {
            "viewer": {
              "repositories": {
                "totalCount": 7,
                "pageInfo": {"hasNextPage": true, "endCursor": "next=="},
                "edges": [{
                  "cursor": "edge-one==",
                  "node": {
                    "name": "Private.Extension",
                    "owner": {"login": "example-owner"},
                    "description": "  One\n  useful   extension  ",
                    "isPrivate": true,
                    "isArchived": false,
                    "isFork": false,
                    "updatedAt": "2026-08-30T12:00:00Z",
                    "viewerPermission": "ADMIN",
                    "manifest": null,
                    "latestRelease": null
                  }
                }]
              }
            }
          }
        }"#;
        let mut batch = parse_repository_batch(body, false).unwrap();
        assert!(batch.page_info.has_next_page);
        assert_eq!(batch.page_info.end_cursor.as_deref(), Some("next=="));
        assert_eq!(batch.edges.len(), 1);
        let repository = normalize_repository(batch.edges.remove(0).node.unwrap()).unwrap();
        assert_eq!(
            repository.name_with_owner,
            "example-owner/Private.Extension"
        );
        assert_eq!(
            repository.url,
            "https://github.com/example-owner/Private.Extension"
        );
        assert_eq!(
            repository.description.as_deref(),
            Some("One useful extension")
        );
        assert!(repository.is_private);
        assert_eq!(repository.viewer_permission.as_deref(), Some("ADMIN"));
    }

    #[test]
    fn repository_search_pages_and_graphql_errors_are_classified() {
        let search = br#"{
          "data": {
            "search": {
              "repositoryCount": 1,
              "pageInfo": {"hasNextPage": false, "endCursor": null},
                "edges": [{
                  "cursor": "search-edge==",
                  "node": {
                    "name": "public-extension",
                    "owner": {"login": "someone"},
                    "description": null,
                    "isPrivate": false,
                    "isArchived": false,
                    "isFork": true,
                    "updatedAt": null,
                    "viewerPermission": "READ",
                    "manifest": null,
                    "latestRelease": null
                  }
                }]
            }
          }
        }"#;
        let page = parse_repository_batch(search, true).unwrap();
        assert!(!page.page_info.has_next_page);
        assert!(page.page_info.end_cursor.is_none());
        assert_eq!(
            page.edges[0].node.as_ref().unwrap().name,
            "public-extension"
        );
        assert_eq!(
            normalize_repository(page.edges.into_iter().next().unwrap().node.unwrap())
                .unwrap()
                .name_with_owner,
            "someone/public-extension"
        );

        let limited = br#"{
          "data": null,
          "errors": [{"message":"API rate limit exceeded","extensions":{"type":"RATE_LIMITED"}}]
        }"#;
        let error = parse_repository_batch(limited, false).unwrap_err();
        assert_eq!(error.code, "GITHUB_RATE_LIMITED");
        assert!(error.retryable);

        let malformed = parse_repository_batch(br#"{"data":{}}"#, false).unwrap_err();
        assert_eq!(malformed.code, "INVALID_GITHUB_RESPONSE");
    }

    #[test]
    fn repository_browser_recognizes_only_aseprite_manifests_and_release_assets() {
        for contribution in [
            r#"{"scripts":[{"path":"./main.lua"}]}"#,
            r#"{"keys":[{"id":"keys","path":"./keys.json"}]}"#,
            r#"{"languages":[{"id":"en","path":"./en.ini"}]}"#,
            r#"{"themes":[{"id":"dark","path":"./theme"}]}"#,
            r#"{"palettes":[{"id":"colors","path":"./colors.gpl"}]}"#,
            r#"{"ditheringMatrices":[{"id":"matrix","path":"./matrix.png"}]}"#,
            r#"{"scripts":"./legacy.lua"}"#,
        ] {
            let manifest = format!(
                r#"{{"name":"real-extension","version":"1.2.3","contributes":{contribution}}}"#
            );
            let (node, manifests) = repository_with_manifest(&manifest);
            assert!(is_aseprite_repository(&node, &manifests));
        }

        for manifest in [
            r#"{"name":"npm-package","version":"1.0.0","main":"index.js","scripts":{"test":"node test.js"},"dependencies":{"left-pad":"1"}}"#,
            r#"{"name":"com.example.unity","version":"1.0.0","unity":"2022.3","dependencies":{"com.unity.modules.ui":"1.0.0"}}"#,
            r#"{"name":"vscode-theme","version":"1.0.0","engines":{"vscode":"^1.80.0"},"contributes":{"themes":[{"label":"Dark","uiTheme":"vs-dark","path":"./theme.json"}]}}"#,
            r#"{"name":"empty-extension","version":"1.0.0","contributes":{"scripts":[]}}"#,
            r#"{"name":"wrong-shape","version":"1.0.0","contributes":{"languages":[{"id":"lua"}]}}"#,
            r#"{"name":"broken","version":"1.0.0","contributes": "#,
        ] {
            let (node, manifests) = repository_with_manifest(manifest);
            assert!(!is_aseprite_repository(&node, &manifests));
        }

        let stable = repository_with_release("PACKAGE.ASEPRITE-EXTENSION", false, false);
        assert!(is_aseprite_repository(&stable, &HashMap::new()));
        let wrong_suffix = repository_with_release("package.aseprite-extension.zip", false, false);
        assert!(!is_aseprite_repository(&wrong_suffix, &HashMap::new()));
        let draft = repository_with_release("package.aseprite-extension", true, false);
        assert!(!is_aseprite_repository(&draft, &HashMap::new()));
        let prerelease = repository_with_release("package.aseprite-extension", false, true);
        assert!(!is_aseprite_repository(&prerelease, &HashMap::new()));
    }

    #[test]
    fn repository_manifest_probes_are_bounded_and_immutable() {
        let (mut node, manifests) = repository_with_manifest(
            r#"{"name":"sample","version":"1.0.0","contributes":{"scripts":"./main.lua"}}"#,
        );
        assert!(has_aseprite_manifest(&node, &manifests));

        node.manifest.as_mut().unwrap().is_binary = true;
        assert!(!has_aseprite_manifest(&node, &manifests));
        node.manifest.as_mut().unwrap().is_binary = false;
        node.manifest.as_mut().unwrap().is_truncated = true;
        assert!(!has_aseprite_manifest(&node, &manifests));
        node.manifest.as_mut().unwrap().is_truncated = false;
        node.manifest.as_mut().unwrap().byte_size = GH_REPOSITORY_MANIFEST_BYTES + 1;
        assert!(!has_aseprite_manifest(&node, &manifests));
        node.manifest.as_mut().unwrap().byte_size = 1;
        node.manifest.as_mut().unwrap().oid = Some("not-a-blob-oid".to_owned());
        assert!(!has_aseprite_manifest(&node, &manifests));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn repository_browser_scans_past_unrelated_raw_pages() {
        let (directory, executable) = fake_gh(
            r#"#!/bin/sh
for argument in "$@"; do
  if [ "$argument" = "after=raw-next==" ]; then
    printf '%s' '{"data":{"viewer":{"repositories":{"totalCount":2,"pageInfo":{"hasNextPage":false,"endCursor":null},"edges":[{"cursor":"aseprite-cursor==","node":{"name":"real-extension","owner":{"login":"example"},"description":null,"isPrivate":false,"isArchived":false,"isFork":false,"updatedAt":null,"viewerPermission":"READ","manifest":null,"latestRelease":{"isDraft":false,"isPrerelease":false,"releaseAssets":{"nodes":[{"name":"real.aseprite-extension"}]}}}}]}}}}'
    exit 0
  fi
done
printf '%s' '{"data":{"viewer":{"repositories":{"totalCount":2,"pageInfo":{"hasNextPage":true,"endCursor":"raw-next=="},"edges":[{"cursor":"unity-cursor==","node":{"name":"unity-package","owner":{"login":"example"},"description":null,"isPrivate":false,"isArchived":false,"isFork":false,"updatedAt":null,"viewerPermission":"READ","manifest":null,"latestRelease":null}}]}}}}'
"#,
        );
        let page = list_repositories_with_executable(&executable, None, None)
            .await
            .unwrap();
        drop(directory);
        assert_eq!(page.repositories.len(), 1);
        assert_eq!(
            page.repositories[0].name_with_owner,
            "example/real-extension"
        );
        assert_eq!(page.total_count, 1);
        assert!(!page.has_next_page);
        assert!(page.end_cursor.is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn repository_browser_cursor_follows_the_sixth_filtered_match() {
        let edges = (1..=7)
            .map(|index| {
                serde_json::json!({
                    "cursor": format!("cursor-{index}=="),
                    "node": {
                        "name": format!("extension-{index}"),
                        "owner": {"login": "example"},
                        "description": null,
                        "isPrivate": false,
                        "isArchived": false,
                        "isFork": false,
                        "updatedAt": null,
                        "viewerPermission": "READ",
                        "manifest": null,
                        "latestRelease": {
                            "isDraft": false,
                            "isPrerelease": false,
                            "releaseAssets": {
                                "nodes": [{"name": format!("extension-{index}.aseprite-extension")}]
                            }
                        }
                    }
                })
            })
            .collect::<Vec<_>>();
        let response = serde_json::json!({
            "data": {
                "viewer": {
                    "repositories": {
                        "totalCount": 7,
                        "pageInfo": {"hasNextPage": false, "endCursor": null},
                        "edges": edges
                    }
                }
            }
        })
        .to_string();
        let script = format!("#!/bin/sh\nprintf '%s' '{response}'\n");
        let (directory, executable) = fake_gh(&script);
        let page = list_repositories_with_executable(&executable, None, None)
            .await
            .unwrap();
        drop(directory);
        assert_eq!(page.repositories.len(), GH_REPOSITORY_PAGE_SIZE);
        assert_eq!(page.total_count, 7);
        assert!(page.has_next_page);
        assert_eq!(page.end_cursor.as_deref(), Some("cursor-6=="));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn repository_browser_maps_cli_authentication_failure() {
        let (_directory, executable) = fake_gh("#!/bin/sh\nexit 4\n");
        let arguments = github_repository_arguments(None, None);
        let error = run_gh_graphql_with_executable(&executable, &arguments)
            .await
            .expect_err("exit 4 requires GitHub CLI authentication");
        assert_eq!(error.code, "GITHUB_CLI_AUTH_REQUIRED");
    }

    #[tokio::test]
    async fn gh_archive_output_is_rejected_at_the_download_limit() {
        let (mut writer, mut reader) = tokio::io::duplex(16);
        writer.write_all(b"12345").await.unwrap();
        writer.shutdown().await.unwrap();

        let error = read_bounded_gh_output(&mut reader, 4, GhOutputKind::Archive)
            .await
            .expect_err("oversize GitHub CLI output must be rejected");
        assert_eq!(error.code, "ARCHIVE_TOO_LARGE");
    }

    #[tokio::test]
    async fn gh_json_output_is_bounded_separately_from_archives() {
        let (mut writer, mut reader) = tokio::io::duplex(16);
        writer.write_all(b"12345").await.unwrap();
        writer.shutdown().await.unwrap();

        let error = read_bounded_gh_output(&mut reader, 4, GhOutputKind::Json)
            .await
            .expect_err("oversize GitHub CLI JSON must be rejected");
        assert_eq!(error.code, "INVALID_GITHUB_RESPONSE");
    }

    #[tokio::test]
    async fn etag_not_modified_uses_cached_body() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let saw_validator = Arc::new(AtomicBool::new(false));
        let validator = saw_validator.clone();
        let server = tokio::spawn(async move {
            for request_number in 0..2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut bytes = vec![0_u8; 4096];
                let count = stream.read(&mut bytes).await.unwrap();
                let request = String::from_utf8_lossy(&bytes[..count]);
                if request_number == 0 {
                    let body = br#"{"value":42}"#;
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nETag: \"fixture\"\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        String::from_utf8_lossy(body)
                    );
                    stream.write_all(response.as_bytes()).await.unwrap();
                } else {
                    validator.store(
                        request
                            .to_ascii_lowercase()
                            .contains("if-none-match: \"fixture\""),
                        Ordering::SeqCst,
                    );
                    stream
                        .write_all(
                            b"HTTP/1.1 304 Not Modified\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        )
                        .await
                        .unwrap();
                }
            }
        });
        let temporary = tempfile::tempdir().unwrap();
        let state = State::new(temporary.path()).unwrap();
        let client = Client::builder().user_agent("fixture").build().unwrap();
        let github = GitHubClient { client, state };
        let endpoint = format!("http://{address}/fixture");
        let first: Value = github.get_json(&endpoint).await.unwrap();
        let second: Value = github.get_json(&endpoint).await.unwrap();
        server.await.unwrap();
        assert_eq!(first, second);
        assert_eq!(second["value"], 42);
        assert!(saw_validator.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn interrupted_download_leaves_no_partial_staged_artifact() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request).await.unwrap();
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\nshort",
                )
                .await
                .unwrap();
        });
        let temporary = tempfile::tempdir().unwrap();
        let state = State::new(temporary.path()).unwrap();
        let client = Client::builder().build().unwrap();
        let response = client
            .get(format!("http://{address}/artifact"))
            .send()
            .await
            .unwrap();
        let github = GitHubClient {
            client,
            state: state.clone(),
        };
        assert!(github.stream_download(response).await.is_err());
        server.await.unwrap();
        assert_eq!(
            fs::read_dir(state.root().join("staging")).unwrap().count(),
            0
        );
    }
}
