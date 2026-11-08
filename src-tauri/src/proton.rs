use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Child;

use tauri::ipc::Channel;
use tauri::{AppHandle, Manager};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::error::{AppError, AppResult};
use crate::github::GitHubClient;
use crate::installer;
use crate::models::{
    OperationEvent, Preferences, ProtonSource, ProtonStatus, ReleaseInfo, UmuSource,
};
use crate::storage;

pub const PROTON_TAG: &str = "GE-Proton11-11";

// Architecture, asset name, size in bytes and SHA-256. Bump them together with PROTON_TAG.
const PROTON_ASSETS: [(&str, &str, u64, &str); 2] = [
    (
        "x86_64",
        "GE-Proton11-11-x86_64.tar.gz",
        564_789_552,
        "aa85986cada5709aa8bbfa8add7f3bfb925cbe15710c98995898d308ff2ca3c1",
    ),
    (
        "aarch64",
        "GE-Proton11-11-aarch64.tar.gz",
        648_071_663,
        "c70eb3cc1c48a4cf1719d643c86fc069060fc6a1fecb490ec03295f2f7541ad1",
    ),
];

const UMU_TAG: &str = "1.4.4";

// Asset name, size in bytes and SHA-256 of the zipapp. Bump them together with UMU_TAG.
const UMU_ASSET: (&str, u64, &str) = (
    "umu-launcher-1.4.4-zipapp.tar",
    430_080,
    "eb590691841f7fad3fc3ad8fd5db4ccb87849fe7948e62b28ece7a4ee48cc851",
);

const LOG_FILE: &str = "umu.log";

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

fn proton_release() -> AppResult<ReleaseInfo> {
    let arch = std::env::consts::ARCH;
    let (_, asset_name, size, sha256) = PROTON_ASSETS
        .into_iter()
        .find(|asset| asset.0 == arch)
        .ok_or_else(|| {
            AppError::message(format!(
                "GE-Proton does not publish a build for the {arch} architecture."
            ))
        })?;
    Ok(ReleaseInfo {
        tag: PROTON_TAG.into(),
        asset_name: asset_name.into(),
        download_url: format!(
            "https://github.com/GloriousEggroll/proton-ge-custom/releases/download/{PROTON_TAG}/{asset_name}"
        ),
        size,
        digest: Some(format!("sha256:{sha256}")),
    })
}

fn umu_release() -> ReleaseInfo {
    let (asset_name, size, sha256) = UMU_ASSET;
    ReleaseInfo {
        tag: UMU_TAG.into(),
        asset_name: asset_name.into(),
        download_url: format!(
            "https://github.com/Open-Wine-Components/umu-launcher/releases/download/{UMU_TAG}/{asset_name}"
        ),
        size,
        digest: Some(format!("sha256:{sha256}")),
    }
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

fn managed_proton_path(app: &AppHandle) -> AppResult<PathBuf> {
    Ok(proton_versions_dir(app)?.join(PROTON_TAG))
}

fn managed_umu_path(app: &AppHandle) -> AppResult<PathBuf> {
    Ok(umu_versions_dir(app)?
        .join(UMU_TAG)
        .join("umu")
        .join("umu-run"))
}

pub fn find_umu_run(
    app: &AppHandle,
    preferences: &Preferences,
) -> AppResult<Option<(PathBuf, String, String)>> {
    if preferences.umu_source == UmuSource::System {
        return Ok(find_on_path("umu-run").map(|path| (path, "system".into(), "system".into())));
    }

    let managed = managed_umu_path(app)?;
    if managed.is_file() {
        return Ok(Some((managed, UMU_TAG.into(), "managed".into())));
    }

    if let Some(path) = find_on_path("umu-run") {
        return Ok(Some((path, "system".into(), "system".into())));
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
            let path = managed_proton_path(app)?;
            Ok(looks_like_proton(&path).then(|| (path, PROTON_TAG.to_owned())))
        }
    }
}

pub fn status(app: &AppHandle, preferences: &Preferences) -> AppResult<ProtonStatus> {
    let prefix = prefix_path(app, preferences)?;
    let prefix_initialized = prefix.join("drive_c").is_dir();
    let managed_installed = looks_like_proton(&managed_proton_path(app)?);

    let (umu_available, umu_version, umu_source) = match find_umu_run(app, preferences) {
        Ok(Some((_, version, source))) => (true, Some(version), Some(source)),
        _ => (false, None, None),
    };

    let (proton_path, selected_tag, error) = match resolve_proton_path(app, preferences) {
        Ok(Some((path, tag))) => (Some(path.to_string_lossy().into_owned()), Some(tag), None),
        Ok(None) => (
            None,
            None,
            Some("GE-Proton is not installed yet. Download it to continue.".into()),
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
        managed_tag: PROTON_TAG.into(),
        managed_installed,
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
    on_event: &Channel<OperationEvent>,
    cancel: &CancellationToken,
) -> AppResult<()> {
    let release = proton_release()?;
    let versions = proton_versions_dir(app)?;
    tokio::fs::create_dir_all(&versions)
        .await
        .map_err(|error| AppError::io("Could not create the Proton folder", error))?;
    let version_directory = versions.join(PROTON_TAG);
    if looks_like_proton(&version_directory) {
        return Ok(());
    }

    cancel_check(cancel)?;
    let temporary = tempfile::Builder::new()
        .prefix(".ge-proton-")
        .tempdir_in(&versions)
        .map_err(|error| AppError::io("Could not create a temporary Proton folder", error))?;
    let archive = temporary.path().join(&release.asset_name);
    github
        .download_large(&release, &archive, on_event, "proton", 15, 70)
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

/** Removes every managed GE-Proton build, including ones an earlier launcher installed. */
pub fn remove_proton(app: &AppHandle) -> AppResult<()> {
    let directory = proton_versions_dir(app)?;
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
) -> AppResult<()> {
    let release = umu_release();
    let versions = umu_versions_dir(app)?;
    tokio::fs::create_dir_all(&versions)
        .await
        .map_err(|error| AppError::io("Could not create the umu folder", error))?;
    let version_directory = versions.join(UMU_TAG);
    if managed_umu_path(app)?.is_file() {
        return Ok(());
    }

    cancel_check(cancel)?;
    let temporary = tempfile::Builder::new()
        .prefix(".umu-")
        .tempdir_in(&versions)
        .map_err(|error| AppError::io("Could not create a temporary umu folder", error))?;
    let archive = temporary.path().join(&release.asset_name);
    github
        .download(&release, &archive, on_event, "proton", 5, 15)
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
    Ok(())
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

/** Unpacks inside `destination` only; setuid, setgid and sticky bits are dropped. */
fn extract_tar<R: Read>(reader: R, destination: &Path) -> AppResult<()> {
    let mut archive = tar::Archive::new(reader);
    archive.set_preserve_permissions(false);
    archive
        .unpack(destination)
        .map_err(|error| AppError::message(format!("The archive could not be extracted: {error}")))
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use super::{Mutex, ProcessTracker};
    use super::{
        PROTON_ASSETS, PROTON_TAG, extract_tar_gz, proton_release, single_root, umu_release,
    };

    #[test]
    fn pinned_releases_always_carry_a_sha256() {
        for (_, asset_name, size, sha256) in PROTON_ASSETS {
            assert!(asset_name.starts_with(PROTON_TAG));
            assert!(size > 0);
            assert_eq!(sha256.len(), 64);
            assert!(sha256.bytes().all(|byte| byte.is_ascii_hexdigit()));
        }
        let umu = umu_release();
        assert!(
            umu.digest
                .as_deref()
                .is_some_and(|digest| digest.len() == 71)
        );
        if let Ok(proton) = proton_release() {
            assert!(proton.download_url.ends_with(&proton.asset_name));
            assert!(proton.digest.is_some());
        }
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
    fn extract_tar_skips_escaping_paths_and_drops_setuid() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let mut bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut bytes);
            for name in [&b"../escaped"[..], &b"root/tool"[..]] {
                let mut header = tar::Header::new_old();
                header.as_old_mut().name[..name.len()].copy_from_slice(name);
                header.set_size(1);
                header.set_mode(0o4755);
                header.set_cksum();
                builder.append(&header, &b"x"[..]).expect("append entry");
            }
            builder.finish().expect("finish tar");
        }

        let destination = temporary.path().join("out");
        super::extract_tar(&bytes[..], &destination).expect("extract");
        assert!(!temporary.path().join("escaped").exists());
        let tool = destination.join("root").join("tool");
        assert!(tool.is_file());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&tool)
                .expect("metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o7000, 0);
        }
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
