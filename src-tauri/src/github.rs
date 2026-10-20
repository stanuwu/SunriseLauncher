use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use reqwest::header::{ACCEPT, USER_AGENT};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tauri::ipc::Channel;
use tokio::io::AsyncWriteExt;

use crate::error::{AppError, AppResult};
use crate::models::{OperationEvent, ReleaseAsset, ReleaseInfo, ReleaseSummary};

/// Default ceiling for launcher-managed downloads (release DLLs, tools).
pub const DEFAULT_DOWNLOAD_BYTES: u64 = 512 * 1024 * 1024;
/// Ceiling for large compatibility tools; GE-Proton is currently ~650 MB.
pub const LARGE_DOWNLOAD_BYTES: u64 = 2 * 1024 * 1024 * 1024;
// Mission scripts are text; a larger archive is not the scripts repository.
const MAX_SOURCE_BYTES: u64 = 64 * 1024 * 1024;
// GitHub returns at most 100 releases per page.
const MAX_RELEASE_PAGE: u32 = 100;

#[derive(Clone)]
pub struct GitHubClient {
    client: reqwest::Client,
}

#[derive(Debug, Deserialize)]
struct ApiRelease {
    tag_name: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    published_at: Option<DateTime<Utc>>,
    assets: Vec<ApiAsset>,
}

impl ApiRelease {
    fn into_summary(self) -> ReleaseSummary {
        ReleaseSummary {
            name: self.name.unwrap_or_else(|| self.tag_name.clone()),
            tag: self.tag_name,
            prerelease: self.prerelease,
            published_at: self.published_at,
            assets: self
                .assets
                .into_iter()
                .map(|asset| ReleaseAsset {
                    name: asset.name,
                    download_url: asset.browser_download_url,
                    size: asset.size,
                    digest: asset.digest,
                })
                .collect(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct ApiCommit {
    sha: String,
}

#[derive(Debug, Deserialize)]
struct ApiAsset {
    name: String,
    browser_download_url: String,
    size: u64,
    digest: Option<String>,
}

impl GitHubClient {
    pub fn new() -> AppResult<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(600))
            .build()?;
        Ok(Self { client })
    }

    pub async fn latest_release(
        &self,
        owner: &str,
        repository: &str,
        asset_name: &str,
    ) -> AppResult<ReleaseInfo> {
        let url = format!("https://api.github.com/repos/{owner}/{repository}/releases/latest");
        let response = self
            .client
            .get(url)
            .header(USER_AGENT, "Project-Sunrise-Launcher/0.1")
            .header(ACCEPT, "application/vnd.github+json")
            .timeout(Duration::from_secs(20))
            .send()
            .await?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(AppError::message(format!(
                "No published release was found for {owner}/{repository}."
            )));
        }
        if !response.status().is_success() {
            return Err(AppError::message(format!(
                "GitHub returned HTTP {} while checking {owner}/{repository}.",
                response.status().as_u16()
            )));
        }

        let release: ApiRelease = response.json().await?;
        let asset = release
            .assets
            .into_iter()
            .find(|asset| asset.name.eq_ignore_ascii_case(asset_name))
            .ok_or_else(|| {
                AppError::message(format!(
                    "Release {} does not include {asset_name}.",
                    release.tag_name
                ))
            })?;

        if asset.size == 0 || asset.size > DEFAULT_DOWNLOAD_BYTES {
            return Err(AppError::message(format!(
                "{} has an unexpected download size.",
                asset.name
            )));
        }

        Ok(ReleaseInfo {
            tag: release.tag_name,
            asset_name: asset.name,
            download_url: asset.browser_download_url,
            size: asset.size,
            digest: asset.digest,
        })
    }

    pub async fn releases(
        &self,
        owner: &str,
        repository: &str,
        limit: u32,
    ) -> AppResult<Vec<ReleaseSummary>> {
        let per_page = limit.clamp(1, MAX_RELEASE_PAGE);
        let url = format!(
            "https://api.github.com/repos/{owner}/{repository}/releases?per_page={per_page}"
        );
        let response = self
            .client
            .get(url)
            .header(USER_AGENT, "Project-Sunrise-Launcher/0.1")
            .header(ACCEPT, "application/vnd.github+json")
            .timeout(Duration::from_secs(20))
            .send()
            .await?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(AppError::message(format!(
                "No published releases were found for {owner}/{repository}."
            )));
        }
        if !response.status().is_success() {
            return Err(AppError::message(format!(
                "GitHub returned HTTP {} while checking {owner}/{repository}.",
                response.status().as_u16()
            )));
        }

        let releases: Vec<ApiRelease> = response.json().await?;
        Ok(releases
            .into_iter()
            .filter(|release| !release.draft)
            .map(ApiRelease::into_summary)
            .collect())
    }

    pub async fn latest_commit(
        &self,
        owner: &str,
        repository: &str,
        branch: &str,
    ) -> AppResult<String> {
        let url = format!("https://api.github.com/repos/{owner}/{repository}/commits/{branch}");
        let response = self
            .client
            .get(url)
            .header(USER_AGENT, "Project-Sunrise-Launcher/0.1")
            .header(ACCEPT, "application/vnd.github+json")
            .timeout(Duration::from_secs(20))
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(AppError::message(format!(
                "GitHub returned HTTP {} while checking {owner}/{repository}.",
                response.status().as_u16()
            )));
        }
        let commit: ApiCommit = response.json().await?;
        if commit.sha.len() != 40 || !commit.sha.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(AppError::message(format!(
                "GitHub returned an invalid commit for {owner}/{repository}."
            )));
        }
        Ok(commit.sha)
    }

    /** Downloads a source archive, which GitHub publishes without a size or digest. */
    pub async fn download_source(
        &self,
        owner: &str,
        repository: &str,
        commit: &str,
        destination: &Path,
    ) -> AppResult<()> {
        let url = format!("https://codeload.github.com/{owner}/{repository}/zip/{commit}");
        let mut response = self
            .client
            .get(url)
            .header(USER_AGENT, "Project-Sunrise-Launcher/0.1")
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(AppError::message(format!(
                "GitHub returned HTTP {} while downloading {owner}/{repository}.",
                response.status().as_u16()
            )));
        }
        let mut file = tokio::fs::File::create(destination)
            .await
            .map_err(|error| AppError::io("Could not create the download file", error))?;
        let mut downloaded = 0_u64;
        while let Some(chunk) = response.chunk().await? {
            downloaded = downloaded.saturating_add(chunk.len() as u64);
            if downloaded > MAX_SOURCE_BYTES {
                return Err(AppError::message(
                    "The download is larger than the safety limit.",
                ));
            }
            file.write_all(&chunk)
                .await
                .map_err(|error| AppError::io("Could not write the download", error))?;
        }
        file.flush()
            .await
            .map_err(|error| AppError::io("Could not finish the download", error))
    }

    pub async fn download(
        &self,
        release: &ReleaseInfo,
        destination: &Path,
        on_event: &Channel<OperationEvent>,
        stage: &str,
        start_percent: u8,
        end_percent: u8,
    ) -> AppResult<String> {
        self.download_limited(
            release,
            destination,
            on_event,
            stage,
            start_percent,
            end_percent,
            DEFAULT_DOWNLOAD_BYTES,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn download_large(
        &self,
        release: &ReleaseInfo,
        destination: &Path,
        on_event: &Channel<OperationEvent>,
        stage: &str,
        start_percent: u8,
        end_percent: u8,
    ) -> AppResult<String> {
        self.download_limited(
            release,
            destination,
            on_event,
            stage,
            start_percent,
            end_percent,
            LARGE_DOWNLOAD_BYTES,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn download_limited(
        &self,
        release: &ReleaseInfo,
        destination: &Path,
        on_event: &Channel<OperationEvent>,
        stage: &str,
        start_percent: u8,
        end_percent: u8,
        max_bytes: u64,
    ) -> AppResult<String> {
        let response = self
            .client
            .get(&release.download_url)
            .header(USER_AGENT, "Project-Sunrise-Launcher/0.1")
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(AppError::message(format!(
                "GitHub returned HTTP {} while downloading {}.",
                response.status().as_u16(),
                release.asset_name
            )));
        }

        if response
            .content_length()
            .is_some_and(|size| size > max_bytes)
        {
            return Err(AppError::message(
                "The download is larger than the safety limit.",
            ));
        }

        let mut file = tokio::fs::File::create(destination)
            .await
            .map_err(|error| AppError::io("Could not create the download file", error))?;
        let mut stream = response.bytes_stream();
        let mut downloaded = 0_u64;
        let mut hasher = Sha256::new();
        let mut last_ratio: Option<u8> = None;

        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            downloaded = downloaded.saturating_add(chunk.len() as u64);
            if downloaded > max_bytes || downloaded > release.size.saturating_add(1024) {
                return Err(AppError::message(
                    "The download exceeded its published size.",
                ));
            }
            file.write_all(&chunk)
                .await
                .map_err(|error| AppError::io("Could not write the download", error))?;
            hasher.update(&chunk);

            let ratio = downloaded
                .saturating_mul(100)
                .checked_div(release.size)
                .unwrap_or(0)
                .min(100) as u8;

            if last_ratio == Some(ratio) {
                continue;
            }

            last_ratio = Some(ratio);
            let range = (end_percent.saturating_sub(start_percent)) as u32;
            let percent = start_percent.saturating_add(((range * ratio as u32) / 100) as u8);

            let _ = on_event.send(OperationEvent::Progress {
                stage: stage.into(),
                message: format!("Downloading {}… {}%", release.asset_name, ratio),
                percent: f32::from(percent),
            });
        }
        file.flush()
            .await
            .map_err(|error| AppError::io("Could not finish the download", error))?;

        if downloaded != release.size {
            return Err(AppError::message(format!(
                "The download was incomplete (expected {}, received {} bytes).",
                release.size, downloaded
            )));
        }

        let actual = hex::encode(hasher.finalize());
        verify_digest(release.digest.as_deref(), &actual)?;
        Ok(actual)
    }
}

fn verify_digest(digest: Option<&str>, actual: &str) -> AppResult<()> {
    let Some(digest) = digest else {
        return Ok(());
    };
    let expected = digest
        .strip_prefix("sha256:")
        .or_else(|| digest.strip_prefix("SHA256:"))
        .ok_or_else(|| AppError::message("GitHub supplied an unsupported asset digest."))?;
    if !expected.eq_ignore_ascii_case(actual) {
        return Err(AppError::message(
            "The downloaded file failed its SHA-256 check.",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{ApiRelease, verify_digest};

    #[test]
    fn accepts_matching_github_digest() {
        assert!(verify_digest(Some("sha256:aabb"), "AABB").is_ok());
    }

    #[test]
    fn rejects_mismatched_github_digest() {
        assert!(verify_digest(Some("sha256:aabb"), "ccdd").is_err());
    }

    #[test]
    fn release_summary_parses_name_fallback_and_missing_fields() {
        let release: ApiRelease = serde_json::from_str(
            r#"{
                "tag_name": "GE-Proton11-7",
                "name": null,
                "prerelease": false,
                "draft": false,
                "published_at": "2026-07-24T18:26:00Z",
                "assets": [
                    {
                        "name": "GE-Proton11-7-x86_64.tar.gz",
                        "browser_download_url": "https://example.invalid/ge.tar.gz",
                        "size": 563784602,
                        "digest": "sha256:c5448b76"
                    }
                ]
            }"#,
        )
        .expect("release should parse");

        let summary = release.into_summary();
        assert_eq!(summary.tag, "GE-Proton11-7");
        assert_eq!(summary.name, "GE-Proton11-7");
        assert!(!summary.prerelease);
        assert!(summary.published_at.is_some());
        assert_eq!(summary.assets.len(), 1);
        assert_eq!(summary.assets[0].size, 563_784_602);
        assert_eq!(summary.assets[0].digest.as_deref(), Some("sha256:c5448b76"));

        let minimal: ApiRelease = serde_json::from_str(r#"{ "tag_name": "1.4.4", "assets": [] }"#)
            .expect("minimal release should parse");
        let minimal_summary = minimal.into_summary();
        assert_eq!(minimal_summary.name, "1.4.4");
        assert!(!minimal_summary.prerelease);
        assert!(minimal_summary.published_at.is_none());
        assert!(minimal_summary.assets.is_empty());
    }
}
