use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Child;
use std::time::{Duration, SystemTime};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::error::{AppError, AppResult};
use crate::github::GitHubClient;
use crate::installer;
use crate::models::{
    OperationEvent, Preferences, ProtonRelease, ProtonSource, ProtonStatus, ReleaseAsset,
    ReleaseInfo, ReleaseSummary, UmuSource,
};
use crate::storage;

pub const PROTON_REPOSITORY_OWNER: &str = "GloriousEggroll";
pub const PROTON_REPOSITORY: &str = "proton-ge-custom";
pub const UMU_REPOSITORY_OWNER: &str = "Open-Wine-Components";
pub const UMU_REPOSITORY: &str = "umu-launcher";

const CATALOG_LIMIT: u32 = 30;
const CATALOG_TTL: Duration = Duration::from_secs(6 * 60 * 60);
const CATALOG_SCHEMA: u8 = 2;

const LOG_FILE: &str = "umu.log";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Catalog {
    schema_version: u8,
    fetched_at: DateTime<Utc>,
    releases: Vec<CatalogEntry>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct CatalogEntry {
    tag: String,
    asset_name: String,
    download_url: String,
    size: u64,
    digest: Option<String>,
    published_at: Option<DateTime<Utc>>,
    prerelease: bool,
}

pub struct ProcessTracker {
    pub processes: Mutex<Vec<Child>>,
}

impl ProcessTracker {
    pub fn has_running(&self) -> bool {
        let Ok(mut guard) = self.processes.try_lock() else {
            return true;
        };
        guard.retain_mut(|child| matches!(child.try_wait(), Ok(None)));
        !guard.is_empty()
    }
}

fn asset_arch() -> AppResult<&'static str> {
    match std::env::consts::ARCH {
        "x86_64" => Ok("x86_64"),
        "aarch64" => Ok("aarch64"),
        other => Err(AppError::message(format!(
            "GE-Proton does not publish a build for the {other} architecture."
        ))),
    }
}

fn select_asset<'a>(release: &'a ReleaseSummary, arch: &str) -> Option<&'a ReleaseAsset> {
    let suffixed = format!("-{arch}.tar.gz");
    if let Some(asset) = release
        .assets
        .iter()
        .find(|asset| asset.name.ends_with(&suffixed))
    {
        return Some(asset);
    }
    if arch == "x86_64" {
        let legacy = format!("{}.tar.gz", release.tag);
        return release.assets.iter().find(|asset| asset.name == legacy);
    }
    None
}

fn catalog_path(app: &AppHandle) -> AppResult<PathBuf> {
    Ok(storage::app_data_dir(app)?
        .join("tools")
        .join("proton-catalog.json"))
}

pub fn proton_versions_dir(app: &AppHandle) -> AppResult<PathBuf> {
    Ok(storage::app_data_dir(app)?
        .join("tools")
        .join("GE-Proton")
        .join("versions"))
}

pub fn umu_versions_dir(app: &AppHandle) -> AppResult<PathBuf> {
    Ok(storage::app_data_dir(app)?
        .join("tools")
        .join("umu")
        .join("versions"))
}

pub fn prefix_path(app: &AppHandle, preferences: &Preferences) -> AppResult<PathBuf> {
    let configured = preferences.proton_prefix.trim();
    if !configured.is_empty() {
        return Ok(PathBuf::from(configured));
    }
    Ok(storage::app_data_dir(app)?.join("proton").join("prefix"))
}

pub fn managed_proton_path(app: &AppHandle, tag: &str) -> AppResult<PathBuf> {
    Ok(proton_versions_dir(app)?.join(sanitize_name(tag)))
}

fn sanitize_name(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

// Accurately compares version-like tags for natural sorting. (proton)
fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let mut a_chars = a.chars().peekable();
    let mut b_chars = b.chars().peekable();
    loop {
        return match (a_chars.peek(), b_chars.peek()) {
            (None, None) => std::cmp::Ordering::Equal,
            (None, Some(_)) => std::cmp::Ordering::Less,
            (Some(_), None) => std::cmp::Ordering::Greater,
            (Some(ac), Some(bc)) if ac.is_ascii_digit() && bc.is_ascii_digit() => {
                match take_number(&mut a_chars).cmp(&take_number(&mut b_chars)) {
                    std::cmp::Ordering::Equal => continue,
                    other => other,
                }
            }
            _ => match a_chars.next().cmp(&b_chars.next()) {
                std::cmp::Ordering::Equal => continue,
                other => other,
            },
        };
    }
}

fn take_number(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> u128 {
    let mut value: u128 = 0;
    while let Some(character) = chars.peek() {
        let Some(digit) = character.to_digit(10) else {
            break;
        };
        value = value.saturating_mul(10).saturating_add(u128::from(digit));
        chars.next();
    }
    value
}

fn sort_tags_newest_first(tags: &mut [String]) {
    tags.sort_by(|a, b| natural_cmp(a, b));
    tags.reverse();
}

pub fn installed_tags(app: &AppHandle) -> AppResult<Vec<String>> {
    let root = proton_versions_dir(app)?;
    let Ok(entries) = std::fs::read_dir(&root) else {
        return Ok(Vec::new());
    };
    let mut tags = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect::<Vec<_>>();
    sort_tags_newest_first(&mut tags);
    Ok(tags)
}

pub async fn load_cached_catalog(app: &AppHandle) -> AppResult<Option<Vec<ProtonRelease>>> {
    let path = catalog_path(app)?;
    let Ok(bytes) = tokio::fs::read(&path).await else {
        return Ok(None);
    };
    let Ok(catalog) = serde_json::from_slice::<Catalog>(&bytes) else {
        return Ok(None);
    };
    if catalog.schema_version != CATALOG_SCHEMA {
        return Ok(None);
    }
    let age = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .saturating_sub(catalog.fetched_at.timestamp().max(0) as u64);
    if age > CATALOG_TTL.as_secs() {
        return Ok(None);
    }
    Ok(Some(entries_to_releases(app, catalog.releases)?))
}

pub async fn refresh_catalog(
    app: &AppHandle,
    github: &GitHubClient,
) -> AppResult<Vec<ProtonRelease>> {
    let arch = asset_arch()?;
    let summaries = github
        .releases(PROTON_REPOSITORY_OWNER, PROTON_REPOSITORY, CATALOG_LIMIT)
        .await?;

    let entries = summaries
        .iter()
        .filter_map(|release| {
            let asset = select_asset(release, arch)?;
            Some(CatalogEntry {
                tag: release.tag.clone(),
                asset_name: asset.name.clone(),
                download_url: asset.download_url.clone(),
                size: asset.size,
                digest: asset.digest.clone(),
                published_at: release.published_at,
                prerelease: release.prerelease,
            })
        })
        .collect::<Vec<_>>();

    if entries.is_empty() {
        return Err(AppError::message(format!(
            "No GE-Proton builds were found for the {arch} architecture."
        )));
    }

    let catalog = Catalog {
        schema_version: CATALOG_SCHEMA,
        fetched_at: Utc::now(),
        releases: entries,
    };
    let path = catalog_path(app)?;
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|error| AppError::io("Could not create the tools folder", error))?;
    }
    let bytes = serde_json::to_vec_pretty(&catalog)?;
    tokio::fs::write(&path, bytes)
        .await
        .map_err(|error| AppError::io("Could not cache the Proton catalog", error))?;

    entries_to_releases(app, catalog.releases)
}

fn entries_to_releases(
    app: &AppHandle,
    entries: Vec<CatalogEntry>,
) -> AppResult<Vec<ProtonRelease>> {
    let installed = installed_tags(app)?;
    Ok(entries
        .into_iter()
        .map(|entry| ProtonRelease {
            installed: installed.iter().any(|tag| tag == &entry.tag),
            tag: entry.tag,
            asset_name: entry.asset_name,
            download_url: entry.download_url,
            size: entry.size,
            digest: entry.digest,
            published_at: entry.published_at,
            prerelease: entry.prerelease,
        })
        .collect())
}

pub fn find_umu_run(
    app: &AppHandle,
    preferences: &Preferences,
) -> AppResult<Option<(PathBuf, String, String)>> {
    if preferences.umu_source == UmuSource::System {
        return Ok(find_on_path("umu-run").map(|path| (path, "system".into(), "system".into())));
    }

    if let Some((path, tag)) = find_managed_umu(app)? {
        return Ok(Some((path, tag, "managed".into())));
    }

    if let Some(path) = find_on_path("umu-run") {
        return Ok(Some((path, "system".into(), "system".into())));
    }
    Ok(None)
}

fn find_managed_umu(app: &AppHandle) -> AppResult<Option<(PathBuf, String)>> {
    let versions = umu_versions_dir(app)?;
    let Ok(entries) = std::fs::read_dir(&versions) else {
        return Ok(None);
    };
    let mut tags = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect::<Vec<_>>();
    sort_tags_newest_first(&mut tags);
    for tag in tags {
        let candidate = versions.join(&tag).join("umu").join("umu-run");
        if candidate.is_file() {
            return Ok(Some((candidate, tag)));
        }
    }
    Ok(None)
}

fn find_on_path(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|directory| directory.join(program))
        .find(|candidate| candidate.is_file())
}

pub fn resolve_proton_path(
    app: &AppHandle,
    preferences: &Preferences,
) -> AppResult<Option<(PathBuf, String)>> {
    match preferences.proton_source {
        ProtonSource::Custom => {
            let configured = preferences.proton_custom_path.trim();
            if configured.is_empty() {
                return Ok(None);
            }
            let path = PathBuf::from(configured);
            if !path.is_dir() {
                return Err(AppError::message(format!(
                    "The custom Proton folder does not exist: {configured}"
                )));
            }
            Ok(Some((path, "custom".into())))
        }
        ProtonSource::Managed => {
            let installed = installed_tags(app)?;
            let selected = preferences.proton_version.trim();
            if !selected.is_empty() {
                if !installed.iter().any(|tag| tag == selected) {
                    return Err(AppError::message(
                        "The selected GE-Proton build is not installed yet. Use Download to install it.",
                    ));
                }
                return Ok(Some((
                    managed_proton_path(app, selected)?,
                    selected.to_owned(),
                )));
            }
            if installed.is_empty() {
                return Ok(None);
            }
            Ok(Some((
                managed_proton_path(app, &installed[0])?,
                installed[0].clone(),
            )))
        }
    }
}

pub fn status(app: &AppHandle, preferences: &Preferences) -> AppResult<ProtonStatus> {
    let prefix = prefix_path(app, preferences)?;
    let prefix_initialized = prefix.join("drive_c").is_dir();
    let installed = installed_tags(app)?;

    let (umu_available, umu_version, umu_source) = match find_umu_run(app, preferences) {
        Ok(Some((_, version, source))) => (true, Some(version), Some(source)),
        _ => (false, None, None),
    };

    let (proton_path, selected_tag, error) = match resolve_proton_path(app, preferences) {
        Ok(Some((path, tag))) => (Some(path.to_string_lossy().into_owned()), Some(tag), None),
        Ok(None) => (
            None,
            None,
            Some("No Proton build is installed yet. Download one to continue.".into()),
        ),
        Err(error) => (None, None, Some(error.to_string())),
    };

    let ready = umu_available && proton_path.is_some() && prefix_initialized;
    let error = if ready {
        None
    } else if error.is_some() {
        error
    } else if !umu_available {
        Some("umu-launcher is not available yet.".into())
    } else if !prefix_initialized {
        Some("The Proton environment has not been prepared yet.".into())
    } else {
        None
    };

    Ok(ProtonStatus {
        supported: true,
        umu_available,
        umu_version,
        umu_source,
        selected_tag,
        proton_path,
        installed_tags: installed,
        prefix_path: prefix.to_string_lossy().into_owned(),
        prefix_initialized,
        ready,
        error,
    })
}

pub fn looks_like_proton(path: &Path) -> bool {
    path.join("proton").is_file()
}

pub async fn initialize_prefix(
    app: &AppHandle,
    preferences: &Preferences,
    on_event: &Channel<OperationEvent>,
    cancel: &CancellationToken,
) -> AppResult<()> {
    let (umu_run, _, _) = find_umu_run(app, preferences)?.ok_or_else(|| {
        AppError::message("umu-launcher is not installed yet. Download Proton first.")
    })?;
    let (proton_path, _) = resolve_proton_path(app, preferences)?.ok_or_else(|| {
        AppError::message("No Proton build is installed yet. Download one first.")
    })?;
    let prefix = prefix_path(app, preferences)?;
    tokio::fs::create_dir_all(&prefix)
        .await
        .map_err(|error| AppError::io("Could not create the Proton prefix folder", error))?;

    let _ = on_event.send(OperationEvent::Progress {
        stage: "proton".into(),
        message: "Preparing the Proton environment and Steam Linux Runtime…".into(),
        percent: 80.0,
    });

    let mut command = tokio::process::Command::new(&umu_run);
    command
        .arg("cmd.exe")
        .arg("/c")
        .arg("exit")
        .env("WINEPREFIX", &prefix)
        .env("PROTONPATH", &proton_path)
        .env("GAMEID", game_id(preferences))
        .env("STORE", "none")
        .env("PROTON_VERB", "waitforexitandrun")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let output = tokio::select! {
        result = command.output() => result
            .map_err(|error| AppError::io("umu-launcher could not be started", error))?,
        _ = cancel.cancelled() => return Err(AppError::Cancelled),
    };

    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        let detail = detail.trim();
        return Err(AppError::message(if detail.is_empty() {
            "The Proton environment could not be prepared.".to_owned()
        } else {
            format!("The Proton environment could not be prepared: {detail}")
        }));
    }
    Ok(())
}

fn game_id(preferences: &Preferences) -> String {
    let configured = preferences.game_id.trim();
    if configured.is_empty() {
        "sunrise".to_owned()
    } else {
        configured.to_owned()
    }
}

pub fn launch(
    app: &AppHandle,
    preferences: &Preferences,
    install_directory: &str,
) -> AppResult<()> {
    let root = PathBuf::from(install_directory.trim());
    let executable = root.join(crate::models::GAME_EXECUTABLE);
    if !executable.is_file() {
        return Err(AppError::message(
            "destiny2.exe was not found in the installation folder.",
        ));
    }

    let (umu_run, _, _) = find_umu_run(app, preferences)?.ok_or_else(|| {
        AppError::message("umu-launcher is not installed yet. Open Settings and download Proton.")
    })?;
    let (proton_path, _) = resolve_proton_path(app, preferences)?.ok_or_else(|| {
        AppError::message("No Proton build is installed yet. Open Settings and download one.")
    })?;
    let prefix = prefix_path(app, preferences)?;
    if !prefix.join("drive_c").is_dir() {
        return Err(AppError::message(
            "The Proton environment has not been prepared yet. Open Settings and prepare it.",
        ));
    }

    let plan =
        crate::proton_launch::plan(&umu_run, &proton_path, &prefix, &executable, preferences)?;

    let mut command = std::process::Command::new(&plan.program);
    command.args(&plan.args).current_dir(&plan.cwd);
    for (key, value) in &plan.env {
        command.env(key, value);
    }

    let root = PathBuf::from(install_directory.trim());
    let log_path = root.join(LOG_FILE);
    let mut stdout_file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&log_path)
        .map_err(|error| AppError::io("Failed to open log file", error))?;

    let stderr_file = stdout_file
        .try_clone()
        .map_err(|error| AppError::message(format!("Failed to clone stdout file: {}", error)))?;

    let command_str = format!("Sunrise starting with: {:?}\n", command);
    stdout_file.write_all(command_str.as_bytes()).ok();

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }

    let child = command
        .stdin(std::process::Stdio::null())
        .stdout(stdout_file)
        .stderr(stderr_file)
        .spawn()
        .map_err(|error| AppError::io("Destiny 2 could not be launched", error))?;

    let tracker = app.state::<ProcessTracker>();
    let mut guard = tracker
        .processes
        .try_lock()
        .map_err(|_error| AppError::message("Failed to lock processes"))?;
    guard.push(child);
    Ok(())
}

fn cancel_check(cancel: &CancellationToken) -> AppResult<()> {
    if cancel.is_cancelled() {
        return Err(AppError::Cancelled);
    }
    Ok(())
}

pub async fn install_proton(
    app: &AppHandle,
    github: &GitHubClient,
    release: &ProtonRelease,
    on_event: &Channel<OperationEvent>,
    cancel: &CancellationToken,
) -> AppResult<()> {
    let versions = proton_versions_dir(app)?;
    tokio::fs::create_dir_all(&versions)
        .await
        .map_err(|error| AppError::io("Could not create the Proton folder", error))?;
    let version_directory = versions.join(sanitize_name(&release.tag));
    if looks_like_proton(&version_directory) {
        return Ok(());
    }

    cancel_check(cancel)?;
    let temporary = tempfile::Builder::new()
        .prefix(".ge-proton-")
        .tempdir_in(&versions)
        .map_err(|error| AppError::io("Could not create a temporary Proton folder", error))?;
    let archive = temporary.path().join(&release.asset_name);
    let release_info = ReleaseInfo {
        tag: release.tag.clone(),
        asset_name: release.asset_name.clone(),
        download_url: release.download_url.clone(),
        size: release.size,
        digest: release.digest.clone(),
    };
    github
        .download_large(&release_info, &archive, on_event, "proton", 15, 70)
        .await?;

    cancel_check(cancel)?;
    let _ = on_event.send(OperationEvent::Progress {
        stage: "proton".into(),
        message: "Extracting the GE-Proton build…".into(),
        percent: 72.0,
    });
    let extracted = temporary.path().join("extracted");
    let (archive_copy, extracted_copy) = (archive.clone(), extracted.clone());
    tokio::task::spawn_blocking(move || extract_tar_gz(&archive_copy, &extracted_copy))
        .await
        .map_err(|error| {
            AppError::message(format!("Archive extraction stopped unexpectedly: {error}"))
        })??;

    let root = single_root(&extracted)?;
    if version_directory.exists() {
        std::fs::remove_dir_all(&version_directory)
            .map_err(|error| AppError::io("Could not replace the cached Proton build", error))?;
    }
    std::fs::rename(&root, &version_directory)
        .map_err(|error| AppError::io("Could not install GE-Proton", error))?;

    Ok(())
}

pub fn remove_proton(app: &AppHandle, tag: &str) -> AppResult<()> {
    let directory = managed_proton_path(app, tag)?;
    if !directory.exists() {
        return Ok(());
    }
    std::fs::remove_dir_all(&directory)
        .map_err(|error| AppError::io("Could not remove the Proton build", error))
}

pub async fn install_umu(
    app: &AppHandle,
    github: &GitHubClient,
    on_event: &Channel<OperationEvent>,
    cancel: &CancellationToken,
) -> AppResult<String> {
    if let Some((_, tag)) = find_managed_umu(app)? {
        return Ok(tag);
    }

    let releases = github
        .releases(UMU_REPOSITORY_OWNER, UMU_REPOSITORY, 10)
        .await?;
    let (tag, asset) = releases
        .iter()
        .filter(|release| !release.prerelease)
        .find_map(|release| {
            release
                .assets
                .iter()
                .find(|asset| asset.name.ends_with("-zipapp.tar"))
                .map(|asset| (release.tag.clone(), asset.clone()))
        })
        .ok_or_else(|| AppError::message("No umu-launcher zipapp release was found."))?;

    let versions = umu_versions_dir(app)?;
    tokio::fs::create_dir_all(&versions)
        .await
        .map_err(|error| AppError::io("Could not create the umu folder", error))?;
    let version_directory = versions.join(sanitize_name(&tag));
    let umu_run = version_directory.join("umu").join("umu-run");
    if umu_run.is_file() {
        return Ok(tag);
    }

    cancel_check(cancel)?;
    let temporary = tempfile::Builder::new()
        .prefix(".umu-")
        .tempdir_in(&versions)
        .map_err(|error| AppError::io("Could not create a temporary umu folder", error))?;
    let archive = temporary.path().join(&asset.name);
    let release_info = ReleaseInfo {
        tag: tag.clone(),
        asset_name: asset.name.clone(),
        download_url: asset.download_url.clone(),
        size: asset.size,
        digest: asset.digest.clone(),
    };
    github
        .download(&release_info, &archive, on_event, "proton", 5, 15)
        .await?;

    cancel_check(cancel)?;
    let extracted = temporary.path().join("extracted");
    let (archive_copy, extracted_copy) = (archive.clone(), extracted.clone());
    tokio::task::spawn_blocking(move || extract_tar_file(&archive_copy, &extracted_copy))
        .await
        .map_err(|error| {
            AppError::message(format!("Archive extraction stopped unexpectedly: {error}"))
        })??;

    if version_directory.exists() {
        std::fs::remove_dir_all(&version_directory)
            .map_err(|error| AppError::io("Could not replace the cached umu build", error))?;
    }
    std::fs::rename(&extracted, &version_directory)
        .map_err(|error| AppError::io("Could not install umu-launcher", error))?;

    let installed = version_directory.join("umu").join("umu-run");
    if !installed.is_file() {
        return Err(AppError::message(
            "The umu-launcher archive did not contain umu-run.",
        ));
    }
    installer::make_executable(&installed)?;
    Ok(tag)
}

fn single_root(directory: &Path) -> AppResult<PathBuf> {
    let entries = std::fs::read_dir(directory)
        .map_err(|error| AppError::io("Could not read the extracted archive", error))?;
    let mut directories = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir());
    let first = directories
        .next()
        .ok_or_else(|| AppError::message("The archive did not contain a folder."))?;
    if directories.next().is_some() {
        return Err(AppError::message(
            "The archive has an unexpected folder layout.",
        ));
    }
    Ok(first)
}

fn extract_tar_gz(archive_path: &Path, destination: &Path) -> AppResult<()> {
    let file = std::fs::File::open(archive_path)
        .map_err(|error| AppError::io("Could not open the downloaded archive", error))?;
    extract_tar(flate2::read::GzDecoder::new(file), destination)
}

fn extract_tar_file(archive_path: &Path, destination: &Path) -> AppResult<()> {
    let file = std::fs::File::open(archive_path)
        .map_err(|error| AppError::io("Could not open the downloaded archive", error))?;
    extract_tar(file, destination)
}

fn extract_tar<R: Read>(reader: R, destination: &Path) -> AppResult<()> {
    let mut archive = tar::Archive::new(reader);
    let entries = archive
        .entries()
        .map_err(|error| AppError::message(format!("The archive could not be read: {error}")))?;

    for entry in entries {
        let mut entry =
            entry.map_err(|error| AppError::message(format!("The archive is damaged: {error}")))?;
        let path = entry
            .path()
            .map_err(|error| AppError::message(format!("The archive is damaged: {error}")))?;
        let relative = safe_relative_path(&path)
            .ok_or_else(|| AppError::message("The archive contains an unsafe path."))?;
        let output = destination.join(&relative);
        let kind = entry.header().entry_type();

        if kind.is_dir() {
            std::fs::create_dir_all(&output)
                .map_err(|error| AppError::io("Could not create an extracted folder", error))?;
            continue;
        }
        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| AppError::io("Could not create an extracted folder", error))?;
        }

        if kind.is_symlink() {
            let target = entry
                .link_name()
                .map_err(|error| AppError::message(format!("The archive is damaged: {error}")))?
                .ok_or_else(|| AppError::message("The archive has a link without a target."))?;
            if !symlink_target_is_safe(&relative, &target) {
                return Err(AppError::message(
                    "The archive contains a link that escapes its folder.",
                ));
            }
            create_symlink(&target, &output)?;
            continue;
        }

        if kind.is_hard_link() {
            let target = entry
                .link_name()
                .map_err(|error| AppError::message(format!("The archive is damaged: {error}")))?
                .ok_or_else(|| AppError::message("The archive has a link without a target."))?;
            let Some(source) = safe_relative_path(&target).map(|path| destination.join(path))
            else {
                return Err(AppError::message(
                    "The archive contains a link that escapes its folder.",
                ));
            };
            if !source.is_file() {
                continue;
            }
            std::fs::copy(&source, &output)
                .map_err(|error| AppError::io("Could not extract a linked file", error))?;
            continue;
        }

        if kind.is_file() {
            let mut file = std::fs::File::create(&output)
                .map_err(|error| AppError::io("Could not create an extracted file", error))?;
            std::io::copy(&mut entry, &mut file)
                .map_err(|error| AppError::io("Could not extract a file", error))?;

            #[cfg(unix)]
            if let Ok(mode) = entry.header().mode() {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(metadata) = file.metadata() {
                    let mut perms = metadata.permissions();
                    perms.set_mode(mode);
                    let _ = file.set_permissions(perms);
                }
            }
        }
    }
    Ok(())
}

fn safe_relative_path(path: &Path) -> Option<PathBuf> {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => result.push(part),
            Component::CurDir => {}
            _ => return None,
        }
    }
    (!result.as_os_str().is_empty()).then_some(result)
}

fn symlink_target_is_safe(relative: &Path, target: &Path) -> bool {
    if target.is_absolute() {
        return false;
    }
    let mut depth = relative
        .parent()
        .map(|parent| parent.components().count() as i64)
        .unwrap_or(0);
    for component in target.components() {
        match component {
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            Component::ParentDir => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            _ => return false,
        }
    }
    true
}

#[cfg(unix)]
fn create_symlink(target: &Path, output: &Path) -> AppResult<()> {
    std::os::unix::fs::symlink(target, output)
        .map_err(|error| AppError::io("Could not extract a link", error))
}

#[cfg(not(unix))]
fn create_symlink(_target: &Path, _output: &Path) -> AppResult<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use super::{Mutex, ProcessTracker};
    use super::{asset_arch, extract_tar_gz, natural_cmp, sanitize_name, select_asset, single_root};
    use crate::models::{ReleaseAsset, ReleaseSummary};

    fn release(assets: Vec<(&str, u64)>) -> ReleaseSummary {
        ReleaseSummary {
            tag: "GE-Proton11-7".into(),
            name: "GE-Proton11-7".into(),
            prerelease: false,
            published_at: None,
            assets: assets
                .into_iter()
                .map(|(name, size)| ReleaseAsset {
                    name: name.into(),
                    download_url: format!("https://example.invalid/{name}"),
                    size,
                    digest: Some("sha256:aabb".into()),
                })
                .collect(),
        }
    }

    #[test]
    fn select_asset_prefers_arch_suffixed_tarball_and_falls_back_to_legacy_name() {
        let summary = release(vec![
            ("GE-Proton11-7-aarch64.sha512sum", 159),
            ("GE-Proton11-7-aarch64.tar.gz", 645_786_140),
            ("GE-Proton11-7-x86_64.sha512sum", 158),
            ("GE-Proton11-7-x86_64.tar.gz", 563_784_602),
        ]);
        let x86 = select_asset(&summary, "x86_64").expect("x86_64 asset");
        assert_eq!(x86.name, "GE-Proton11-7-x86_64.tar.gz");
        let arm = select_asset(&summary, "aarch64").expect("aarch64 asset");
        assert_eq!(arm.name, "GE-Proton11-7-aarch64.tar.gz");

        let checksum_only = release(vec![("GE-Proton11-7-x86_64.sha512sum", 158)]);
        assert!(select_asset(&checksum_only, "x86_64").is_none());

        let legacy = release(vec![("GE-Proton11-7.tar.gz", 560_000_000)]);
        assert_eq!(
            select_asset(&legacy, "x86_64").expect("legacy asset").name,
            "GE-Proton11-7.tar.gz"
        );
        assert!(select_asset(&legacy, "aarch64").is_none());

        let both = release(vec![
            ("GE-Proton11-7.tar.gz", 1),
            ("GE-Proton11-7-x86_64.tar.gz", 563_784_602),
        ]);
        assert_eq!(
            select_asset(&both, "x86_64").expect("arch asset").name,
            "GE-Proton11-7-x86_64.tar.gz"
        );

        assert!(asset_arch().is_ok());
    }

    #[test]
    fn extract_tar_gz_writes_files_and_resolves_single_root() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let archive_path = temporary.path().join("ok.tar.gz");
        {
            let file = std::fs::File::create(&archive_path).expect("archive file");
            let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::fast());
            let mut builder = tar::Builder::new(encoder);
            let mut header = tar::Header::new_gnu();
            header.set_size(5);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, "GE-Proton11-7/proton", &b"hello"[..])
                .expect("append file");
            builder
                .into_inner()
                .expect("finish tar")
                .finish()
                .expect("finish gzip");
        }

        let destination = temporary.path().join("out");
        std::fs::create_dir_all(&destination).expect("destination");
        extract_tar_gz(&archive_path, &destination).expect("extract");
        let extracted = destination.join("GE-Proton11-7").join("proton");
        assert_eq!(std::fs::read_to_string(&extracted).expect("read"), "hello");
        assert_eq!(
            single_root(&destination).expect("single root"),
            destination.join("GE-Proton11-7")
        );
    }

    #[test]
    fn sanitize_name_rejects_path_traversal_characters() {
        assert_eq!(sanitize_name("GE-Proton11-7"), "GE-Proton11-7");
        assert_eq!(sanitize_name("../../etc/passwd"), ".._.._etc_passwd");
        assert_eq!(sanitize_name("a/b\\c"), "a_b_c");
    }

    #[test]
    fn natural_cmp_sorts_versions_numerically_not_lexicographically() {
        use std::cmp::Ordering;

        assert_eq!(
            natural_cmp("GE-Proton9-27", "GE-Proton10-1"),
            Ordering::Less
        );
        assert_eq!(natural_cmp("1.4.4", "1.10.0"), Ordering::Less);
        assert_eq!(
            natural_cmp("GE-Proton11-7", "GE-Proton11-7"),
            Ordering::Equal
        );

        let mut tags = vec![
            "GE-Proton9-27".to_owned(),
            "GE-Proton10-1".to_owned(),
            "GE-Proton11-7".to_owned(),
        ];
        super::sort_tags_newest_first(&mut tags);
        assert_eq!(
            tags,
            vec!["GE-Proton11-7", "GE-Proton10-1", "GE-Proton9-27"]
        );
    }

    #[cfg(unix)]
    #[test]
    fn process_tracker_reports_running_and_reaps_exited_children() {
        let empty = ProcessTracker {
            processes: Mutex::new(Vec::new()),
        };
        assert!(!empty.has_running());

        let running_child = std::process::Command::new("sleep")
            .arg("2")
            .spawn()
            .expect("spawn sleeper");
        let running = ProcessTracker {
            processes: Mutex::new(vec![running_child]),
        };
        assert!(running.has_running());

        let mut exited_child = std::process::Command::new("true")
            .spawn()
            .expect("spawn true");
        exited_child.wait().expect("wait");
        let exited = ProcessTracker {
            processes: Mutex::new(vec![exited_child]),
        };
        assert!(!exited.has_running());
    }
}
