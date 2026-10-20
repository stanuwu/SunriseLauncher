//! CrossOver launch support for macOS.
//!
//! Sunrise is a Windows DLL inside a Windows game, so on a Mac both run under CrossOver's Wine
//! with Rosetta 2. The launcher's job is small: find CrossOver, keep one bottle for the game
//! with the right graphics backend, and start `destiny2.exe` inside it. Everything that runs a
//! process is macOS-only; the parsing and argument building is plain Rust so the tests run on
//! every platform the launcher is built on.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::{AppError, AppResult};
use crate::models::Preferences;

/// Bottle the launcher creates when the user has not named one.
pub const DEFAULT_BOTTLE: &str = "Sunrise";
/// Graphics backends CrossOver 26 can run this game with. DXVK is left out: its Direct3D 11
/// module does not initialise on MoltenVK for this game and Wine falls back to a renderer that
/// never presents a frame.
pub const GRAPHICS_BACKENDS: [&str; 2] = ["d3dmetal", "dxmt"];
/// The default backend. D3DMetal was the one that played through a full session.
pub const DEFAULT_BACKEND: &str = "d3dmetal";
/// Bottle template: 64-bit Windows 10, which is what the game build expects.
const BOTTLE_TEMPLATE: &str = "win10_64";
/// Where CrossOver keeps its private tools, relative to the app bundle.
const CROSSOVER_BIN: &str = "Contents/SharedSupport/CrossOver/bin";
/// Longest bottle name accepted. CrossOver uses it as a folder name.
const MAX_BOTTLE_NAME: usize = 64;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CrossOverStatus {
    /// True only on macOS, where launching through CrossOver applies at all.
    pub supported: bool,
    pub installed: bool,
    pub app_path: Option<String>,
    pub version: Option<String>,
    pub bottle_name: String,
    pub bottle_path: String,
    pub bottle_exists: bool,
    /// Backend recorded in the bottle, when the bottle exists.
    pub graphics_backend: Option<String>,
    /// Whether a launch can start right now.
    pub ready: bool,
    /// A blocking problem to show the user, if any.
    pub error: Option<String>,
}

impl CrossOverStatus {
    pub fn unsupported() -> Self {
        Self {
            supported: false,
            installed: false,
            app_path: None,
            version: None,
            bottle_name: DEFAULT_BOTTLE.into(),
            bottle_path: String::new(),
            bottle_exists: false,
            graphics_backend: None,
            ready: false,
            error: None,
        }
    }
}

/// Folders a CrossOver bundle can live in, in the order they are tried.
pub fn app_candidates(home: &Path) -> [PathBuf; 2] {
    [
        PathBuf::from("/Applications/CrossOver.app"),
        home.join("Applications").join("CrossOver.app"),
    ]
}

/// CrossOver's private bottle folder for one user.
pub fn bottles_dir(home: &Path) -> PathBuf {
    home.join("Library")
        .join("Application Support")
        .join("CrossOver")
        .join("Bottles")
}

/// Checks a bottle name the way CrossOver will use it: as one folder name.
pub fn sanitize_bottle_name(name: &str) -> AppResult<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Ok(DEFAULT_BOTTLE.into());
    }
    if trimmed.len() > MAX_BOTTLE_NAME {
        return Err(AppError::message(format!(
            "The bottle name is too long; use at most {MAX_BOTTLE_NAME} characters."
        )));
    }
    if trimmed.starts_with('.')
        || trimmed
            .chars()
            .any(|character| matches!(character, '/' | '\\' | ':' | '\0'))
    {
        return Err(AppError::message(
            "The bottle name cannot start with a dot or contain slashes or colons.",
        ));
    }
    Ok(trimmed.to_owned())
}

/// Picks a backend the launcher knows, falling back to the default for anything else.
pub fn resolve_backend(backend: &str) -> &'static str {
    GRAPHICS_BACKENDS
        .iter()
        .copied()
        .find(|known| known.eq_ignore_ascii_case(backend.trim()))
        .unwrap_or(DEFAULT_BACKEND)
}

/// Reads CFBundleShortVersionString from an Info.plist converted to XML.
pub fn parse_bundle_version(plist_xml: &str) -> Option<String> {
    let key = "<key>CFBundleShortVersionString</key>";
    let after_key = &plist_xml[plist_xml.find(key)? + key.len()..];
    let start = after_key.find("<string>")? + "<string>".len();
    let end = after_key[start..].find("</string>")? + start;
    let version = after_key[start..end].trim();
    (!version.is_empty()).then(|| version.to_owned())
}

/// Reads CX_GRAPHICS_BACKEND from a bottle's cxbottle.conf, which is an INI-style file.
pub fn parse_graphics_backend(cxbottle_conf: &str) -> Option<String> {
    cxbottle_conf
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with(';'))
        .find_map(|line| {
            let (key, value) = line.split_once('=')?;
            if key.trim().trim_matches('"') != "CX_GRAPHICS_BACKEND" {
                return None;
            }
            let value = value.trim().trim_matches('"');
            (!value.is_empty()).then(|| value.to_owned())
        })
}

/// Arguments for `cxbottle` that create the game's bottle with its backend already chosen.
pub fn create_bottle_args(bottle: &str, backend: &str) -> Vec<String> {
    vec![
        "--create".into(),
        "--bottle".into(),
        bottle.into(),
        "--template".into(),
        BOTTLE_TEMPLATE.into(),
        "--description".into(),
        "Project Sunrise".into(),
        "--param".into(),
        format!("EnvironmentVariables:CX_GRAPHICS_BACKEND={backend}"),
    ]
}

/// Arguments for CrossOver's `wine` that start the game inside the bottle.
/// The executable is passed as a plain native path: the launcher converts that to a Windows
/// path itself, whereas `--cx-app` looks for the file inside the bottle's C: drive.
/// `--no-wait` returns once the game is running instead of holding a process until it exits.
pub fn launch_args(bottle: &str, root: &Path, game_executable: &Path) -> Vec<String> {
    vec![
        "--bottle".into(),
        bottle.into(),
        "--no-update".into(),
        "--no-wait".into(),
        "--workdir".into(),
        root.to_string_lossy().into_owned(),
        game_executable.to_string_lossy().into_owned(),
    ]
}

/// Facts about the host that the status and the launch both need.
struct Host {
    app_path: PathBuf,
    home: PathBuf,
}

#[cfg(target_os = "macos")]
fn find_host() -> Option<Host> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    let app_path = app_candidates(&home)
        .into_iter()
        .find(|candidate| candidate.join(CROSSOVER_BIN).join("wine").is_file())?;
    Some(Host { app_path, home })
}

#[cfg(target_os = "macos")]
fn bundle_version(app_path: &Path) -> Option<String> {
    let output = std::process::Command::new("/usr/bin/plutil")
        .args(["-convert", "xml1", "-o", "-"])
        .arg(app_path.join("Contents").join("Info.plist"))
        .output()
        .ok()?;
    parse_bundle_version(&String::from_utf8_lossy(&output.stdout))
}

/// Reports whether CrossOver and the game's bottle are in place. Never fails: a problem is
/// carried in the status so the interface can show it.
#[cfg(target_os = "macos")]
pub fn status(preferences: &Preferences) -> CrossOverStatus {
    let bottle_name = match sanitize_bottle_name(&preferences.crossover_bottle) {
        Ok(name) => name,
        Err(error) => {
            return CrossOverStatus {
                supported: true,
                error: Some(error.to_string()),
                ..CrossOverStatus::unsupported()
            };
        }
    };
    let Some(host) = find_host() else {
        return CrossOverStatus {
            supported: true,
            bottle_name,
            error: Some(
                "CrossOver was not found in /Applications. Install CrossOver 26 or newer, then refresh."
                    .into(),
            ),
            ..CrossOverStatus::unsupported()
        };
    };
    let bottle_path = bottles_dir(&host.home).join(&bottle_name);
    let bottle_exists = bottle_path.join("cxbottle.conf").is_file();
    let graphics_backend = bottle_exists
        .then(|| std::fs::read_to_string(bottle_path.join("cxbottle.conf")).ok())
        .flatten()
        .and_then(|conf| parse_graphics_backend(&conf));
    CrossOverStatus {
        supported: true,
        installed: true,
        app_path: Some(host.app_path.to_string_lossy().into_owned()),
        version: bundle_version(&host.app_path),
        bottle_name,
        bottle_path: bottle_path.to_string_lossy().into_owned(),
        bottle_exists,
        graphics_backend,
        ready: bottle_exists,
        error: None,
    }
}

#[cfg(not(target_os = "macos"))]
pub fn status(_preferences: &Preferences) -> CrossOverStatus {
    CrossOverStatus::unsupported()
}

/// Creates the game's bottle when it is missing. Creation runs CrossOver's own tool and takes a
/// few seconds; an existing bottle is left exactly as it is.
#[cfg(target_os = "macos")]
pub fn prepare(preferences: &Preferences) -> AppResult<CrossOverStatus> {
    let current = status(preferences);
    if let Some(error) = current.error.as_deref() {
        return Err(AppError::message(error));
    }
    if current.bottle_exists {
        return Ok(current);
    }
    let host = find_host().ok_or_else(|| AppError::message("CrossOver was not found."))?;
    let backend = resolve_backend(&preferences.crossover_backend);
    let output = std::process::Command::new(host.app_path.join(CROSSOVER_BIN).join("cxbottle"))
        .args(create_bottle_args(&current.bottle_name, backend))
        .output()
        .map_err(|error| AppError::io("CrossOver's bottle tool could not be started", error))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        return Err(AppError::message(format!(
            "CrossOver could not create the bottle: {}",
            detail.trim()
        )));
    }
    Ok(status(preferences))
}

#[cfg(not(target_os = "macos"))]
pub fn prepare(_preferences: &Preferences) -> AppResult<CrossOverStatus> {
    Err(AppError::message(
        "CrossOver launching is only available on macOS.",
    ))
}

/// Starts the game inside the bottle. Returns once CrossOver has taken the process.
#[cfg(target_os = "macos")]
pub fn launch(root: &Path, preferences: &Preferences) -> AppResult<()> {
    let current = status(preferences);
    if let Some(error) = current.error.as_deref() {
        return Err(AppError::message(error));
    }
    if !current.bottle_exists {
        return Err(AppError::message(
            "The CrossOver bottle has not been prepared yet. Open Settings and choose Prepare bottle.",
        ));
    }
    let executable = root.join(crate::models::GAME_EXECUTABLE);
    if !executable.is_file() {
        return Err(AppError::message(
            "destiny2.exe was not found in the installation folder.",
        ));
    }
    let host = find_host().ok_or_else(|| AppError::message("CrossOver was not found."))?;
    std::process::Command::new(host.app_path.join(CROSSOVER_BIN).join("wine"))
        .args(launch_args(&current.bottle_name, root, &executable))
        .current_dir(root)
        .spawn()
        .map_err(|error| {
            AppError::io("Destiny 2 could not be launched through CrossOver", error)
        })?;
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn launch(_root: &Path, _preferences: &Preferences) -> AppResult<()> {
    Err(AppError::message(
        "CrossOver launching is only available on macOS.",
    ))
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{
        DEFAULT_BACKEND, DEFAULT_BOTTLE, app_candidates, bottles_dir, create_bottle_args,
        launch_args, parse_bundle_version, parse_graphics_backend, resolve_backend,
        sanitize_bottle_name,
    };

    #[test]
    fn bottle_names_are_single_folder_names() {
        assert_eq!(sanitize_bottle_name("  ").unwrap(), DEFAULT_BOTTLE);
        assert_eq!(
            sanitize_bottle_name(" Sunrise Test ").unwrap(),
            "Sunrise Test"
        );
        assert!(sanitize_bottle_name("../escape").is_err());
        assert!(sanitize_bottle_name("a/b").is_err());
        assert!(sanitize_bottle_name("a:b").is_err());
        assert!(sanitize_bottle_name(".hidden").is_err());
        assert!(sanitize_bottle_name(&"x".repeat(65)).is_err());
    }

    #[test]
    fn unknown_backends_fall_back_to_the_default() {
        assert_eq!(resolve_backend("DXMT"), "dxmt");
        assert_eq!(resolve_backend(" d3dmetal "), "d3dmetal");
        assert_eq!(resolve_backend("dxvk"), DEFAULT_BACKEND);
        assert_eq!(resolve_backend(""), DEFAULT_BACKEND);
    }

    #[test]
    fn reads_the_bundle_version_from_plist_xml() {
        let plist = r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleName</key><string>CrossOver</string>
<key>CFBundleShortVersionString</key>
<string>26.3</string>
<key>CFBundleVersion</key><string>26.3.0.40010</string>
</dict></plist>"#;
        assert_eq!(parse_bundle_version(plist).as_deref(), Some("26.3"));
        assert_eq!(parse_bundle_version("<plist/>"), None);
    }

    #[test]
    fn reads_the_graphics_backend_from_the_bottle_config() {
        let conf = "[Bottle]\n\"Template\" = \"win10_64\"\n\n[EnvironmentVariables]\n;;\"PROMPT\" = \"$p$g\"\n\"CX_GRAPHICS_BACKEND\" = \"d3dmetal\"\n\"WINEMSYNC\" = \"1\"\n";
        assert_eq!(parse_graphics_backend(conf).as_deref(), Some("d3dmetal"));
        assert_eq!(parse_graphics_backend("[Bottle]\n"), None);
        assert_eq!(
            parse_graphics_backend(";;\"CX_GRAPHICS_BACKEND\" = \"dxvk\"\n"),
            None
        );
    }

    #[test]
    fn bottle_creation_carries_the_backend_into_the_bottle() {
        let args = create_bottle_args("Sunrise", "dxmt");
        assert_eq!(args[..3], ["--create", "--bottle", "Sunrise"]);
        assert!(args.contains(&"win10_64".to_owned()));
        assert_eq!(
            args.last().map(String::as_str),
            Some("EnvironmentVariables:CX_GRAPHICS_BACKEND=dxmt")
        );
    }

    #[test]
    fn launch_arguments_pass_a_native_path_and_do_not_wait() {
        let root = Path::new("/Users/guardian/Games/Sunrise");
        let executable = root.join("destiny2.exe");
        let args = launch_args("Sunrise", root, &executable);
        // The join uses the host's separator, so the test compares against the same join.
        assert_eq!(
            args,
            [
                "--bottle",
                "Sunrise",
                "--no-update",
                "--no-wait",
                "--workdir",
                &*root.to_string_lossy(),
                &*executable.to_string_lossy()
            ]
        );
        // --cx-app would make CrossOver look inside the bottle's C: drive instead.
        assert!(!args.iter().any(|argument| argument == "--cx-app"));
    }

    #[test]
    fn crossover_paths_follow_the_home_folder() {
        let home = PathBuf::from("/Users/guardian");
        assert_eq!(
            app_candidates(&home)[1],
            PathBuf::from("/Users/guardian/Applications/CrossOver.app")
        );
        assert_eq!(
            bottles_dir(&home),
            PathBuf::from("/Users/guardian/Library/Application Support/CrossOver/Bottles")
        );
    }
}
