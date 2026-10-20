use std::collections::{BTreeMap, HashSet};
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use fs2::available_space;
use tauri::{AppHandle, ipc::Channel};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{ChildStdin, Command};
use tokio::sync::Mutex as AsyncMutex;
use tokio_util::sync::CancellationToken;
use walkdir::WalkDir;

use crate::depot_errors::{DepotFailure, DepotFailureScanner};
use crate::error::{AppError, AppResult};
use crate::github::GitHubClient;
use crate::missions;
use crate::models::{
    AuthMethod, BASE_DEPOT, DepotSpec, FRESH_INSTALL_BYTES, GAME_EXECUTABLE, InstallerState,
    LanguageSpec, OperationEvent, OperationKind, OperationRequest, OperationResult, REPAIR_BYTES,
    ReleaseInfo, STEAM_APP_ID, UPDATE_BYTES, depots_for, resolve_language,
};
use crate::storage;

pub async fn run(
    app: &AppHandle,
    request: OperationRequest,
    on_event: Channel<OperationEvent>,
    cancel: CancellationToken,
    terminal_input: Arc<AsyncMutex<Option<ChildStdin>>>,
) -> AppResult<OperationResult> {
    validate_username(request.kind, &request.steam_username)?;
    let existing_game = Path::new(request.install_directory.trim())
        .join(GAME_EXECUTABLE)
        .is_file();
    let required = match request.kind {
        OperationKind::Install if existing_game => REPAIR_BYTES,
        OperationKind::Install => FRESH_INSTALL_BYTES,
        OperationKind::Repair => REPAIR_BYTES,
        OperationKind::Update | OperationKind::Missions => UPDATE_BYTES,
    };
    progress(
        &on_event,
        "preflight",
        if existing_game {
            "Existing Destiny 2 installation found; preparing file verification…"
        } else {
            "Checking the installation folder…"
        },
        1,
    );
    let root = prepare_install_root(&request.install_directory, required)?;
    ensure_game_closed()?;
    if matches!(request.kind, OperationKind::Missions) {
        return update_missions(&root, &on_event).await;
    }
    let requested_language = *resolve_language(&request.steam_language);
    let existing_state = storage::load_state(&root).await?;
    let installed_language = storage::installed_language(&root, existing_state.as_ref())
        .await
        .copied();
    let operation_language = if matches!(request.kind, OperationKind::Update) {
        installed_language.unwrap_or(requested_language)
    } else {
        requested_language
    };
    // Old files are only known for an install this launcher or the old installer recorded.
    let previous_language =
        existing_state
            .as_ref()
            .zip(installed_language)
            .and_then(|(state, previous)| {
                (previous.steam_language != operation_language.steam_language).then(|| {
                    let manifest_id = state
                        .manifests
                        .get(&previous.depot.depot_id)
                        .copied()
                        .unwrap_or(previous.depot.manifest_id);
                    (previous, manifest_id)
                })
            });
    let mut obsolete_files = existing_state
        .as_ref()
        .map(|state| state.pending_language_files.clone())
        .unwrap_or_default();

    if !matches!(request.kind, OperationKind::Install) {
        verify_game_files(&root)?;
    }

    let github = GitHubClient::new()?;
    if matches!(request.kind, OperationKind::Install | OperationKind::Repair) {
        progress(&on_event, "tool", "Preparing DepotDownloader…", 3);
        let downloader = ensure_depot_downloader(app, &github, &on_event, &cancel).await?;
        cancel_check(&cancel)?;
        download_depots(
            &downloader,
            &root,
            request.steam_username.trim(),
            request.auth_method,
            &operation_language,
            matches!(request.kind, OperationKind::Repair),
            &on_event,
            &cancel,
            terminal_input.clone(),
        )
        .await?;
        verify_game_files(&root)?;
        if let Some((previous, previous_manifest_id)) = previous_language {
            obsolete_files = find_obsolete_language_files(
                &downloader,
                request.steam_username.trim(),
                request.auth_method,
                &previous,
                previous_manifest_id,
                &operation_language,
                obsolete_files,
                &on_event,
                &cancel,
                terminal_input.clone(),
            )
            .await?;
        }
        if matches!(request.kind, OperationKind::Repair) {
            progress(
                &on_event,
                "repair",
                "Clearing Sunrise configuration and cached data…",
                88,
            );
            delete_sunrise_data(&root)?;
        }
    }

    cancel_check(&cancel)?;
    progress(
        &on_event,
        "release",
        "Checking the latest Sunrise release…",
        89,
    );
    let release = github
        .latest_release("stanuwu", "Sunrise", "steam_api64.dll")
        .await?;

    if matches!(request.kind, OperationKind::Update)
        && installation_is_current(&root, &release).await?
    {
        if let Some(state) = existing_state {
            finish_language_cleanup(&root, state, &on_event).await?;
        }
        progress(
            &on_event,
            "complete",
            &format!("Sunrise {} is already current.", release.tag),
            100,
        );
        return Ok(OperationResult {
            changed: false,
            release_tag: release.tag.clone(),
            message: format!("Sunrise {} is already current.", release.tag),
        });
    }

    let cache_root = storage::app_cache_dir(app)?;
    tokio::fs::create_dir_all(&cache_root)
        .await
        .map_err(|error| AppError::io("Could not create the download cache", error))?;
    let staging = tempfile::Builder::new()
        .prefix("sunrise-payload-")
        .tempdir_in(&cache_root)
        .map_err(|error| AppError::io("Could not create a temporary download folder", error))?;
    let payload = staging.path().join("steam_api64.dll");
    let payload_hash = github
        .download(&release, &payload, &on_event, "release", 90, 96)
        .await?;
    cancel_check(&cancel)?;

    let settings = storage::prepare_sunrise_settings(
        &root,
        &payload,
        operation_language.steam_language,
        matches!(request.kind, OperationKind::Repair),
    )
    .await?;
    // Install and Repair start without scripts; Repair deleted them with the Sunrise data.
    let missions_commit = if matches!(request.kind, OperationKind::Update) {
        existing_state
            .as_ref()
            .and_then(|state| state.missions_commit.clone())
    } else if missions::is_git_checkout(&root) {
        let _ = on_event.send(OperationEvent::Notice {
            message: "The scripts folder is a git checkout, so the missions were left as they are."
                .into(),
        });
        None
    } else {
        progress(&on_event, "missions", "Installing the latest missions…", 97);
        Some(missions::install_latest(&github, &root).await?)
    };
    progress(&on_event, "install", "Installing Sunrise…", 98);
    install_payload(
        &root,
        &payload,
        &payload_hash,
        matches!(request.kind, OperationKind::Install | OperationKind::Repair),
    )
    .await?;
    storage::write_sunrise_settings(&root, &settings).await?;
    let manifests = if matches!(request.kind, OperationKind::Update) {
        existing_state
            .as_ref()
            .map(|state| state.manifests.clone())
            .unwrap_or_else(|| language_manifests(&operation_language))
    } else {
        language_manifests(&operation_language)
    };
    let state = InstallerState {
        schema_version: 2,
        app_id: STEAM_APP_ID,
        release_tag: release.tag.clone(),
        release_asset: release.asset_name.clone(),
        release_asset_digest: release.digest.clone(),
        installed_dll_sha256: payload_hash,
        installed_at_utc: chrono::Utc::now(),
        steam_language: Some(operation_language.steam_language.into()),
        manifests,
        pending_language_files: obsolete_files,
        missions_commit,
    };
    // The new language is saved before old files go, so an interrupted cleanup still boots.
    storage::save_state(&root, &state).await?;
    finish_language_cleanup(&root, state, &on_event).await?;

    let verb = match request.kind {
        OperationKind::Install => "Installed",
        OperationKind::Repair => "Repaired",
        OperationKind::Update | OperationKind::Missions => "Updated",
    };
    let message = format!("{verb} Sunrise {} successfully.", release.tag);
    progress(&on_event, "complete", &message, 100);
    Ok(OperationResult {
        changed: true,
        release_tag: release.tag,
        message,
    })
}

async fn update_missions(
    root: &Path,
    on_event: &Channel<OperationEvent>,
) -> AppResult<OperationResult> {
    verify_game_files(root)?;
    progress(on_event, "missions", "Downloading the latest missions…", 10);
    let commit = missions::install_latest(&GitHubClient::new()?, root).await?;
    if let Some(mut state) = storage::load_state(root).await? {
        state.missions_commit = Some(commit.clone());
        storage::save_state(root, &state).await?;
    }
    let short = &commit[..7];
    let message = format!("Installed missions {short}.");
    progress(on_event, "complete", &message, 100);
    Ok(OperationResult {
        changed: true,
        release_tag: short.into(),
        message,
    })
}

fn language_manifests(language: &LanguageSpec) -> BTreeMap<u32, u64> {
    depots_for(language)
        .into_iter()
        .map(|depot| (depot.depot_id, depot.manifest_id))
        .collect()
}

fn progress(on_event: &Channel<OperationEvent>, stage: &str, message: &str, percent: u8) {
    let _ = on_event.send(OperationEvent::Progress {
        stage: stage.into(),
        message: message.into(),
        percent: f32::from(percent),
    });
}

fn cancel_check(cancel: &CancellationToken) -> AppResult<()> {
    if cancel.is_cancelled() {
        Err(AppError::Cancelled)
    } else {
        Ok(())
    }
}

fn validate_username(kind: OperationKind, username: &str) -> AppResult<()> {
    if matches!(kind, OperationKind::Update | OperationKind::Missions) {
        return Ok(());
    }
    if username.trim().is_empty() {
        return Err(AppError::message(
            "Enter the Steam account name that owns Destiny 2.",
        ));
    }
    if username.chars().any(char::is_control) {
        return Err(AppError::message(
            "The Steam account name contains an invalid character.",
        ));
    }
    Ok(())
}

fn prepare_install_root(raw: &str, required_bytes: u64) -> AppResult<PathBuf> {
    if raw.trim().is_empty() {
        return Err(AppError::message("Select an installation folder."));
    }
    let requested = PathBuf::from(raw.trim());
    if !requested.is_absolute() {
        return Err(AppError::message(
            "Choose an absolute installation folder path.",
        ));
    }
    std::fs::create_dir_all(&requested)
        .map_err(|error| AppError::io("Could not create the installation folder", error))?;
    let root = requested
        .canonicalize()
        .map_err(|error| AppError::io("Could not resolve the installation folder", error))?;
    if root.parent().is_none() {
        return Err(AppError::message(
            "Choose a folder inside a drive, not the drive itself.",
        ));
    }

    tempfile::Builder::new()
        .prefix(".sunrise-write-")
        .tempfile_in(&root)
        .map_err(|error| {
            AppError::io(
                "The launcher cannot write to this installation folder",
                error,
            )
        })?;

    let free = available_space(&root)
        .map_err(|error| AppError::io("Could not check available disk space", error))?;
    if free < required_bytes {
        return Err(AppError::message(format!(
            "Not enough free space. {} is required, but only {} is available.",
            format_bytes(required_bytes),
            format_bytes(free)
        )));
    }

    Ok(root)
}

fn format_bytes(bytes: u64) -> String {
    format!("{:.1} GiB", bytes as f64 / (1024_f64 * 1024_f64 * 1024_f64))
}

fn ensure_game_closed() -> AppResult<()> {
    #[cfg(windows)]
    {
        let output = std::process::Command::new("tasklist")
            .args(["/FI", "IMAGENAME eq destiny2.exe", "/FO", "CSV", "/NH"])
            .output();
        if output.is_ok_and(|value| {
            String::from_utf8_lossy(&value.stdout)
                .to_ascii_lowercase()
                .contains("destiny2.exe")
        }) {
            return Err(AppError::message(
                "Close Destiny 2 before installing, repairing, or updating Sunrise.",
            ));
        }
    }
    Ok(())
}

fn verify_game_files(root: &Path) -> AppResult<()> {
    if !root.join(GAME_EXECUTABLE).is_file() {
        return Err(AppError::message(
            "DepotDownloader finished, but destiny2.exe is missing.",
        ));
    }
    if !root.join("bin").join("x64").is_dir() {
        return Err(AppError::message(
            "DepotDownloader finished, but the bin/x64 folder is missing.",
        ));
    }
    Ok(())
}

async fn installation_is_current(root: &Path, release: &ReleaseInfo) -> AppResult<bool> {
    let Some(state) = storage::load_state(root).await? else {
        return Ok(false);
    };
    if !state.release_tag.eq_ignore_ascii_case(&release.tag) {
        return Ok(false);
    }
    if let (Some(installed), Some(latest)) = (
        state.release_asset_digest.as_deref(),
        release.digest.as_deref(),
    ) && !installed.eq_ignore_ascii_case(latest)
    {
        return Ok(false);
    }
    let dll = storage::mod_path(root);
    if !dll.is_file() {
        return Ok(false);
    }
    Ok(storage::hash_file(&dll)
        .await?
        .eq_ignore_ascii_case(&state.installed_dll_sha256))
}

async fn ensure_depot_downloader(
    app: &AppHandle,
    github: &GitHubClient,
    on_event: &Channel<OperationEvent>,
    cancel: &CancellationToken,
) -> AppResult<PathBuf> {
    let release = depot_downloader_release()?;
    let versions = storage::app_data_dir(app)?
        .join("tools")
        .join("DepotDownloader")
        .join("versions");
    tokio::fs::create_dir_all(&versions)
        .await
        .map_err(|error| AppError::io("Could not create the tools folder", error))?;
    let version_directory = versions.join(sanitize_name(&release.tag));
    if let Some(executable) = find_depot_executable(&version_directory) {
        return Ok(executable);
    }

    cancel_check(cancel)?;
    let temporary = tempfile::Builder::new()
        .prefix(".depot-downloader-")
        .tempdir_in(&versions)
        .map_err(|error| AppError::io("Could not create a temporary tools folder", error))?;
    let archive = temporary.path().join("DepotDownloader.zip");
    github
        .download(&release, &archive, on_event, "tool", 3, 8)
        .await?;
    cancel_check(cancel)?;
    let extracted = temporary.path().join("extracted");
    let archive_copy = archive.clone();
    let extracted_copy = extracted.clone();
    tokio::task::spawn_blocking(move || extract_zip(&archive_copy, &extracted_copy))
        .await
        .map_err(|error| {
            AppError::message(format!("Archive extraction stopped unexpectedly: {error}"))
        })??;

    if version_directory.exists() {
        std::fs::remove_dir_all(&version_directory)
            .map_err(|error| AppError::io("Could not replace the cached tool", error))?;
    }
    std::fs::rename(&extracted, &version_directory)
        .map_err(|error| AppError::io("Could not cache DepotDownloader", error))?;
    let executable = find_depot_executable(&version_directory)
        .ok_or_else(|| AppError::message("The DepotDownloader archive has no executable."))?;
    make_executable(&executable)?;
    Ok(executable)
}

// The output parsers match this release's text; re-check them before changing the pin.
const DEPOT_DOWNLOADER_TAG: &str = "DepotDownloader_3.4.0";

// Asset name, size in bytes and SHA-256 per platform. GitHub publishes no digest for this release.
const DEPOT_DOWNLOADER_ASSETS: [(&str, &str, &str, u64, &str); 6] = [
    (
        "windows",
        "x86_64",
        "DepotDownloader-windows-x64.zip",
        33_474_005,
        "41c9e9f0df54b3ad02e67a11726756e5c73283bd7c2e1b04acfa5ae4c2ed3767",
    ),
    (
        "windows",
        "aarch64",
        "DepotDownloader-windows-arm64.zip",
        32_428_618,
        "1449ba47775e9974036e615bedf00d72bef747cc5b93a78048ed4e0b2c63b2b3",
    ),
    (
        "linux",
        "x86_64",
        "DepotDownloader-linux-x64.zip",
        33_442_357,
        "a999dec66b4850fc961bd50366696d23c2d0fad7b18790e6a5647b2f19097a53",
    ),
    (
        "linux",
        "aarch64",
        "DepotDownloader-linux-arm64.zip",
        32_026_406,
        "d9fb612ccebc1db8eeea3b4045d2221ec70431381393ce908fb72f01d4f9c812",
    ),
    (
        "macos",
        "x86_64",
        "DepotDownloader-macos-x64.zip",
        33_808_980,
        "3214b689564d73e9342a8a4aef693de6ad3d293801b0f300a4466f60ec75befb",
    ),
    (
        "macos",
        "aarch64",
        "DepotDownloader-macos-arm64.zip",
        32_245_964,
        "60e80c7c496f3f9a079cd3c62036b35d088c27bc0149baf38f009eb57a52f6a5",
    ),
];

fn depot_downloader_release() -> AppResult<ReleaseInfo> {
    let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
    let (_, _, asset_name, size, sha256) = DEPOT_DOWNLOADER_ASSETS
        .into_iter()
        .find(|asset| asset.0 == os && asset.1 == arch)
        .ok_or_else(|| {
            AppError::message(format!(
                "DepotDownloader does not publish an asset for {os}/{arch}."
            ))
        })?;
    Ok(ReleaseInfo {
        tag: DEPOT_DOWNLOADER_TAG.into(),
        asset_name: asset_name.into(),
        download_url: format!(
            "https://github.com/SteamRE/DepotDownloader/releases/download/{DEPOT_DOWNLOADER_TAG}/{asset_name}"
        ),
        size,
        digest: Some(format!("sha256:{sha256}")),
    })
}

fn find_depot_executable(root: &Path) -> Option<PathBuf> {
    if !root.exists() {
        return None;
    }
    WalkDir::new(root)
        .into_iter()
        .filter_map(Result::ok)
        .find(|entry| {
            entry.file_type().is_file()
                && (entry
                    .file_name()
                    .eq_ignore_ascii_case(OsStr::new("DepotDownloader"))
                    || entry
                        .file_name()
                        .eq_ignore_ascii_case(OsStr::new("DepotDownloader.exe")))
        })
        .map(|entry| entry.into_path())
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

pub(crate) fn extract_zip(archive_path: &Path, destination: &Path) -> AppResult<()> {
    let file = std::fs::File::open(archive_path)
        .map_err(|error| AppError::io("Could not open the downloaded archive", error))?;
    let mut archive = zip::ZipArchive::new(file)?;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let enclosed = entry
            .enclosed_name()
            .ok_or_else(|| AppError::message("The archive contains an unsafe path."))?;
        let output = destination.join(enclosed);
        if entry.is_dir() {
            std::fs::create_dir_all(&output)
                .map_err(|error| AppError::io("Could not create an extracted folder", error))?;
            continue;
        }
        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| AppError::io("Could not create an extracted folder", error))?;
        }
        let mut file = std::fs::File::create(&output)
            .map_err(|error| AppError::io("Could not create an extracted file", error))?;
        std::io::copy(&mut entry, &mut file)
            .map_err(|error| AppError::io("Could not extract DepotDownloader", error))?;
    }
    Ok(())
}

#[cfg(unix)]
pub(crate) fn make_executable(path: &Path) -> AppResult<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(path)
        .map_err(|error| AppError::io("Could not inspect file", error))?
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions)
        .map_err(|error| AppError::io("Could not make file executable", error))
}

#[cfg(not(unix))]
pub(crate) fn make_executable(_path: &Path) -> AppResult<()> {
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn download_depots(
    executable: &Path,
    install_root: &Path,
    steam_username: &str,
    auth_method: AuthMethod,
    language: &LanguageSpec,
    validate: bool,
    on_event: &Channel<OperationEvent>,
    cancel: &CancellationToken,
    terminal_input: Arc<AsyncMutex<Option<ChildStdin>>>,
) -> AppResult<()> {
    let auth_message = match auth_method {
        AuthMethod::Qr => {
            "Steam authentication is handled directly by DepotDownloader. Scan the QR code when it appears; your password is never stored by this launcher."
        }
        AuthMethod::TwoFactor => {
            "Steam authentication is handled directly by DepotDownloader. Enter your password and Steam Guard code when prompted; neither is stored by this launcher."
        }
    };
    let _ = on_event.send(OperationEvent::Notice {
        message: auth_message.into(),
    });
    for (index, depot) in depots_for(language).into_iter().enumerate() {
        cancel_check(cancel)?;
        let (start_percent, end_percent) = if index == 0 { (12, 49) } else { (50, 85) };
        let message = if index == 0 {
            "Preparing shared game files…".into()
        } else {
            format!("Preparing {} language files…", language.display_name)
        };
        progress(on_event, "depots", &message, start_percent);
        run_depot(
            executable,
            install_root,
            steam_username,
            depot.depot_id,
            depot.manifest_id,
            validate,
            auth_method,
            index == 0,
            start_percent,
            end_percent,
            on_event,
            cancel,
            terminal_input.clone(),
            false,
        )
        .await?;
    }
    progress(on_event, "depots", "Steam game files are ready.", 86);
    Ok(())
}

/** Lists the previous language's files, plus older leftovers, that the new language does not use. */
#[allow(clippy::too_many_arguments)]
async fn find_obsolete_language_files(
    executable: &Path,
    steam_username: &str,
    auth_method: AuthMethod,
    previous_language: &LanguageSpec,
    previous_manifest_id: u64,
    selected_language: &LanguageSpec,
    pending_files: Vec<String>,
    on_event: &Channel<OperationEvent>,
    cancel: &CancellationToken,
    terminal_input: Arc<AsyncMutex<Option<ChildStdin>>>,
) -> AppResult<Vec<String>> {
    progress(
        on_event,
        "language",
        &format!(
            "Switching game language from {} to {}…",
            previous_language.display_name, selected_language.display_name
        ),
        87,
    );
    let previous_depot = DepotSpec::new(previous_language.depot.depot_id, previous_manifest_id);
    let previous_files = read_manifest_files(
        executable,
        steam_username,
        previous_depot,
        auth_method,
        on_event,
        cancel,
        terminal_input.clone(),
    )
    .await?;
    let shared_files = read_manifest_files(
        executable,
        steam_username,
        BASE_DEPOT,
        auth_method,
        on_event,
        cancel,
        terminal_input.clone(),
    )
    .await?;
    let selected_files = read_manifest_files(
        executable,
        steam_username,
        selected_language.depot,
        auth_method,
        on_event,
        cancel,
        terminal_input,
    )
    .await?;
    let keep_files = shared_files
        .iter()
        .chain(&selected_files)
        .map(|path| normalize_manifest_path(path))
        .collect::<HashSet<_>>();
    let mut seen = HashSet::new();
    let obsolete_files = previous_files
        .into_iter()
        .chain(pending_files)
        .map(|path| path.replace('\\', "/"))
        .filter(|path| {
            let normalized = normalize_manifest_path(path);
            !keep_files.contains(&normalized) && seen.insert(normalized)
        })
        .collect::<Vec<_>>();
    progress(
        on_event,
        "language",
        &format!(
            "{} language files are ready.",
            selected_language.display_name
        ),
        88,
    );
    Ok(obsolete_files)
}

async fn finish_language_cleanup(
    install_root: &Path,
    mut state: InstallerState,
    on_event: &Channel<OperationEvent>,
) -> AppResult<()> {
    if state.pending_language_files.is_empty() {
        return Ok(());
    }
    let removed = remove_depot_files(install_root, &state.pending_language_files).map_err(
        |error| {
            AppError::message(format!(
                "Sunrise is installed, but some old language files could not be removed. Run Check or Update again to finish. {error}"
            ))
        },
    )?;
    state.pending_language_files.clear();
    storage::save_state(install_root, &state).await?;
    let _ = on_event.send(OperationEvent::Notice {
        message: format!("Removed {removed} old language file(s)."),
    });
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn read_manifest_files(
    executable: &Path,
    steam_username: &str,
    depot: DepotSpec,
    auth_method: AuthMethod,
    on_event: &Channel<OperationEvent>,
    cancel: &CancellationToken,
    terminal_input: Arc<AsyncMutex<Option<ChildStdin>>>,
) -> AppResult<Vec<String>> {
    cancel_check(cancel)?;
    let temporary = tempfile::Builder::new()
        .prefix("sunrise-manifest-")
        .tempdir()
        .map_err(|error| AppError::io("Could not create a manifest folder", error))?;
    run_depot(
        executable,
        temporary.path(),
        steam_username,
        depot.depot_id,
        depot.manifest_id,
        false,
        auth_method,
        false,
        86,
        86,
        on_event,
        cancel,
        terminal_input,
        true,
    )
    .await?;
    let manifest_path = temporary.path().join(format!(
        "manifest_{}_{}.txt",
        depot.depot_id, depot.manifest_id
    ));
    let manifest = tokio::fs::read_to_string(&manifest_path)
        .await
        .map_err(|error| {
            AppError::io("DepotDownloader did not produce its manifest file", error)
        })?;
    let files = manifest
        .lines()
        .filter_map(parse_manifest_file)
        .collect::<Vec<_>>();
    if files.is_empty() {
        return Err(AppError::message(format!(
            "Depot {} manifest contained no readable files.",
            depot.depot_id
        )));
    }
    Ok(files)
}

fn parse_manifest_file(line: &str) -> Option<String> {
    let mut remainder = line.trim();
    let size = take_manifest_field(&mut remainder)?;
    let chunks = take_manifest_field(&mut remainder)?;
    let hash = take_manifest_field(&mut remainder)?;
    let flags = take_manifest_field(&mut remainder)?;
    size.parse::<u64>().ok()?;
    chunks.parse::<u32>().ok()?;
    (hash.len() == 40 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())).then_some(())?;
    u32::from_str_radix(flags, 16).ok()?;
    (!remainder.is_empty()).then(|| remainder.replace('\\', "/"))
}

fn take_manifest_field<'a>(remainder: &mut &'a str) -> Option<&'a str> {
    let boundary = remainder.find(char::is_whitespace)?;
    let field = &remainder[..boundary];
    *remainder = remainder[boundary..].trim_start();
    (!field.is_empty()).then_some(field)
}

fn normalize_manifest_path(path: &str) -> String {
    path.replace('\\', "/")
        .trim_start_matches("./")
        .to_ascii_lowercase()
}

fn remove_depot_files(install_root: &Path, relative_paths: &[String]) -> AppResult<usize> {
    let root = install_root
        .canonicalize()
        .map_err(|error| AppError::io("Could not resolve the installation folder", error))?;
    let mut removed = 0;
    for relative_path in relative_paths {
        let normalized = relative_path.replace('\\', "/");
        let relative = Path::new(&normalized);
        if normalized.is_empty()
            || normalized.contains(':')
            || relative.is_absolute()
            || relative.components().any(|component| {
                matches!(
                    component,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
        {
            return Err(AppError::message(
                "A language depot contained an unsafe file path.",
            ));
        }
        let target = root.join(relative);
        if !target.exists() {
            continue;
        }
        let resolved = target
            .canonicalize()
            .map_err(|error| AppError::io("Could not inspect a language file", error))?;
        if !resolved.starts_with(&root) {
            return Err(AppError::message(
                "A language depot contained an unsafe file path.",
            ));
        }
        if resolved.is_file() {
            std::fs::remove_file(&target)
                .map_err(|error| AppError::io("Could not remove an old language file", error))?;
            removed += 1;
        }
    }
    Ok(removed)
}

#[allow(clippy::too_many_arguments)]
async fn run_depot(
    executable: &Path,
    install_root: &Path,
    steam_username: &str,
    depot: u32,
    manifest: u64,
    validate: bool,
    auth_method: AuthMethod,
    first_depot: bool,
    start_percent: u8,
    end_percent: u8,
    on_event: &Channel<OperationEvent>,
    cancel: &CancellationToken,
    terminal_input: Arc<AsyncMutex<Option<ChildStdin>>>,
    manifest_only: bool,
) -> AppResult<()> {
    let mut command = Command::new(executable);
    command
        .current_dir(executable.parent().unwrap_or_else(|| Path::new(".")))
        .args(["-app", &STEAM_APP_ID.to_string()])
        .args(["-depot", &depot.to_string()])
        .args(["-manifest", &manifest.to_string()])
        .arg("-dir")
        .arg(install_root)
        .args(authentication_args(
            steam_username,
            auth_method,
            first_depot,
        ))
        .args(["-os", "windows"])
        .args(["-osarch", "64"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    if manifest_only {
        command.arg("-manifest-only");
    }
    if validate {
        command.arg("-validate");
    }
    #[cfg(windows)]
    {
        command.creation_flags(0x0800_0000);
    }

    let mut child = command
        .spawn()
        .map_err(|error| AppError::io("DepotDownloader could not be started", error))?;
    *terminal_input.lock().await = child.stdin.take();
    let depot_progress = (!manifest_only).then(|| DepotProgress::new(start_percent, end_percent));
    let stdout_task = child
        .stdout
        .take()
        .map(|stdout| stream_output(stdout, "stdout", on_event.clone(), depot_progress));
    let stderr_task = child
        .stderr
        .take()
        .map(|stderr| stream_output(stderr, "stderr", on_event.clone(), None));

    let outcome = tokio::select! {
        status = child.wait() => status
            .map_err(|error| AppError::io("Could not wait for DepotDownloader", error)),
        _ = cancel.cancelled() => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            Err(AppError::Cancelled)
        }
    };
    *terminal_input.lock().await = None;
    let mut failure = None;
    for task in [stdout_task, stderr_task].into_iter().flatten() {
        if let Ok(Some(found)) = task.await {
            failure.get_or_insert(found);
        }
    }
    let status = outcome?;
    // DepotDownloader exits 0 when a depot is refused, so a reported failure wins.
    if let Some(failure) = failure {
        return Err(AppError::message(failure.refine(install_root).message()));
    }
    if !status.success() {
        return Err(AppError::message(format!(
            "DepotDownloader stopped with exit code {}. The captured output is available in Settings for diagnostics.",
            status
                .code()
                .map_or_else(|| "unknown".into(), |code| code.to_string())
        )));
    }
    Ok(())
}

fn authentication_args(username: &str, auth_method: AuthMethod, first_depot: bool) -> Vec<String> {
    if first_depot && auth_method == AuthMethod::Qr {
        vec!["-qr".into(), "-remember-password".into()]
    } else {
        let mut args = vec![
            "-username".into(),
            username.into(),
            "-remember-password".into(),
        ];
        if first_depot && auth_method == AuthMethod::TwoFactor {
            args.push("-no-mobile".into());
        }
        args
    }
}

#[derive(Default)]
struct TerminalDecoder {
    pending: Vec<u8>,
}

impl TerminalDecoder {
    fn push(&mut self, bytes: &[u8]) -> String {
        self.pending.extend_from_slice(bytes);
        let mut output = String::new();
        loop {
            match std::str::from_utf8(&self.pending) {
                Ok(text) => {
                    output.push_str(text);
                    self.pending.clear();
                    break;
                }
                Err(error) => {
                    let valid = error.valid_up_to();
                    if valid > 0 {
                        output.push_str(
                            std::str::from_utf8(&self.pending[..valid])
                                .expect("the decoder reported a valid UTF-8 prefix"),
                        );
                        self.pending.drain(..valid);
                        continue;
                    }
                    let Some(invalid_length) = error.error_len() else {
                        break;
                    };
                    let invalid: Vec<u8> = self.pending.drain(..invalid_length).collect();
                    for byte in invalid {
                        output.push(decode_oem_character(byte));
                    }
                }
            }
        }
        output
    }

    fn finish(&mut self) -> String {
        self.pending.drain(..).map(decode_oem_character).collect()
    }
}

fn decode_oem_character(byte: u8) -> char {
    match byte {
        0xB0 => '░',
        0xB1 => '▒',
        0xB2 => '▓',
        0xDB => '█',
        0xDC => '▄',
        0xDF => '▀',
        0x00..=0x7F => char::from(byte),
        _ => '�',
    }
}

#[derive(Default)]
struct QrCapture {
    rows: Vec<String>,
    expected_rows: Option<usize>,
}

impl QrCapture {
    fn push_line(&mut self, line: &str) -> Option<Vec<String>> {
        let width = line.chars().count();
        if width == 0 || !line.chars().all(is_qr_character) {
            self.rows.clear();
            self.expected_rows = None;
            return None;
        }
        let expected = *self.expected_rows.get_or_insert(width.div_ceil(2));
        self.rows.push(line.into());
        if self.rows.len() < expected {
            return None;
        }
        self.expected_rows = None;
        Some(std::mem::take(&mut self.rows))
    }
}

fn is_qr_character(character: char) -> bool {
    matches!(character, ' ' | '░' | '▒' | '▓' | '█' | '▄' | '▀')
}

struct TerminalEventParser {
    line_buffer: String,
    qr_capture: Option<QrCapture>,
    auth_prompts: AuthPromptScanner,
    auth_completion: AuthCompletionScanner,
    depot_progress: Option<DepotProgress>,
    failures: DepotFailureScanner,
}

impl TerminalEventParser {
    fn new(depot_progress: Option<DepotProgress>) -> Self {
        Self {
            line_buffer: String::new(),
            qr_capture: None,
            auth_prompts: AuthPromptScanner::default(),
            auth_completion: AuthCompletionScanner::default(),
            depot_progress,
            failures: DepotFailureScanner::default(),
        }
    }

    fn push(&mut self, text: &str, on_event: &Channel<OperationEvent>) {
        for (kind, message) in self.auth_prompts.push(text) {
            let _ = on_event.send(OperationEvent::AuthPrompt {
                kind: kind.into(),
                message: message.into(),
            });
        }
        if self.auth_completion.push(text) {
            let _ = on_event.send(OperationEvent::AuthComplete);
        }
        self.line_buffer.push_str(text);
        while let Some(newline) = self.line_buffer.find('\n') {
            let mut remainder = self.line_buffer.split_off(newline + 1);
            std::mem::swap(&mut remainder, &mut self.line_buffer);
            let line = remainder.trim_end_matches(['\r', '\n']);
            self.observe_line(line, on_event);
        }
    }

    fn finish(&mut self, on_event: &Channel<OperationEvent>) {
        if !self.line_buffer.is_empty() {
            let line = std::mem::take(&mut self.line_buffer);
            self.observe_line(&line, on_event);
        }
    }

    fn observe_line(&mut self, line: &str, on_event: &Channel<OperationEvent>) {
        self.failures.observe_line(line);
        if let Some(progress) = self.depot_progress.as_mut()
            && let Some(update) = progress.observe_line(line)
        {
            let _ = on_event.send(OperationEvent::Progress {
                stage: "depots".into(),
                message: update.message,
                percent: update.percent,
            });
        }
        if line.contains("Use the Steam Mobile App to sign in with this QR code:") {
            self.qr_capture = Some(QrCapture::default());
            return;
        }
        if let Some(capture) = self.qr_capture.as_mut() {
            if let Some(rows) = capture.push_line(line) {
                let _ = on_event.send(OperationEvent::QrCode { rows });
                self.qr_capture = None;
            } else if capture.expected_rows.is_none() && capture.rows.is_empty() {
                self.qr_capture = None;
            }
        }
    }
}

struct DepotProgress {
    start_percent: u8,
    end_percent: u8,
    depot_percent: f32,
}

impl DepotProgress {
    fn new(start_percent: u8, end_percent: u8) -> Self {
        Self {
            start_percent,
            end_percent,
            depot_percent: 0.0,
        }
    }

    fn observe_line(&mut self, line: &str) -> Option<DepotProgressUpdate> {
        if let Some(file_name) = parse_validating_file(line) {
            return Some(DepotProgressUpdate {
                percent: self.overall_percent(),
                message: format!("Validating {file_name}"),
            });
        }

        let (depot_percent, file_name) = parse_depot_file(line)?;
        self.depot_percent = self.depot_percent.max(depot_percent);
        Some(DepotProgressUpdate {
            percent: self.overall_percent(),
            message: format!("Downloading {file_name}"),
        })
    }

    fn overall_percent(&self) -> f32 {
        let span = self.end_percent.saturating_sub(self.start_percent) as f32;
        (f32::from(self.start_percent) + span * self.depot_percent / 100.0)
            .min(f32::from(self.end_percent))
    }
}

struct DepotProgressUpdate {
    percent: f32,
    message: String,
}

fn parse_validating_file(line: &str) -> Option<&str> {
    line.trim_start()
        .strip_prefix("Validating ")
        .and_then(terminal_file_name)
}

fn parse_depot_file(line: &str) -> Option<(f32, &str)> {
    let trimmed = line.trim_start();
    let (number, remainder) = trimmed.split_once('%')?;
    if number.is_empty()
        || number
            .chars()
            .any(|character| !character.is_ascii_digit() && character != '.')
        || !remainder.starts_with(char::is_whitespace)
    {
        return None;
    }
    let percent = number.parse::<f32>().ok()?;
    let file_name = terminal_file_name(remainder)?;
    percent
        .is_finite()
        .then_some((percent.clamp(0.0, 100.0), file_name))
}

fn terminal_file_name(path: &str) -> Option<&str> {
    path.trim()
        .rsplit(['\\', '/'])
        .find(|component| !component.is_empty())
}

const CODE_REJECTED_MARKER: &str = "previous 2-factor auth code you have provided is incorrect";
const CODE_REJECTED_MESSAGE: &str = "Steam did not accept that code. Enter a new Steam Guard code.";

const AUTH_PROMPT_DEFINITIONS: [(&str, &str, &str); 6] = [
    (CODE_REJECTED_MARKER, "codeRejected", CODE_REJECTED_MESSAGE),
    (
        "enter account password",
        "password",
        "Enter your Steam password",
    ),
    (
        "enter the auth code sent to the email",
        "emailCode",
        "Enter the Steam Guard code sent to your email",
    ),
    (
        "enter your 2-factor auth code",
        "twoFactor",
        "Enter the Steam Guard code from your authenticator",
    ),
    (
        "enter your 2 factor auth code",
        "twoFactor",
        "Enter the Steam Guard code from your authenticator",
    ),
    (
        "authentication code sent to your email",
        "emailCode",
        "Enter the Steam Guard code sent to your email",
    ),
];

#[derive(Default)]
struct AuthPromptScanner {
    buffer: String,
    code_rejected: bool,
}

impl AuthPromptScanner {
    fn push(&mut self, text: &str) -> Vec<(&'static str, &'static str)> {
        self.buffer.push_str(text);
        let lower = self.buffer.to_ascii_lowercase();
        let mut scan_from = 0;
        let mut prompts = Vec::new();

        loop {
            let next = AUTH_PROMPT_DEFINITIONS
                .iter()
                .filter_map(|definition| {
                    lower[scan_from..]
                        .find(definition.0)
                        .map(|relative| (scan_from + relative, definition))
                })
                .min_by_key(|(start, _)| *start);
            let Some((start, definition)) = next else {
                break;
            };
            scan_from = start + definition.0.len();
            // A rejected code is followed by a new prompt, which carries the retry message.
            if definition.1 == "codeRejected" {
                self.code_rejected = true;
            } else if definition.1 != "password" && std::mem::take(&mut self.code_rejected) {
                prompts.push((definition.1, CODE_REJECTED_MESSAGE));
            } else {
                prompts.push((definition.1, definition.2));
            }
        }

        if scan_from > 0 {
            self.buffer.drain(..scan_from);
        }
        self.retain_partial_prompt();
        prompts
    }

    fn retain_partial_prompt(&mut self) {
        let tail_length = AUTH_PROMPT_DEFINITIONS
            .iter()
            .map(|definition| definition.0.len())
            .max()
            .unwrap_or(1)
            .saturating_sub(1);
        if self.buffer.len() <= tail_length {
            return;
        }
        let mut drain_to = self.buffer.len() - tail_length;
        while !self.buffer.is_char_boundary(drain_to) {
            drain_to += 1;
        }
        self.buffer.drain(..drain_to);
    }
}

const AUTH_COMPLETE_MARKERS: [&str; 9] = [
    "using steam3 suggested cellid",
    "got session token",
    "licenses for account",
    "got appinfo for",
    "using app branch:",
    "got depot key for",
    "processing depot ",
    "downloading depot ",
    "got manifest request code",
];

#[derive(Default)]
struct AuthCompletionScanner {
    buffer: String,
    emitted: bool,
}

impl AuthCompletionScanner {
    fn push(&mut self, text: &str) -> bool {
        if self.emitted {
            return false;
        }
        self.buffer.push_str(text);
        let lower = self.buffer.to_ascii_lowercase();
        if AUTH_COMPLETE_MARKERS
            .iter()
            .any(|marker| lower.contains(marker))
        {
            self.emitted = true;
            self.buffer.clear();
            return true;
        }

        let tail_length = AUTH_COMPLETE_MARKERS
            .iter()
            .map(|marker| marker.len())
            .max()
            .unwrap_or(1)
            .saturating_sub(1);
        if self.buffer.len() > tail_length {
            let mut drain_to = self.buffer.len() - tail_length;
            while !self.buffer.is_char_boundary(drain_to) {
                drain_to += 1;
            }
            self.buffer.drain(..drain_to);
        }
        false
    }
}

fn stream_output<R>(
    mut reader: R,
    stream: &'static str,
    on_event: Channel<OperationEvent>,
    depot_progress: Option<DepotProgress>,
) -> tokio::task::JoinHandle<Option<DepotFailure>>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut buffer = vec![0_u8; 4096];
        let mut decoder = TerminalDecoder::default();
        let mut parser = TerminalEventParser::new(depot_progress);
        loop {
            match reader.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(count) => {
                    let text = decoder.push(&buffer[..count]);
                    if text.is_empty() {
                        continue;
                    }
                    parser.push(&text, &on_event);
                    let _ = on_event.send(OperationEvent::Terminal {
                        stream: stream.into(),
                        text,
                    });
                }
            }
        }
        let trailing = decoder.finish();
        if !trailing.is_empty() {
            parser.push(&trailing, &on_event);
            let _ = on_event.send(OperationEvent::Terminal {
                stream: stream.into(),
                text: trailing,
            });
        }
        parser.finish(&on_event);
        parser.failures.failure()
    })
}

fn delete_sunrise_data(root: &Path) -> AppResult<()> {
    let x64 = root.join("bin").join("x64");
    let sunrise = x64.join("Sunrise");
    if !sunrise.starts_with(&x64) {
        return Err(AppError::message("The Sunrise data folder path is unsafe."));
    }
    if sunrise.exists() {
        std::fs::remove_dir_all(sunrise)
            .map_err(|error| AppError::io("Could not clear Sunrise data", error))?;
    }
    Ok(())
}

async fn install_payload(
    root: &Path,
    payload: &Path,
    expected_hash: &str,
    preserve_depot_dll: bool,
) -> AppResult<()> {
    let target = storage::mod_path(root);
    let target_directory = target
        .parent()
        .ok_or_else(|| AppError::message("The game DLL path is invalid."))?;
    if !target_directory.is_dir() {
        return Err(AppError::message(
            "The game bin/x64 folder is missing. Run Install or Repair first.",
        ));
    }

    let metadata_root = root.join(".sunrise");
    let rollback = metadata_root.join("rollback").join("steam_api64.dll");
    let original = metadata_root.join("original").join("steam_api64.dll");
    std::fs::create_dir_all(rollback.parent().expect("rollback has a parent"))
        .map_err(|error| AppError::io("Could not create the rollback folder", error))?;
    if target.is_file() {
        std::fs::copy(&target, &rollback)
            .map_err(|error| AppError::io("Could not create a rollback copy", error))?;
        if preserve_depot_dll {
            std::fs::create_dir_all(original.parent().expect("original has a parent"))
                .map_err(|error| AppError::io("Could not create the original DLL folder", error))?;
            std::fs::copy(&target, &original).map_err(|error| {
                AppError::io("Could not preserve the original Steam DLL", error)
            })?;
        }
    }

    let temporary = target_directory.join(format!(".steam_api64-{}.tmp", std::process::id()));
    tokio::fs::copy(payload, &temporary)
        .await
        .map_err(|error| AppError::io("Could not stage the Sunrise DLL", error))?;
    if target.exists() {
        tokio::fs::remove_file(&target).await.map_err(|error| {
            AppError::io("Could not replace steam_api64.dll; close the game", error)
        })?;
    }
    if let Err(error) = tokio::fs::rename(&temporary, &target).await {
        if rollback.is_file() {
            let _ = tokio::fs::copy(&rollback, &target).await;
        }
        return Err(AppError::io(
            "Could not finish installing the Sunrise DLL",
            error,
        ));
    }

    let actual = storage::hash_file(&target).await?;
    if !actual.eq_ignore_ascii_case(expected_hash) {
        if rollback.is_file() {
            let _ = tokio::fs::copy(&rollback, &target).await;
        }
        return Err(AppError::message(
            "The installed Sunrise DLL failed its SHA-256 check; the previous DLL was restored.",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        AuthCompletionScanner, AuthPromptScanner, DepotProgress, QrCapture, TerminalDecoder,
        authentication_args, depot_downloader_release, parse_depot_file, parse_manifest_file,
        parse_validating_file, remove_depot_files, sanitize_name,
    };
    use crate::models::AuthMethod;

    #[test]
    fn qr_and_username_authentication_are_never_combined() {
        let qr = authentication_args("sunrise_user", AuthMethod::Qr, true);
        assert!(qr.iter().any(|argument| argument == "-qr"));
        assert!(!qr.iter().any(|argument| argument == "-username"));

        let cached = authentication_args("sunrise_user", AuthMethod::Qr, false);
        assert!(cached.iter().any(|argument| argument == "-username"));
        assert!(!cached.iter().any(|argument| argument == "-qr"));
    }

    #[test]
    fn two_factor_authentication_requests_a_code_without_qr() {
        let two_factor = authentication_args("sunrise_user", AuthMethod::TwoFactor, true);
        assert!(two_factor.iter().any(|argument| argument == "-username"));
        assert!(two_factor.iter().any(|argument| argument == "-no-mobile"));
        assert!(!two_factor.iter().any(|argument| argument == "-qr"));
    }

    #[test]
    fn terminal_decoder_preserves_windows_qr_blocks() {
        let mut decoder = TerminalDecoder::default();
        assert_eq!(decoder.push(&[b' ', 0xDB, 0xDB, b'\n']), " ██\n");
    }

    #[test]
    fn terminal_decoder_reassembles_split_utf8_characters() {
        let mut decoder = TerminalDecoder::default();
        assert_eq!(decoder.push(&[0xE2, 0x96]), "");
        assert_eq!(decoder.push(&[0x88]), "█");
    }

    #[test]
    fn qr_capture_emits_a_complete_square_matrix() {
        let mut capture = QrCapture::default();
        assert!(capture.push_line("      ").is_none());
        assert!(capture.push_line("  ██  ").is_none());
        assert_eq!(
            capture.push_line("      "),
            Some(vec!["      ".into(), "  ██  ".into(), "      ".into()])
        );
    }

    #[test]
    fn auth_scanner_emits_password_then_hyphenated_steam_guard_prompt() {
        let mut scanner = AuthPromptScanner::default();
        assert!(scanner.push("Enter account pass").is_empty());
        assert_eq!(
            scanner
                .push("word for \"sunrise\": ")
                .into_iter()
                .map(|prompt| prompt.0)
                .collect::<Vec<_>>(),
            vec!["password"]
        );
        assert_eq!(
            scanner
                .push(
                    "Connecting to Steam3... Done! STEAM GUARD! Please enter your 2-factor auth code from your authenticator app: ",
                )
                .into_iter()
                .map(|prompt| prompt.0)
                .collect::<Vec<_>>(),
            vec!["twoFactor"]
        );
    }

    #[test]
    fn auth_scanner_emits_repeated_code_prompts_for_retries() {
        let mut scanner = AuthPromptScanner::default();
        let first =
            scanner.push("Please enter your 2 factor auth code from your authenticator app: ");
        let retry = scanner.push(
            "Invalid code. Please enter your 2-factor auth code from your authenticator app: ",
        );
        assert_eq!(first[0].0, "twoFactor");
        assert_eq!(retry[0].0, "twoFactor");
    }

    #[test]
    fn auth_scanner_marks_a_rejected_code_on_the_next_prompt() {
        let mut scanner = AuthPromptScanner::default();
        let retry = scanner.push(
            "The previous 2-factor auth code you have provided is incorrect.\nSTEAM GUARD! Please enter your 2-factor auth code from your authenticator app: ",
        );
        assert_eq!(retry, vec![("twoFactor", super::CODE_REJECTED_MESSAGE)]);
        let next = scanner.push("STEAM GUARD! Please enter your 2-factor auth code: ");
        assert_ne!(next[0].1, super::CODE_REJECTED_MESSAGE);
    }

    #[test]
    fn auth_scanner_detects_the_steamkit_email_prompt() {
        let mut scanner = AuthPromptScanner::default();
        let prompts = scanner
            .push("STEAM GUARD! Please enter the auth code sent to the email at s***@gmail.com: ");
        assert_eq!(prompts[0].0, "emailCode");
    }

    #[test]
    fn auth_completion_scanner_detects_cached_login_across_chunks_once() {
        let mut scanner = AuthCompletionScanner::default();
        assert!(!scanner.push("Logging 'sunrise' into Steam3... Done! Using Steam3 sug"));
        assert!(scanner.push("gested CellID: 207"));
        assert!(!scanner.push("Got session token!"));
    }

    #[test]
    fn auth_completion_scanner_detects_post_steam_guard_output() {
        let mut scanner = AuthCompletionScanner::default();
        assert!(!scanner.push(
            "STEAM GUARD! Please enter your 2-factor auth code from your authenticator app: "
        ));
        assert!(!scanner.push("Done!\r\n"));
        assert!(scanner.push("Got 2329 licenses for account!\r\n"));
    }

    #[test]
    fn depot_percent_parser_reads_depotdownloader_file_lines() {
        assert_eq!(
            parse_depot_file(" 42.17% C:\\Games\\Project Sunrise\\destiny2.exe"),
            Some((42.17, "destiny2.exe"))
        );
        assert_eq!(parse_depot_file("Downloading depot 1085661"), None);
        assert_eq!(parse_depot_file("Retrying after 50% of a second"), None);
        assert_eq!(
            parse_validating_file(
                "Validating \\\\?\\C:\\Games\\Project Sunrise\\packages\\activity.pkg"
            ),
            Some("activity.pkg")
        );
    }

    #[test]
    fn manifest_parser_preserves_file_paths_with_spaces() {
        assert_eq!(
            parse_manifest_file(
                "123 4 0123456789abcdef0123456789abcdef01234567 0 packages/audio file.pkg"
            ),
            Some("packages/audio file.pkg".into())
        );
        assert_eq!(parse_manifest_file("not a manifest line"), None);
    }

    #[test]
    fn language_cleanup_only_removes_safe_manifest_files() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let packages = temporary.path().join("packages");
        std::fs::create_dir_all(&packages).expect("packages directory");
        let obsolete = packages.join("old.pkg");
        let retained = packages.join("shared.pkg");
        std::fs::write(&obsolete, b"old").expect("obsolete language file");
        std::fs::write(&retained, b"shared").expect("shared language file");
        let removed = remove_depot_files(
            temporary.path(),
            &["packages/old.pkg".into(), "packages/missing.pkg".into()],
        )
        .expect("safe cleanup");
        assert_eq!(removed, 1);
        assert!(!obsolete.exists());
        assert!(retained.exists());
        assert!(remove_depot_files(temporary.path(), &["../outside.pkg".into()]).is_err());
    }

    #[test]
    fn depot_progress_maps_local_percent_and_never_moves_backwards() {
        let mut progress = DepotProgress::new(12, 49);
        let validating = progress
            .observe_line("Validating C:\\Games\\first.pkg")
            .expect("validation should update the current file");
        assert_eq!(validating.percent, 12.0);
        assert_eq!(validating.message, "Validating first.pkg");

        let early = progress
            .observe_line(" 00.85% C:\\Games\\early.pkg")
            .expect("download should update progress");
        assert!((early.percent - 12.3145).abs() < f32::EPSILON);
        assert_eq!(early.message, "Downloading early.pkg");

        let later = progress
            .observe_line(" 50.00% C:\\Games\\second.pkg")
            .expect("download should update progress");
        assert_eq!(later.percent, 30.5);

        let delayed = progress
            .observe_line(" 49.00% C:\\Games\\delayed.pkg")
            .expect("file name should still update");
        assert_eq!(delayed.percent, 30.5);

        let complete = progress
            .observe_line("100.00% C:\\Games\\destiny2.exe")
            .expect("completion should update progress");
        assert_eq!(complete.percent, 49.0);
    }

    #[test]
    fn release_tags_become_safe_folder_names() {
        assert_eq!(
            sanitize_name("DepotDownloader 3.4/0"),
            "DepotDownloader_3.4_0"
        );
    }

    #[test]
    fn current_platform_has_a_depot_asset_when_supported() {
        if matches!(std::env::consts::OS, "windows" | "linux" | "macos")
            && matches!(std::env::consts::ARCH, "x86_64" | "aarch64")
        {
            assert!(depot_downloader_release().is_ok());
        }
    }
}
