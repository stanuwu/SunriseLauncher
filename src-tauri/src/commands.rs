use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use tauri::Manager;
use tauri::{AppHandle, State, ipc::Channel};
use tokio::io::AsyncWriteExt;
use tokio::process::ChildStdin;
use tokio::sync::Mutex as AsyncMutex;
use tokio_util::sync::CancellationToken;

use crate::crossover;
use crate::error::{AppError, AppResult};
use crate::github::GitHubClient;
use crate::installer;
use crate::models::{
    AppSnapshot, InstallationSnapshot, OperationEvent, OperationRequest, OperationResult,
    Preferences, ProtonRelease, ProtonSource, ProtonStatus, UmuSource, current_platform,
    resolve_language,
};
use crate::proton;
use crate::storage;

#[derive(Default)]
pub struct OperationState {
    active: AtomicBool,
    proton_active: AtomicBool,
    cancel: Mutex<Option<CancellationToken>>,
    terminal_input: Arc<AsyncMutex<Option<ChildStdin>>>,
}

impl OperationState {
    pub fn cancel_active(&self) -> bool {
        let guard = self.cancel.lock().expect("operation cancel mutex poisoned");
        if let Some(cancel) = guard.as_ref() {
            cancel.cancel();
            true
        } else {
            false
        }
    }
}

/// Claims the single-operation slot and releases it on drop, so an early
/// return or a panic can never leave the launcher permanently "busy".
struct ActiveOperation<'a> {
    state: &'a OperationState,
    proton: bool,
}

impl<'a> ActiveOperation<'a> {
    fn begin(state: &'a OperationState, proton: bool) -> AppResult<Self> {
        if state
            .active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(AppError::message(
                "Another installer operation is already running.",
            ));
        }
        if proton {
            state.proton_active.store(true, Ordering::Release);
        }
        Ok(Self { state, proton })
    }
}

impl Drop for ActiveOperation<'_> {
    fn drop(&mut self) {
        if self.proton {
            self.state.proton_active.store(false, Ordering::Release);
        }
        self.state.active.store(false, Ordering::Release);
    }
}

impl Drop for OperationState {
    fn drop(&mut self) {
        if let Ok(guard) = self.cancel.lock()
            && let Some(cancel) = guard.as_ref()
        {
            cancel.cancel();
        }
    }
}

#[tauri::command]
pub async fn get_app_snapshot(app: AppHandle) -> AppResult<AppSnapshot> {
    let mut preferences = storage::load_preferences(&app).await?;
    preferences.steam_language = resolve_language(&preferences.steam_language)
        .steam_language
        .into();
    let launcher_directory = storage::default_install_directory()?;
    let launcher_is_in_installation =
        storage::is_existing_installation_directory(Path::new(&launcher_directory));
    let detected_directory =
        if launcher_is_in_installation || preferences.install_directory.trim().is_empty() {
            Some(launcher_directory)
        } else {
            None
        };
    if let Some(detected_directory) = detected_directory
        && preferences.install_directory != detected_directory
    {
        preferences.install_directory = detected_directory;
        storage::save_preferences(&app, &preferences).await?;
    }
    let installation = storage::inspect_installation(&preferences.install_directory).await?;
    // Repair and Install use the selected language, so it starts as the installed one.
    if let Some(installed) = installation.steam_language.as_ref()
        && preferences.steam_language != *installed
    {
        preferences.steam_language = installed.clone();
        storage::save_preferences(&app, &preferences).await?;
    }
    let release_result = async {
        GitHubClient::new()?
            .latest_release("stanuwu", "Sunrise", "steam_api64.dll")
            .await
    }
    .await;
    let (latest_release, release_error) = match release_result {
        Ok(release) => (Some(release), None),
        Err(error) => (None, Some(error.to_string())),
    };
    let update_available = latest_release
        .as_ref()
        .is_some_and(|release| installation.update_available(release));
    let mut platform = current_platform();
    let crossover = crossover_status_blocking(preferences.clone()).await?;
    if let Some(status) = crossover.as_ref() {
        platform.can_launch = status.ready;
    }
    let proton = proton_status(&app, &preferences);
    Ok(AppSnapshot {
        platform,
        preferences,
        installation,
        latest_release,
        update_available,
        release_error,
        crossover,
        proton,
    })
}

#[cfg(target_os = "macos")]
fn crossover_status(preferences: &Preferences) -> Option<crossover::CrossOverStatus> {
    Some(crossover::status(preferences))
}

#[cfg(not(target_os = "macos"))]
fn crossover_status(_preferences: &Preferences) -> Option<crossover::CrossOverStatus> {
    None
}

/// The status reads the bottle folder and asks plutil for CrossOver's version, so it runs on
/// a blocking thread rather than inside the async command itself.
async fn crossover_status_blocking(
    preferences: Preferences,
) -> AppResult<Option<crossover::CrossOverStatus>> {
    tauri::async_runtime::spawn_blocking(move || crossover_status(&preferences))
        .await
        .map_err(|error| AppError::message(format!("CrossOver status was interrupted: {error}")))
}

#[tauri::command]
pub async fn get_crossover_status(
    preferences: Preferences,
) -> AppResult<Option<crossover::CrossOverStatus>> {
    crossover_status_blocking(preferences).await
}

/// Creates the game's CrossOver bottle. Runs on a blocking thread because bottle creation
/// takes a few seconds and must not stall the interface.
#[tauri::command]
pub async fn prepare_crossover(
    app: AppHandle,
    preferences: Preferences,
) -> AppResult<crossover::CrossOverStatus> {
    storage::save_preferences(&app, &preferences).await?;
    tauri::async_runtime::spawn_blocking(move || crossover::prepare(&preferences))
        .await
        .map_err(|error| {
            AppError::message(format!("Bottle preparation was interrupted: {error}"))
        })?
}

#[cfg(target_os = "linux")]
fn proton_status(app: &AppHandle, preferences: &Preferences) -> Option<ProtonStatus> {
    match proton::status(app, preferences) {
        Ok(status) => Some(status),
        Err(error) => Some(ProtonStatus {
            supported: true,
            umu_available: false,
            umu_version: None,
            umu_source: None,
            selected_tag: None,
            proton_path: None,
            installed_tags: Vec::new(),
            prefix_path: String::new(),
            prefix_initialized: false,
            ready: false,
            error: Some(error.to_string()),
        }),
    }
}

#[cfg(not(target_os = "linux"))]
fn proton_status(_app: &AppHandle, _preferences: &Preferences) -> Option<ProtonStatus> {
    None
}

#[tauri::command]
pub async fn get_proton_status(
    app: AppHandle,
    preferences: Preferences,
) -> AppResult<Option<ProtonStatus>> {
    Ok(proton_status(&app, &preferences))
}

#[tauri::command]
pub async fn list_proton_releases(app: AppHandle, refresh: bool) -> AppResult<Vec<ProtonRelease>> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (app, refresh);
        return Err(AppError::message("Proton is only available on Linux."));
    }
    #[cfg(target_os = "linux")]
    {
        if !refresh && let Some(cached) = proton::load_cached_catalog(&app).await? {
            return Ok(cached);
        }
        proton::refresh_catalog(&app, &GitHubClient::new()?).await
    }
}

#[tauri::command]
pub async fn inspect_installation(install_directory: String) -> AppResult<InstallationSnapshot> {
    storage::inspect_installation(&install_directory).await
}

#[tauri::command]
pub async fn install_proton(
    app: AppHandle,
    state: State<'_, OperationState>,
    tag: String,
    on_event: Channel<OperationEvent>,
) -> AppResult<ProtonStatus> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (app, state, tag, on_event);
        return Err(AppError::message("Proton is only available on Linux."));
    }
    #[cfg(target_os = "linux")]
    {
        let _active = ActiveOperation::begin(state.inner(), true)?;
        let cancel = CancellationToken::new();
        *state
            .cancel
            .lock()
            .expect("operation cancel mutex poisoned") = Some(cancel.clone());

        let result = async {
            let github = GitHubClient::new()?;
            let mut preferences = storage::load_preferences(&app).await?;
            let mut releases = proton::load_cached_catalog(&app).await?.unwrap_or_default();
            if !releases.iter().any(|release| release.tag == tag) {
                releases = proton::refresh_catalog(&app, &github).await?;
            }
            let release = releases
                .iter()
                .find(|release| release.tag == tag)
                .ok_or_else(|| {
                    AppError::message(format!("GE-Proton {tag} is not in the release list."))
                })?;
            if preferences.umu_source == UmuSource::Managed {
                proton::install_umu(&app, &github, &on_event, &cancel).await?;
            }
            proton::install_proton(&app, &github, release, &on_event, &cancel).await?;
            preferences.proton_source = ProtonSource::Managed;
            preferences.proton_version = tag.clone();
            storage::save_preferences(&app, &preferences).await?;
            proton::status(&app, &preferences)
        }
        .await;

        *state
            .cancel
            .lock()
            .expect("operation cancel mutex poisoned") = None;
        result
    }
}

#[tauri::command]
pub async fn remove_proton(
    app: AppHandle,
    state: State<'_, OperationState>,
    tag: String,
) -> AppResult<ProtonStatus> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (app, state, tag);
        return Err(AppError::message("Proton is only available on Linux."));
    }
    #[cfg(target_os = "linux")]
    {
        let _active = ActiveOperation::begin(state.inner(), true)?;
        proton::remove_proton(&app, &tag)?;
        let preferences = storage::load_preferences(&app).await?;
        proton::status(&app, &preferences)
    }
}

#[tauri::command]
pub async fn save_preferences(app: AppHandle, mut preferences: Preferences) -> AppResult<()> {
    preferences.steam_language = resolve_language(&preferences.steam_language)
        .steam_language
        .into();
    storage::save_preferences(&app, &preferences).await
}

#[tauri::command]
pub async fn run_operation(
    app: AppHandle,
    state: State<'_, OperationState>,
    request: OperationRequest,
    on_event: Channel<OperationEvent>,
) -> AppResult<OperationResult> {
    let _active = ActiveOperation::begin(state.inner(), false)?;

    let cancel = CancellationToken::new();
    *state
        .cancel
        .lock()
        .expect("operation cancel mutex poisoned") = Some(cancel.clone());
    let result = async {
        // Only the fields the request carries change; platform settings keep their values.
        let mut preferences = storage::load_preferences(&app).await?;
        preferences.install_directory = request.install_directory.clone();
        preferences.steam_username = request.steam_username.clone();
        preferences.steam_language = resolve_language(&request.steam_language)
            .steam_language
            .into();
        preferences.auth_method = request.auth_method;
        storage::save_preferences(&app, &preferences).await?;
        installer::run(
            &app,
            request,
            on_event,
            cancel,
            state.terminal_input.clone(),
        )
        .await
    }
    .await;

    *state.terminal_input.lock().await = None;
    *state
        .cancel
        .lock()
        .expect("operation cancel mutex poisoned") = None;
    result
}

#[tauri::command]
pub async fn send_terminal_input(
    state: State<'_, OperationState>,
    input: String,
) -> AppResult<bool> {
    if input.len() > 512
        || input
            .chars()
            .any(|character| matches!(character, '\r' | '\n'))
    {
        return Err(AppError::message("The console response is not valid."));
    }
    let mut guard = state.terminal_input.lock().await;
    let Some(stdin) = guard.as_mut() else {
        return Ok(false);
    };
    stdin
        .write_all(format!("{input}\n").as_bytes())
        .await
        .map_err(|error| AppError::io("Could not send input to DepotDownloader", error))?;
    stdin
        .flush()
        .await
        .map_err(|error| AppError::io("Could not send input to DepotDownloader", error))?;
    Ok(true)
}

#[tauri::command]
pub async fn initialize_proton_prefix(
    app: AppHandle,
    state: State<'_, OperationState>,
    on_event: Channel<OperationEvent>,
) -> AppResult<ProtonStatus> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (app, state, on_event);
        return Err(AppError::message("Proton is only available on Linux."));
    }
    #[cfg(target_os = "linux")]
    {
        let _active = ActiveOperation::begin(state.inner(), true)?;
        let cancel = CancellationToken::new();
        *state
            .cancel
            .lock()
            .expect("operation cancel mutex poisoned") = Some(cancel.clone());

        let result = async {
            let preferences = storage::load_preferences(&app).await?;
            proton::initialize_prefix(&app, &preferences, &on_event, &cancel).await?;
            proton::status(&app, &preferences)
        }
        .await;

        *state
            .cancel
            .lock()
            .expect("operation cancel mutex poisoned") = None;
        result
    }
}

#[tauri::command]
pub fn cancel_operation(state: State<'_, OperationState>) -> bool {
    state.cancel_active()
}

#[tauri::command]
pub fn proton_operation_active(state: State<'_, OperationState>) -> bool {
    state.proton_active.load(Ordering::Acquire)
}

#[tauri::command]
pub async fn launch_game(app: AppHandle, install_directory: String) -> AppResult<()> {
    #[cfg(target_os = "linux")]
    {
        let preferences = storage::load_preferences(&app).await?;
        proton::launch(&app, &preferences, &install_directory)
    }
    #[cfg(windows)]
    {
        let root = std::path::PathBuf::from(install_directory.trim());
        launch_windows(&app, &root)
    }
    #[cfg(target_os = "macos")]
    {
        let root = std::path::PathBuf::from(install_directory.trim());
        let preferences = storage::load_preferences(&app).await?;
        tauri::async_runtime::spawn_blocking(move || crossover::launch(&root, &preferences))
            .await
            .map_err(|error| AppError::message(format!("The launch was interrupted: {error}")))?
    }
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    {
        let _ = (app, install_directory);
        Err(AppError::message(
            "Launching is supported on Windows, macOS and Linux only.",
        ))
    }
}

#[tauri::command]
pub fn game_status(app: AppHandle) -> AppResult<bool> {
    Ok(app.state::<proton::ProcessTracker>().has_running())
}

#[cfg(windows)]
fn launch_windows(app: &AppHandle, root: &Path) -> AppResult<()> {
    let executable = root.join(crate::models::GAME_EXECUTABLE);
    if !executable.is_file() {
        return Err(AppError::message(
            "destiny2.exe was not found in the installation folder.",
        ));
    }
    let child = std::process::Command::new(executable)
        .current_dir(root)
        .spawn()
        .map_err(|error| AppError::io("Destiny 2 could not be launched", error))?;
    app.state::<proton::ProcessTracker>()
        .processes
        .try_lock()
        .map_err(|_error| AppError::message("Failed to lock processes"))?
        .push(child);
    Ok(())
}
