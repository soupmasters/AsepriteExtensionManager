use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use reqwest::header::{HeaderMap, ETAG, IF_NONE_MATCH};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;

use crate::package::{
    package_repository_archive, validate_and_stage, validate_manager_and_stage, ExpectedManifest,
    PreparedPackage, MAX_ARCHIVE_BYTES,
};
use crate::protocol::{RpcError, RpcResult};
use crate::state::{atomic_write, State};
use crate::VERSION;

const API_ROOT: &str = "https://api.github.com";

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
                let downloaded = self.download(url.clone()).await?;
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
                        asset_id: None,
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
    ) -> RpcResult<PreparedPackage> {
        let url = Url::parse(url)
            .map_err(|error| RpcError::invalid("INVALID_ASSET_URL", error.to_string()))?;
        let downloaded = self.download(url).await?;
        let (actual_hash, actual_length) = crate::package::artifact_hash(&downloaded)?;
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
        let package = validate_and_stage(
            &self.state,
            &downloaded,
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
        if requested_ref.is_none() {
            let endpoint = format!("{API_ROOT}/repos/{owner}/{repository}/releases/latest");
            match self.get_json::<Release>(&endpoint).await {
                Ok(release) if !release.draft && !release.prerelease => {
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
                        let downloaded = self.download(url.clone()).await?;
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
                Err(error) if error.code == "NOT_FOUND" => {}
                Err(error) => return Err(error),
                _ => {}
            }
        }

        let reference = if let Some(reference) = requested_ref {
            reference.to_owned()
        } else {
            let endpoint = format!("{API_ROOT}/repos/{owner}/{repository}");
            self.get_json::<Repository>(&endpoint).await?.default_branch
        };
        let encoded_ref = utf8_percent_encode(&reference, NON_ALPHANUMERIC).to_string();
        let endpoint = format!("{API_ROOT}/repos/{owner}/{repository}/commits/{encoded_ref}");
        let commit = self.get_json::<Commit>(&endpoint).await?.sha;
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
        let downloaded = self.download(url.clone()).await?;
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
            "only public https://github.com URLs are supported",
        ));
    }
    if url.username() != "" || url.password().is_some() || url.port().is_some() {
        return Err(RpcError::invalid(
            "INVALID_GITHUB_URL",
            "credentials and custom ports are unsupported",
        ));
    }
    let segments: Vec<_> = url
        .path_segments()
        .map(|segments| segments.filter(|segment| !segment.is_empty()).collect())
        .unwrap_or_default();
    if segments.len() < 2 {
        return Err(RpcError::invalid(
            "INVALID_GITHUB_URL",
            "URL must identify a GitHub repository",
        ));
    }
    let owner = validate_identifier(segments[0], "owner")?;
    let repository = validate_identifier(
        segments[1].strip_suffix(".git").unwrap_or(segments[1]),
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
    fn refuses_non_public_and_ambiguous_urls() {
        assert!(parse_github_url("http://github.com/example/sample").is_err());
        assert!(parse_github_url("https://git.example.com/example/sample").is_err());
        assert!(parse_github_url("https://github.com/example/sample/issues/1").is_err());
        assert!(parse_github_url("https://token@github.com/example/sample").is_err());
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
