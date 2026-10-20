use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub const STEAM_APP_ID: u32 = 1_085_660;
pub const GAME_EXECUTABLE: &str = "destiny2.exe";
pub const MOD_RELATIVE_PATH: [&str; 3] = ["bin", "x64", "steam_api64.dll"];
pub const FRESH_INSTALL_BYTES: u64 = 110 * 1024 * 1024 * 1024;
pub const REPAIR_BYTES: u64 = 5 * 1024 * 1024 * 1024;
pub const UPDATE_BYTES: u64 = 256 * 1024 * 1024;
pub const BASE_DEPOT: DepotSpec = DepotSpec::new(1_085_661, 7_180_122_903_232_116_872);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DepotSpec {
    pub depot_id: u32,
    pub manifest_id: u64,
}

impl DepotSpec {
    pub const fn new(depot_id: u32, manifest_id: u64) -> Self {
        Self {
            depot_id,
            manifest_id,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LanguageSpec {
    pub display_name: &'static str,
    pub steam_language: &'static str,
    pub depot: DepotSpec,
}

impl LanguageSpec {
    pub const fn new(
        display_name: &'static str,
        steam_language: &'static str,
        depot_id: u32,
        manifest_id: u64,
    ) -> Self {
        Self {
            display_name,
            steam_language,
            depot: DepotSpec::new(depot_id, manifest_id),
        }
    }
}

pub const LANGUAGES: [LanguageSpec; 13] = [
    LanguageSpec::new("English", "english", 1_085_662, 2_210_332_166_360_342_287),
    LanguageSpec::new("French", "french", 1_085_663, 2_934_940_253_687_559_290),
    LanguageSpec::new("German", "german", 1_085_664, 2_207_989_571_290_186_153),
    LanguageSpec::new("Italian", "italian", 1_085_665, 6_668_232_053_215_128_229),
    LanguageSpec::new("Japanese", "japanese", 1_085_666, 7_430_022_397_683_116_838),
    LanguageSpec::new(
        "Portuguese (Brazil)",
        "brazilian",
        1_085_667,
        9_037_238_175_838_085_860,
    ),
    LanguageSpec::new(
        "Spanish (Spain)",
        "spanish",
        1_085_668,
        3_424_833_900_894_552_134,
    ),
    LanguageSpec::new("Russian", "russian", 1_085_669, 4_539_277_942_371_480_381),
    LanguageSpec::new("Polish", "polish", 1_085_670, 6_407_581_507_105_256_731),
    LanguageSpec::new(
        "Chinese (Simplified)",
        "schinese",
        1_085_671,
        4_397_663_774_546_719_308,
    ),
    LanguageSpec::new(
        "Chinese (Traditional)",
        "tchinese",
        1_085_672,
        3_906_738_704_604_711_877,
    ),
    LanguageSpec::new(
        "Spanish (Latin America)",
        "latam",
        1_085_673,
        4_773_170_998_099_699_561,
    ),
    LanguageSpec::new("Korean", "koreana", 1_085_674, 7_148_196_199_569_436_690),
];

pub fn resolve_language(steam_language: &str) -> &'static LanguageSpec {
    LANGUAGES
        .iter()
        .find(|language| {
            language
                .steam_language
                .eq_ignore_ascii_case(steam_language.trim())
        })
        .unwrap_or(&LANGUAGES[0])
}

pub fn depots_for(language: &LanguageSpec) -> [DepotSpec; 2] {
    [BASE_DEPOT, language.depot]
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preferences {
    pub install_directory: String,
    pub steam_username: String,
    #[serde(default = "default_steam_language")]
    pub steam_language: String,
    #[serde(default)]
    pub auth_method: AuthMethod,
    // CrossOver settings; only meaningful on macOS and ignored elsewhere.
    #[serde(default = "default_crossover_bottle")]
    pub crossover_bottle: String,
    #[serde(default = "default_crossover_backend")]
    pub crossover_backend: String,
    // Proton settings; only meaningful on Linux and ignored elsewhere.
    #[serde(default)]
    pub proton_version: String,
    #[serde(default)]
    pub proton_source: ProtonSource,
    #[serde(default)]
    pub proton_custom_path: String,
    #[serde(default)]
    pub umu_source: UmuSource,
    #[serde(default)]
    pub proton_prefix: String,
    #[serde(default)]
    pub launch_options: String,
    #[serde(default = "default_game_id")]
    pub game_id: String,
    #[serde(default)]
    pub env_overrides: String,
    #[serde(default)]
    pub proton_debug_logging: bool,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            install_directory: String::new(),
            steam_username: String::new(),
            steam_language: default_steam_language(),
            auth_method: AuthMethod::default(),
            crossover_bottle: default_crossover_bottle(),
            crossover_backend: default_crossover_backend(),
            proton_version: String::new(),
            proton_source: ProtonSource::default(),
            proton_custom_path: String::new(),
            umu_source: UmuSource::default(),
            proton_prefix: String::new(),
            launch_options: String::new(),
            game_id: default_game_id(),
            env_overrides: String::new(),
            proton_debug_logging: false,
        }
    }
}

fn default_crossover_bottle() -> String {
    crate::crossover::DEFAULT_BOTTLE.into()
}

fn default_crossover_backend() -> String {
    crate::crossover::DEFAULT_BACKEND.into()
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ProtonSource {
    #[default]
    Managed,
    Custom,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum UmuSource {
    #[default]
    Managed,
    System,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AuthMethod {
    #[default]
    Qr,
    TwoFactor,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformSupport {
    pub os: String,
    pub arch: String,
    pub level: String,
    pub can_launch: bool,
    pub summary: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseInfo {
    pub tag: String,
    pub asset_name: String,
    pub download_url: String,
    pub size: u64,
    pub digest: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseAsset {
    pub name: String,
    pub download_url: String,
    pub size: u64,
    pub digest: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseSummary {
    pub tag: String,
    pub name: String,
    pub prerelease: bool,
    pub published_at: Option<DateTime<Utc>>,
    pub assets: Vec<ReleaseAsset>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallerState {
    #[serde(default = "schema_version")]
    pub schema_version: u8,
    #[serde(default = "steam_app_id")]
    pub app_id: u32,
    pub release_tag: String,
    pub release_asset: String,
    pub release_asset_digest: Option<String>,
    pub installed_dll_sha256: String,
    pub installed_at_utc: DateTime<Utc>,
    // Absent in the first state schema; the language then comes from settings.json.
    #[serde(default)]
    pub steam_language: Option<String>,
    pub manifests: BTreeMap<u32, u64>,
    // Old language files still to delete; the list is for the recorded language.
    #[serde(default)]
    pub pending_language_files: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub missions_commit: Option<String>,
}

fn schema_version() -> u8 {
    2
}

fn steam_app_id() -> u32 {
    STEAM_APP_ID
}

fn default_steam_language() -> String {
    "english".into()
}

fn default_game_id() -> String {
    "sunrise".into()
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallationSnapshot {
    pub status: String,
    pub message: String,
    pub game_found: bool,
    pub installed_release: Option<String>,
    pub installed_release_digest: Option<String>,
    pub installed_at: Option<DateTime<Utc>>,
    pub local_file_changed: bool,
    pub steam_language: Option<String>,
    pub missions_commit: Option<String>,
}

impl InstallationSnapshot {
    pub fn missing(message: impl Into<String>) -> Self {
        Self {
            status: "notInstalled".into(),
            message: message.into(),
            game_found: false,
            installed_release: None,
            installed_release_digest: None,
            installed_at: None,
            local_file_changed: false,
            steam_language: None,
            missions_commit: None,
        }
    }

    pub fn update_available(&self, latest: &ReleaseInfo) -> bool {
        let Some(installed_tag) = self.installed_release.as_deref() else {
            return false;
        };
        if !installed_tag.eq_ignore_ascii_case(&latest.tag) {
            return true;
        }

        let Some(installed_digest) = self
            .installed_release_digest
            .as_deref()
            .filter(|digest| !digest.trim().is_empty())
        else {
            return false;
        };
        let Some(latest_digest) = latest
            .digest
            .as_deref()
            .filter(|digest| !digest.trim().is_empty())
        else {
            return false;
        };

        !installed_digest.eq_ignore_ascii_case(latest_digest)
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSnapshot {
    pub platform: PlatformSupport,
    pub preferences: Preferences,
    pub installation: InstallationSnapshot,
    pub latest_release: Option<ReleaseInfo>,
    pub update_available: bool,
    pub release_error: Option<String>,
    /// Present only on macOS; `None` everywhere else.
    pub crossover: Option<crate::crossover::CrossOverStatus>,
    /// Present only on Linux; `None` everywhere else.
    pub proton: Option<ProtonStatus>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtonRelease {
    pub tag: String,
    pub asset_name: String,
    pub download_url: String,
    pub size: u64,
    pub digest: Option<String>,
    pub published_at: Option<DateTime<Utc>>,
    pub prerelease: bool,
    pub installed: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtonStatus {
    pub supported: bool,
    pub umu_available: bool,
    pub umu_version: Option<String>,
    /// "managed" when we supply umu-run, "system" when it came from `PATH`.
    pub umu_source: Option<String>,
    pub selected_tag: Option<String>,
    pub proton_path: Option<String>,
    pub installed_tags: Vec<String>,
    pub prefix_path: String,
    pub prefix_initialized: bool,
    /// Whether the selected Proton is usable for a launch right now.
    pub ready: bool,
    /// A blocking problem to show the user, if any.
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OperationKind {
    Install,
    Repair,
    Update,
    Missions,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationRequest {
    pub kind: OperationKind,
    pub install_directory: String,
    #[serde(default)]
    pub steam_username: String,
    #[serde(default = "default_steam_language")]
    pub steam_language: String,
    #[serde(default)]
    pub auth_method: AuthMethod,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationResult {
    pub changed: bool,
    pub release_tag: String,
    pub message: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase", tag = "event")]
pub enum OperationEvent {
    Progress {
        stage: String,
        message: String,
        percent: f32,
    },
    Terminal {
        stream: String,
        text: String,
    },
    QrCode {
        rows: Vec<String>,
    },
    AuthPrompt {
        kind: String,
        message: String,
    },
    AuthComplete,
    Notice {
        message: String,
    },
}

pub fn current_platform() -> PlatformSupport {
    let os = std::env::consts::OS.to_string();
    let arch = std::env::consts::ARCH.to_string();
    match std::env::consts::OS {
        "windows" => PlatformSupport {
            os,
            arch,
            level: "supported".into(),
            can_launch: true,
            summary: "Native install, repair, update, and launch support.".into(),
        },
        "linux" => PlatformSupport {
            os,
            arch,
            level: "experimental".into(),
            can_launch: false,
            summary: "Native installation is supported; launching through Proton is a follow-up integration.".into(),
        },
        "macos" => PlatformSupport {
            os,
            arch,
            level: "experimental".into(),
            // Raised once CrossOver and the game's bottle are found.
            can_launch: false,
            summary: "Native installation is supported; the game launches through CrossOver.".into(),
        },
        _ => PlatformSupport {
            os,
            arch,
            level: "unsupported".into(),
            can_launch: false,
            summary: "This platform does not have a matching DepotDownloader build.".into(),
        },
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{
        AuthMethod, InstallationSnapshot, InstallerState, LANGUAGES, Preferences, ProtonSource,
        ReleaseInfo, UmuSource, depots_for, resolve_language,
    };

    fn installed_snapshot(digest: Option<&str>) -> InstallationSnapshot {
        InstallationSnapshot {
            status: "installed".into(),
            message: "Sunrise is installed and ready.".into(),
            game_found: true,
            installed_release: Some("v1.2.3".into()),
            installed_release_digest: digest.map(str::to_owned),
            installed_at: None,
            local_file_changed: false,
            steam_language: None,
            missions_commit: None,
        }
    }

    fn latest_release(tag: &str, digest: Option<&str>) -> ReleaseInfo {
        ReleaseInfo {
            tag: tag.into(),
            asset_name: "steam_api64.dll".into(),
            download_url: "https://example.invalid/steam_api64.dll".into(),
            size: 1,
            digest: digest.map(str::to_owned),
        }
    }

    #[test]
    fn update_detection_matches_the_legacy_tag_and_digest_rules() {
        let installed = installed_snapshot(Some("sha256:aabb"));
        assert!(!installed.update_available(&latest_release("V1.2.3", Some("SHA256:AABB"))));
        assert!(installed.update_available(&latest_release("v1.2.4", Some("sha256:aabb"))));
        assert!(installed.update_available(&latest_release("v1.2.3", Some("sha256:ccdd"))));
        assert!(!installed.update_available(&latest_release("v1.2.3", None)));
    }

    #[test]
    fn reads_install_state_written_by_the_legacy_launcher() {
        let state: InstallerState = serde_json::from_str(
            r#"{
                "schemaVersion": 1,
                "appId": 1085660,
                "releaseTag": "v1.2.3",
                "releaseAsset": "steam_api64.dll",
                "releaseAssetDigest": "sha256:aabb",
                "installedDllSha256": "ccdd",
                "installedAtUtc": "2026-08-26T12:00:00+00:00",
                "manifests": { "1085661": 7180122903232116872 }
            }"#,
        )
        .expect("legacy state should remain compatible");

        assert_eq!(state.release_tag, "v1.2.3");
        assert_eq!(state.steam_language, None);
        assert!(state.pending_language_files.is_empty());
        assert_eq!(state.release_asset_digest.as_deref(), Some("sha256:aabb"));
        assert_eq!(
            state.manifests.get(&1_085_661),
            Some(&7_180_122_903_232_116_872)
        );
    }

    #[test]
    fn preferences_written_before_crossover_support_still_load() {
        let preferences: Preferences = serde_json::from_str(
            r#"{
                "installDirectory": "/Users/guardian/Games/Sunrise",
                "steamUsername": "guardian",
                "steamLanguage": "french",
                "authMethod": "twoFactor"
            }"#,
        )
        .expect("legacy preferences should remain compatible");

        assert_eq!(preferences.auth_method, AuthMethod::TwoFactor);
        assert_eq!(preferences.crossover_bottle, "Sunrise");
        assert_eq!(preferences.crossover_backend, "d3dmetal");
    }

    #[test]
    fn preferences_written_before_proton_support_still_load() {
        let preferences: Preferences = serde_json::from_str(
            r#"{
                "installDirectory": "C:\\Games\\Project Sunrise",
                "steamUsername": "guardian",
                "steamLanguage": "french",
                "authMethod": "twoFactor"
            }"#,
        )
        .expect("legacy preferences should remain compatible");

        assert_eq!(preferences.steam_language, "french");
        assert_eq!(preferences.auth_method, AuthMethod::TwoFactor);
        assert_eq!(preferences.proton_source, ProtonSource::Managed);
        assert_eq!(preferences.umu_source, UmuSource::Managed);
        assert_eq!(preferences.game_id, "sunrise");
        assert!(preferences.proton_version.is_empty());
        assert!(preferences.proton_custom_path.is_empty());
        assert!(preferences.proton_prefix.is_empty());
        assert!(preferences.launch_options.is_empty());
        assert!(preferences.env_overrides.is_empty());
    }

    #[test]
    fn proton_preferences_round_trip() {
        let preferences = Preferences {
            proton_version: "GE-Proton11-7".into(),
            proton_source: ProtonSource::Custom,
            proton_custom_path: "/opt/proton".into(),
            umu_source: UmuSource::System,
            proton_prefix: "/games/sunrise-prefix".into(),
            launch_options: "mangohud %command% -novid".into(),
            game_id: "sunrise".into(),
            env_overrides: "DXVK_HUD=1".into(),
            ..Preferences::default()
        };
        let bytes = serde_json::to_vec(&preferences).expect("serialize");
        let parsed: Preferences = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(parsed.proton_version, "GE-Proton11-7");
        assert_eq!(parsed.proton_source, ProtonSource::Custom);
        assert_eq!(parsed.umu_source, UmuSource::System);
        assert_eq!(parsed.launch_options, "mangohud %command% -novid");
        assert_eq!(parsed.env_overrides, "DXVK_HUD=1");
    }

    #[test]
    fn language_specs_have_unique_steam_names_and_depots() {
        let names = LANGUAGES
            .iter()
            .map(|language| language.steam_language)
            .collect::<BTreeSet<_>>();
        let depots = LANGUAGES
            .iter()
            .map(|language| language.depot.depot_id)
            .collect::<BTreeSet<_>>();
        assert_eq!(names.len(), LANGUAGES.len());
        assert_eq!(depots.len(), LANGUAGES.len());
        assert_eq!(resolve_language("FRENCH").display_name, "French");
        assert_eq!(resolve_language("unknown").steam_language, "english");
        assert_eq!(
            depots_for(resolve_language("koreana"))[1].depot_id,
            1_085_674
        );
    }
}
