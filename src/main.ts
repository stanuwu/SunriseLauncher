import { Channel, invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";

type AuthMethod = "qr" | "twoFactor";
type ProtonSource = "managed" | "custom";
type UmuSource = "managed" | "system";
type Preferences = {
  installDirectory: string;
  steamUsername: string;
  steamLanguage: string;
  authMethod: AuthMethod;
  crossoverBottle: string;
  crossoverBackend: string;
  protonVersion: string;
  protonSource: ProtonSource;
  protonCustomPath: string;
  umuSource: UmuSource;
  protonPrefix: string;
  launchOptions: string;
  gameId: string;
  envOverrides: string;
  protonDebugLogging: boolean;
};
type CrossOverStatus = {
  supported: boolean;
  installed: boolean;
  appPath: string | null;
  version: string | null;
  bottleName: string;
  bottlePath: string;
  bottleExists: boolean;
  graphicsBackend: string | null;
  ready: boolean;
  error: string | null;
};
type PlatformSupport = {
  os: string;
  arch: string;
  level: string;
  canLaunch: boolean;
  summary: string;
};
type InstallationSnapshot = {
  status: "notInstalled" | "unmanaged" | "needsRepair" | "modified" | "installed";
  message: string;
  gameFound: boolean;
  installedRelease: string | null;
  installedReleaseDigest: string | null;
  installedAt: string | null;
  localFileChanged: boolean;
  steamLanguage: string | null;
  missionsCommit: string | null;
};
type ReleaseInfo = {
  tag: string;
  assetName: string;
  downloadUrl: string;
  size: number;
  digest: string | null;
};
type ProtonRelease = {
  tag: string;
  assetName: string;
  downloadUrl: string;
  size: number;
  digest: string | null;
  publishedAt: string | null;
  prerelease: boolean;
  installed: boolean;
};
type ProtonStatus = {
  supported: boolean;
  umuAvailable: boolean;
  umuVersion: string | null;
  umuSource: string | null;
  selectedTag: string | null;
  protonPath: string | null;
  installedTags: string[];
  prefixPath: string;
  prefixInitialized: boolean;
  ready: boolean;
  error: string | null;
};
type AppSnapshot = {
  platform: PlatformSupport;
  preferences: Preferences;
  installation: InstallationSnapshot;
  latestRelease: ReleaseInfo | null;
  updateAvailable: boolean;
  releaseError: string | null;
  crossover: CrossOverStatus | null;
  proton: ProtonStatus | null;
};
type OperationKind = "install" | "repair" | "update" | "missions";
type OperationEvent =
  | { event: "progress"; stage: string; message: string; percent: number }
  | { event: "terminal"; stream: string; text: string }
  | { event: "qrCode"; rows: string[] }
  | { event: "authPrompt"; kind: "password" | "twoFactor" | "emailCode"; message: string }
  | { event: "authComplete" }
  | { event: "notice"; message: string };
type EventSink = { onmessage?: (message: OperationEvent) => void };

const LANGUAGE_LABELS: Record<string, string> = {
  english: "English",
  french: "French",
  german: "German",
  italian: "Italian",
  japanese: "Japanese",
  brazilian: "Portuguese (Brazil)",
  spanish: "Spanish (Spain)",
  russian: "Russian",
  polish: "Polish",
  schinese: "Chinese (Simplified)",
  tchinese: "Chinese (Traditional)",
  latam: "Spanish (Latin America)",
  koreana: "Korean",
};

const languageLabel = (steamLanguage: string) => LANGUAGE_LABELS[steamLanguage] ?? "English";

const element = <T extends HTMLElement>(selector: string): T => {
  const found = document.querySelector<T>(selector);
  if (!found) throw new Error(`Missing interface element: ${selector}`);
  return found;
};

const installDirectory = element<HTMLInputElement>("#install-directory");
const steamUsername = element<HTMLInputElement>("#steam-username");
const gameLanguage = element<HTMLSelectElement>("#game-language");
const primaryAction = element<HTMLButtonElement>("#primary-action");
const primaryLabel = element<HTMLElement>("#primary-label");
const repairAction = element<HTMLButtonElement>("#repair-action");
const missionsAction = element<HTMLButtonElement>("#missions-action");
const launchAction = element<HTMLButtonElement>("#launch-action");
const crossoverSection = element<HTMLElement>("#crossover-section");
const crossoverStatus = element<HTMLElement>("#crossover-status");
const crossoverBottle = element<HTMLInputElement>("#crossover-bottle");
const crossoverBackend = element<HTMLSelectElement>("#crossover-backend");
const crossoverPrepare = element<HTMLButtonElement>("#crossover-prepare");
const consoleOutput = element<HTMLElement>("#console-output");
const toast = element<HTMLElement>("#toast");
const settingsView = element<HTMLElement>("#settings-view");
const setupView = element<HTMLElement>("#setup-view");
const authView = element<HTMLElement>("#auth-view");
const operationStrip = element<HTMLElement>("#operation-strip");
const setupInstallDirectory = element<HTMLInputElement>("#setup-install-directory");
const setupSteamUsername = element<HTMLInputElement>("#setup-steam-username");
const setupGameLanguage = element<HTMLSelectElement>("#setup-game-language");
const setupPathStatus = element<HTMLElement>("#setup-path-status");
const setupBack = element<HTMLButtonElement>("#setup-back");
const setupNext = element<HTMLButtonElement>("#setup-next");
const authResponseForm = element<HTMLFormElement>("#auth-response-form");
const authResponse = element<HTMLInputElement>("#auth-response");
const authPrivacy = element<HTMLElement>("#auth-privacy");
const authLoadingPanel = element<HTMLElement>("#auth-loading-panel");
const authLoadingTitle = element<HTMLElement>("#auth-loading-title");
const authLoadingDetail = element<HTMLElement>("#auth-loading-detail");
const authQrCode = (() => {
  const found = document.querySelector<SVGSVGElement>("#auth-qr-code");
  if (!found) throw new Error("Missing interface element: #auth-qr-code");
  return found;
})();
const protonSection = element<HTMLElement>("#proton-section");
const protonStatus = element<HTMLElement>("#proton-status");
const protonVersion = element<HTMLSelectElement>("#proton-version");
const protonCustomPath = element<HTMLInputElement>("#proton-custom-path");
const protonUmuSource = element<HTMLSelectElement>("#proton-umu-source");
const protonToggle = element<HTMLButtonElement>("#proton-toggle");
const launchOptions = element<HTMLInputElement>("#launch-options");
const protonGameId = element<HTMLInputElement>("#proton-game-id");
const protonEnv = element<HTMLTextAreaElement>("#proton-env");
const protonDebug = element<HTMLInputElement>("#proton-debug");
const protonProgress = element<HTMLElement>("#proton-progress");
const protonProgressKind = element<HTMLElement>("#proton-progress-kind");
const protonProgressMessage = element<HTMLElement>("#proton-progress-message");
const protonProgressPercent = element<HTMLElement>("#proton-progress-percent");
const protonProgressFill = element<HTMLElement>("#proton-progress-fill");
const protonProgressCancel = element<HTMLButtonElement>("#proton-progress-cancel");
const setupProtonPage = element<HTMLElement>("#setup-proton-page");
const setupProtonVersion = element<HTMLSelectElement>("#setup-proton-version");
const setupProtonCustom = element<HTMLInputElement>("#setup-proton-custom");

let snapshot: AppSnapshot | null = null;
let primaryMode: OperationKind | "launch" = "install";
let activeOperationKind: OperationKind = "install";
let operationRunning = false;
let gameLaunching = false;
let operationCancellationRequested = false;
let setupStep = 0;
let setupDraft: Preferences = {
  installDirectory: "",
  steamUsername: "",
  steamLanguage: "english",
  authMethod: "qr",
  crossoverBottle: "Sunrise",
  crossoverBackend: "d3dmetal",
  protonVersion: "",
  protonSource: "managed",
  protonCustomPath: "",
  umuSource: "managed",
  protonPrefix: "",
  launchOptions: "",
  gameId: "sunrise",
  envOverrides: "",
  protonDebugLogging: false,
};
let setupInspection: InstallationSnapshot | null = null;
let setupLanguageFolder = "";
let activeAuthMethod: AuthMethod = "qr";
let activeAuthPrompt: "password" | "twoFactor" | "emailCode" | null = null;
let authSessionReady = false;
let passwordPromptSeen = false;
let pendingAuthRestart: AuthMethod | null = null;
let statusTimer: number | undefined;
let setupInspectionTimer: number | undefined;
let toastTimer: number | undefined;
let operationHideTimer: number | undefined;
let gameStatusTimer: number | undefined;
let authHideTimer: number | undefined;
let mockOperationCancelled = false;
let mockGameRunning = false;
let protonReleases: ProtonRelease[] = [];
let protonCatalogError: string | null = null;
type ProtonOperation = "install" | "remove" | "prepare";
let activeProtonOperation: ProtonOperation | null = null;
let protonProgressHideTimer: number | undefined;
let protonReconcileTimer: number | undefined;
let lastConsoleLine: string | null = null;
const isLinux = () => snapshot?.platform.os === "linux";
const GAME_POLL_MS = 1000;
const inTauri = "__TAURI_INTERNALS__" in window;
const appWindow = inTauri ? getCurrentWindow() : null;
const mockQrRows = [
  "                      ",
  "  ██████  ██  ██████  ",
  "  ██  ██    ██  ██  ██  ",
  "  ██  ██  ████  ██  ██  ",
  "  ██████  ██    ██████  ",
  "          ████          ",
  "  ██████    ██████  ██  ",
  "  ██  ██  ██    ████    ",
  "  ██  ██    ████  ████  ",
  "  ██████  ██  ██    ██  ",
  "                      ",
];

const mockSnapshot: AppSnapshot = {
  platform: {
    os: "windows",
    arch: "x86_64",
    level: "supported",
    canLaunch: true,
    summary: "Native install, repair, update, and launch support.",
  },
  preferences: {
    installDirectory: "C:\\Games\\Project Sunrise",
    steamUsername: "",
    steamLanguage: "english",
    authMethod: "qr",
    crossoverBottle: "Sunrise",
    crossoverBackend: "d3dmetal",
    protonVersion: "",
    protonSource: "managed",
    protonCustomPath: "",
    umuSource: "managed",
    protonPrefix: "",
    launchOptions: "",
    gameId: "sunrise",
    envOverrides: "",
    protonDebugLogging: false,
  },
  installation: {
    status: "notInstalled",
    message: "This folder is ready for a new Sunrise installation.",
    gameFound: false,
    installedRelease: null,
    installedReleaseDigest: null,
    installedAt: null,
    localFileChanged: false,
    steamLanguage: null,
    missionsCommit: null,
  },
  latestRelease: {
    tag: "0.3.2",
    assetName: "steam_api64.dll",
    downloadUrl: "",
    size: 7_870_464,
    digest: "sha256:preview",
  },
  updateAvailable: false,
  releaseError: null,
  crossover: null,
  proton: null,
};

async function invokeCommand<T>(command: string, args: Record<string, unknown> = {}): Promise<T> {
  if (inTauri) return invoke<T>(command, args);
  if (command === "get_app_snapshot") return structuredClone(mockSnapshot) as T;
  if (command === "get_proton_status") return null as T;
  if (command === "list_proton_releases") {
    return [
      {
        tag: "GE-Proton11-7",
        assetName: "GE-Proton11-7-x86_64.tar.gz",
        downloadUrl: "",
        size: 563_784_602,
        digest: "sha256:preview",
        publishedAt: null,
        prerelease: false,
        installed: false,
      },
    ] as T;
  }
  if (command === "install_proton" || command === "remove_proton" || command === "initialize_proton_prefix") {
    return null as T;
  }
  if (command === "inspect_installation") {
    const selected = String(args.installDirectory ?? "").trim();
    const knownInstallation = selected === mockSnapshot.preferences.installDirectory && mockSnapshot.installation.gameFound;
    const exampleExistingInstallation = /existing|destiny 2/i.test(selected);
    if (knownInstallation) return structuredClone(mockSnapshot.installation) as T;
    if (exampleExistingInstallation) {
      return {
        status: "unmanaged",
        message: "Destiny 2 was found, but this launcher has not installed Sunrise here.",
        gameFound: true,
        installedRelease: null,
        installedReleaseDigest: null,
        installedAt: null,
        localFileChanged: false,
        steamLanguage: null,
        missionsCommit: null,
      } as T;
    }
    return {
      status: "notInstalled",
      message: selected
        ? "This folder is ready for a new Sunrise installation."
        : "Choose an installation folder to get started.",
      gameFound: false,
      installedRelease: null,
      installedReleaseDigest: null,
      installedAt: null,
      localFileChanged: false,
      steamLanguage: null,
      missionsCommit: null,
    } as T;
  }
  if (command === "run_operation") {
    mockOperationCancelled = false;
    const channel = args.onEvent as EventSink;
    const request = args.request as {
      kind?: OperationKind;
      installDirectory?: string;
      steamUsername?: string;
      steamLanguage?: string;
      authMethod?: AuthMethod;
    };
    const authenticationEvents: OperationEvent[] = request.authMethod === "twoFactor"
      ? [
          { event: "authPrompt", kind: "password", message: "Enter your Steam password" },
          { event: "authPrompt", kind: "twoFactor", message: "Enter the Steam Guard code from your authenticator" },
        ]
      : [{ event: "qrCode", rows: mockQrRows }];
    const events: OperationEvent[] = [
      { event: "progress", stage: "preflight", message: "Checking the installation folder…", percent: 1 },
      { event: "progress", stage: "tool", message: "Preparing DepotDownloader…", percent: 8 },
      { event: "notice", message: "Preview mode: QR sign-in is simulated and no credentials are being used." },
      { event: "progress", stage: "depots", message: "Downloading Steam depot 1085661…", percent: 48 },
      ...authenticationEvents,
      { event: "authComplete" },
      { event: "terminal", stream: "stdout", text: "Connecting to Steam…\nDepot download preview complete.\n" },
      { event: "progress", stage: "release", message: "Verifying steam_api64.dll…", percent: 94 },
      { event: "progress", stage: "install", message: "Installing Sunrise…", percent: 98 },
      { event: "progress", stage: "complete", message: "Installed Sunrise 0.3.2 successfully.", percent: 100 },
    ];
    for (const event of events) {
      await new Promise((resolve) => window.setTimeout(resolve, 90));
      if (mockOperationCancelled) throw new Error("The operation was cancelled.");
      channel.onmessage?.(event);
      if (event.event === "qrCode" || event.event === "authPrompt") {
        await new Promise((resolve) => window.setTimeout(resolve, 1800));
      }
      if (event.event === "authComplete") {
        await new Promise((resolve) => window.setTimeout(resolve, 800));
      }
    }
    mockSnapshot.preferences.installDirectory = request.installDirectory ?? mockSnapshot.preferences.installDirectory;
    mockSnapshot.preferences.steamUsername = request.steamUsername ?? mockSnapshot.preferences.steamUsername;
    mockSnapshot.preferences.steamLanguage = request.steamLanguage ?? mockSnapshot.preferences.steamLanguage;
    mockSnapshot.installation = {
      status: "installed",
      message: "Sunrise is installed and ready to play.",
      gameFound: true,
      installedRelease: mockSnapshot.latestRelease?.tag ?? "0.3.2",
      installedReleaseDigest: mockSnapshot.latestRelease?.digest ?? null,
      installedAt: new Date().toISOString(),
      localFileChanged: false,
      steamLanguage: mockSnapshot.preferences.steamLanguage,
      missionsCommit: "0000000000000000000000000000000000000000",
    };
    mockSnapshot.updateAvailable = false;
    return { changed: true, releaseTag: "0.3.2", message: "Preview operation complete." } as T;
  }
  if (command === "cancel_operation") {
    mockOperationCancelled = true;
    return true as T;
  }
  if (command === "launch_game") {
    mockGameRunning = true;
    return undefined as T;
  }
  if (command === "game_status") return mockGameRunning as T;
  if (command === "proton_operation_active") return false as T;
  if (command === "send_terminal_input") return true as T;
  return undefined as T;
}

function showToast(message: string, kind: "info" | "error" = "info") {
  window.clearTimeout(toastTimer);
  toast.textContent = message;
  toast.dataset.kind = kind;
  toast.hidden = false;
  toastTimer = window.setTimeout(() => {
    toast.hidden = true;
  }, 5200);
}

function humanPlatform(os: string, arch: string) {
  const names: Record<string, string> = { windows: "Windows", linux: "Linux", macos: "macOS" };
  const architectures: Record<string, string> = { x86_64: "x64", aarch64: "ARM64" };
  return `${names[os] ?? os} · ${architectures[arch] ?? arch}`;
}

function statusCopy(installation: InstallationSnapshot) {
  switch (installation.status) {
    case "installed":
      return { label: "Ready", title: "Installation healthy", className: "healthy", icon: "✓" };
    case "modified":
      return { label: "Needs attention", title: "Local files changed", className: "warning", icon: "!" };
    case "needsRepair":
      return { label: "Repair needed", title: "Installation incomplete", className: "warning", icon: "!" };
    case "unmanaged":
      return { label: "Game detected", title: "Existing game found", className: "neutral", icon: "•" };
    default:
      return { label: "Not installed", title: "No installation yet", className: "neutral", icon: "—" };
  }
}

function updateLanguageWarnings() {
  element<HTMLElement>("#settings-language-warning").hidden = gameLanguage.value === "english";
  element<HTMLElement>("#setup-language-warning").hidden = setupGameLanguage.value === "english";
}

function crossoverPreferences() {
  return {
    crossoverBottle: crossoverBottle.value.trim() || "Sunrise",
    crossoverBackend: crossoverBackend.value || "d3dmetal",
  };
}

function renderCrossover(data: AppSnapshot) {
  const status = data.crossover;
  crossoverSection.hidden = data.platform.os !== "macos" || !status;
  if (!status) return;
  crossoverBottle.value = data.preferences.crossoverBottle || "Sunrise";
  crossoverBackend.value = data.preferences.crossoverBackend || "d3dmetal";
  let text: string;
  let className = "crossover-status";
  const version = status.version ? ` ${status.version}` : "";
  if (status.error) {
    text = status.error;
    className += " problem";
  } else if (status.bottleExists) {
    const backend = status.graphicsBackend ? ` · ${status.graphicsBackend.toUpperCase()}` : "";
    text = `CrossOver${version} found · bottle "${status.bottleName}" is ready${backend}.`;
    className += " ready";
  } else {
    text = `CrossOver${version} found. The bottle "${status.bottleName}" does not exist yet; prepare it before the first launch.`;
    className += " problem";
  }
  crossoverStatus.textContent = text;
  crossoverStatus.className = className;
  crossoverPrepare.disabled = operationRunning || gameLaunching || !status.installed || status.bottleExists;
  crossoverPrepare.textContent = status.bottleExists ? "Bottle ready" : "Prepare bottle";
}

function protonReady() {
  return Boolean(snapshot?.proton?.ready);
}

/** Launch is possible on Windows, or on Linux */
function canLaunch() {
  if (!snapshot) return false;
  if (snapshot.platform.canLaunch) return true;
  return isLinux() && protonReady();
}

function protonStatusCopy(status: ProtonStatus | null): string {
  if (!status) return "Proton is unavailable on this platform.";
  if (status.error) return status.error;
  const umu = !status.umuAvailable
    ? "umu-launcher missing"
    : status.umuSource === "system"
      ? "umu-launcher (system)"
      : `umu-launcher ${status.umuVersion} (${status.umuSource})`;
  const proton = status.selectedTag ?? "no Proton selected";
  const prefix = status.prefixInitialized ? "environment ready" : "environment not prepared";
  return `${umu} · ${proton} · ${prefix}`;
}

function renderProtonOptions(select: HTMLSelectElement, selected: string) {
  const installed = snapshot?.proton?.installedTags ?? [];
  const options: HTMLOptionElement[] = [];

  for (const tag of installed) {
    const option = document.createElement("option");
    option.value = tag;
    option.textContent = `${tag} (installed)`;
    options.push(option);
  }
  for (const release of protonReleases) {
    if (installed.includes(release.tag)) continue;
    const option = document.createElement("option");
    option.value = release.tag;
    option.textContent = release.prerelease ? `${release.tag} (pre-release)` : release.tag;
    options.push(option);
  }

  select.replaceChildren(...options);
  if (selected && options.some((option) => option.value === selected)) {
    select.value = selected;
  } else if (options.length) {
    select.value = options[0].value;
  }
}

function renderProton() {
  const status = snapshot?.proton ?? null;
  protonSection.hidden = !isLinux();
  if (!isLinux()) return;

  protonStatus.textContent = protonStatusCopy(status);
  protonStatus.classList.toggle("ready", Boolean(status?.ready));
  protonStatus.classList.toggle("problem", Boolean(status?.error));

  const desired = snapshot?.preferences.protonVersion?.trim() || status?.selectedTag || "";
  renderProtonOptions(protonVersion, desired);
  protonCustomPath.value = snapshot?.preferences.protonCustomPath ?? "";
  protonUmuSource.value = snapshot?.preferences.umuSource ?? "managed";
  launchOptions.value = snapshot?.preferences.launchOptions ?? "";
  protonGameId.value = snapshot?.preferences.gameId ?? "sunrise";
  protonEnv.value = snapshot?.preferences.envOverrides ?? "";
  protonDebug.checked = Boolean(snapshot?.preferences.protonDebugLogging);

  const busy = operationRunning;
  const selected = protonVersion.value;
  const selectedInstalled = (status?.installedTags ?? []).includes(selected);
  protonToggle.textContent = selectedInstalled ? "Remove" : "Download";
  protonToggle.classList.toggle("danger", selectedInstalled);
  protonToggle.disabled = busy || !selected;
  element<HTMLButtonElement>("#proton-prepare").disabled = busy || !status?.protonPath || !status?.umuAvailable;
  element<HTMLButtonElement>("#proton-refresh").disabled = busy;
}

async function loadProtonReleases(refresh = false) {
  if (!isLinux()) return;
  protonCatalogError = null;
  try {
    protonReleases = await invokeCommand<ProtonRelease[]>("list_proton_releases", { refresh });
  } catch (error) {
    protonReleases = [];
    protonCatalogError = String(error);
  }
  renderProton();
}

async function saveProtonPreferences() {
  if (!snapshot) return;
  const preferences: Preferences = {
    ...snapshot.preferences,
    protonVersion: protonVersion.value,
    protonSource: protonCustomPath.value.trim() ? "custom" : "managed",
    protonCustomPath: protonCustomPath.value.trim(),
    umuSource: protonUmuSource.value as UmuSource,
    launchOptions: launchOptions.value,
    gameId: protonGameId.value.trim() || "sunrise",
    envOverrides: protonEnv.value,
    protonDebugLogging: protonDebug.checked,
  };
  try {
    await invokeCommand("save_preferences", { preferences });
    snapshot = { ...snapshot, preferences };
    const status = await invokeCommand<ProtonStatus | null>("get_proton_status", { preferences });
    snapshot = { ...snapshot, proton: status };
    renderProton();
    renderSnapshot(snapshot);
  } catch (error) {
    showToast(String(error), "error");
  }
}

function setProtonProgress(kind: ProtonOperation, message: string, percent: number, cancellable = true) {
  window.clearTimeout(protonProgressHideTimer);
  protonProgressKind.textContent =
    kind === "prepare" ? "PREPARING PROTON" : kind === "remove" ? "REMOVING PROTON" : "DOWNLOADING PROTON";
  protonProgressMessage.textContent = message;
  const normalized = Math.max(0, Math.min(100, percent));
  const percentLabel = Number.isInteger(normalized) ? String(normalized) : normalized.toFixed(2);
  protonProgressPercent.textContent = `${percentLabel}%`;
  protonProgressFill.style.width = `${normalized}%`;
  protonProgress.querySelector<HTMLElement>(".progress-track")!.setAttribute("aria-valuenow", normalized.toFixed(1));
  protonProgressCancel.disabled = !cancellable;
  protonProgress.hidden = false;
}

function hideProtonProgress() {
  window.clearTimeout(protonProgressHideTimer);
  protonProgress.hidden = true;
  protonProgressCancel.disabled = true;
}

function stopProtonReconcile() {
  window.clearInterval(protonReconcileTimer);
  protonReconcileTimer = undefined;
}

async function pollProtonOperation() {
  try {
    if (await invokeCommand<boolean>("proton_operation_active")) return;
  } catch {
    return;
  }
  stopProtonReconcile();
  operationRunning = false;
  activeProtonOperation = null;
  hideProtonProgress();
  await loadSnapshot(true);
  await loadProtonReleases();
}

// Re-attaches to a proton operation that the backend is still running after the
// webview lost track of it (for example a dev reload).
async function syncProtonOperation() {
  if (activeProtonOperation) {
    renderProton();
    return;
  }
  let busy = false;
  try {
    busy = await invokeCommand<boolean>("proton_operation_active");
  } catch {
    return;
  }
  if (!busy) return;
  operationRunning = true;
  setProtonProgress("install", "A Proton operation is already running…", 0, false);
  renderProton();
  stopProtonReconcile();
  protonReconcileTimer = window.setInterval(() => void pollProtonOperation(), GAME_POLL_MS);
}

function handleProtonEvent(message: OperationEvent) {
  if (message.event === "progress") {
    setProtonProgress(activeProtonOperation ?? "install", message.message, message.percent);
    return;
  }
  if (message.event === "notice") {
    element("#operation-notice").textContent = message.message;
    appendConsole(`[launcher] ${message.message}\n`, "notice");
  }
}

async function runProtonCommand(
  command: string,
  args: Record<string, unknown> = {},
  kind: ProtonOperation = "install",
) {
  if (operationRunning) return;
  operationRunning = true;
  activeProtonOperation = kind;
  operationStrip.hidden = true;
  stopProtonReconcile();
  renderProton();
  setProtonProgress(
    kind,
    kind === "remove" ? `Removing ${args.tag ?? ""}…` : "Starting…",
    0,
    kind !== "remove",
  );
  const onEvent: EventSink = inTauri ? new Channel<OperationEvent>() : {};
  if (kind !== "remove") onEvent.onmessage = handleProtonEvent;
  let success = false;
  try {
    if (kind === "remove") {
      setProtonProgress(kind, `Removing ${args.tag ?? ""}…`, 95, false);
      const status = await invokeCommand<ProtonStatus | null>(command, args);
      if (status && snapshot) snapshot = { ...snapshot, proton: status };
    } else {
      const status = await invokeCommand<ProtonStatus | null>(command, { ...args, onEvent });
      if (status && snapshot) snapshot = { ...snapshot, proton: status };
    }
    success = true;
    const toast = kind === "install"
      ? `GE-Proton ${args.tag ?? ""} installed.`
      : kind === "remove"
        ? "GE-Proton removed."
        : "Proton environment is ready.";
    showToast(toast);
  } catch (error) {
    showToast(String(error), "error");
  } finally {
    operationRunning = false;
    activeProtonOperation = null;
    if (success) {
      setProtonProgress(kind, "Done.", 100, true);
      protonProgressHideTimer = window.setTimeout(hideProtonProgress, 1200);
    } else {
      hideProtonProgress();
    }
    await loadSnapshot(true);
    await loadProtonReleases();
  }
}

function renderSnapshot(data: AppSnapshot) {
  snapshot = data;
  gameLanguage.value = data.preferences.steamLanguage || "english";
  renderCrossover(data);
  updateLanguageWarnings();
  const copy = statusCopy(data.installation);
  element("#install-status").textContent = copy.title;
  element("#install-message").textContent = data.installation.message;
  element("#health-ring").className = `health-ring ${copy.className}`;
  element("#health-icon").textContent = copy.icon;
  element("#installed-release").textContent = data.installation.installedRelease ?? "—";
  element("#installed-missions").textContent = data.installation.missionsCommit?.slice(0, 7) ?? "—";
  element("#available-release").textContent = data.latestRelease?.tag ?? "Unavailable";
  element("#latest-release").textContent = data.latestRelease?.tag ?? "Offline";
  element("#installed-at").textContent = data.installation.installedAt
    ? new Intl.DateTimeFormat(undefined, { dateStyle: "medium" }).format(new Date(data.installation.installedAt))
    : "—";
  element("#platform-summary").textContent = `${humanPlatform(data.platform.os, data.platform.arch)} · ${data.platform.summary}`;
  element("#folder-help").textContent = data.installation.message;

  const updateAvailable = data.updateAvailable;
  const installed = data.installation.status === "installed";
  const repairNeeded = ["modified", "needsRepair"].includes(data.installation.status);

  if (repairNeeded) {
    primaryMode = "repair";
    primaryLabel.textContent = "REPAIR";
  } else if (installed && updateAvailable) {
    primaryMode = "update";
    primaryLabel.textContent = "UPDATE";
  } else if (installed && canLaunch()) {
    primaryMode = "launch";
    primaryLabel.textContent = "PLAY";
  } else if (installed) {
    primaryMode = "update";
    primaryLabel.textContent = "CHECK";
  } else {
    primaryMode = "install";
    primaryLabel.textContent = "INSTALL";
  }

  if (gameLaunching) {
    primaryMode = "launch";
    primaryLabel.textContent = "RUNNING";
  }

  const supported = data.platform.level !== "unsupported";
  primaryAction.disabled = operationRunning || gameLaunching || !supported;
  repairAction.disabled = operationRunning || gameLaunching || !data.installation.gameFound;
  missionsAction.disabled = repairAction.disabled;
  launchAction.disabled = operationRunning || gameLaunching || !data.installation.gameFound || !canLaunch();
  launchAction.textContent = gameLaunching ? "Running" : "Launch game";
  renderProton();
}

async function loadSnapshot(useInputs = false) {
  try {
    const data = await invokeCommand<AppSnapshot>("get_app_snapshot");
    if (!useInputs) {
      installDirectory.value = data.preferences.installDirectory;
      steamUsername.value = data.preferences.steamUsername;
    } else if (installDirectory.value !== data.preferences.installDirectory) {
      data.preferences.installDirectory = installDirectory.value;
      data.preferences.steamUsername = steamUsername.value;
      data.installation = await invokeCommand<InstallationSnapshot>("inspect_installation", {
        installDirectory: installDirectory.value,
      });
      data.preferences.steamLanguage = data.installation.steamLanguage ?? gameLanguage.value;
    }
    renderSnapshot(data);
  } catch (error) {
    showToast(String(error), "error");
  }
}

async function saveAndInspect() {
  if (operationRunning) return;
  const preferences: Preferences = {
    ...(snapshot?.preferences ?? setupDraft),
    installDirectory: installDirectory.value.trim(),
    steamUsername: steamUsername.value.trim(),
    steamLanguage: gameLanguage.value,
    authMethod: snapshot?.preferences.authMethod ?? "qr",
    ...crossoverPreferences(),
  };
  try {
    const installation = await invokeCommand<InstallationSnapshot>("inspect_installation", {
      installDirectory: preferences.installDirectory,
    });
    // A newly chosen folder starts at its installed language; the user may then change it.
    if (installation.steamLanguage && preferences.installDirectory !== snapshot?.preferences.installDirectory) {
      preferences.steamLanguage = installation.steamLanguage;
    }
    await invokeCommand("save_preferences", { preferences });
    if (snapshot) renderSnapshot({ ...snapshot, preferences, installation });
  } catch (error) {
    showToast(String(error), "error");
  }
}

function setOverlay(view: HTMLElement, open: boolean) {
  view.hidden = !open;
  document.body.classList.toggle("overlay-open", !settingsView.hidden || !setupView.hidden || !authView.hidden);
}

function openSettings() {
  setOverlay(setupView, false);
  setOverlay(authView, false);
  setOverlay(settingsView, true);
  void loadProtonReleases();
  void syncProtonOperation();
  window.requestAnimationFrame(() => element<HTMLElement>("#settings-title").focus());
}

function closeSettings() {
  setOverlay(settingsView, false);
}

function renderSetupInspection(installation: InstallationSnapshot | null) {
  const existingGame = Boolean(installation?.gameFound);
  setupPathStatus.hidden = !existingGame;
  element("#setup-folder-requirement").textContent = existingGame
    ? "Existing game detected"
    : "Requires approximately 110 GiB";

  if (!installation?.gameFound) return;
  const managed = installation.status !== "unmanaged" && installation.status !== "notInstalled";
  setupPathStatus.querySelector("strong")!.textContent = managed
    ? "Existing Sunrise installation found."
    : "Existing Destiny 2 installation found.";
  setupPathStatus.querySelector("span")!.textContent =
    "DepotDownloader will verify these files and download only what is missing.";
}

async function inspectSetupDirectory(path: string, reportError = false) {
  const selected = path.trim();
  if (!selected) {
    setupInspection = null;
    renderSetupInspection(null);
    return null;
  }

  try {
    const installation = await invokeCommand<InstallationSnapshot>("inspect_installation", {
      installDirectory: selected,
    });
    if (setupInstallDirectory.value.trim() !== selected) return null;
    setupInspection = installation;
    renderSetupInspection(installation);
    return installation;
  } catch (error) {
    setupInspection = null;
    renderSetupInspection(null);
    if (reportError) showToast(String(error), "error");
    return null;
  }
}

function queueSetupInspection() {
  window.clearTimeout(setupInspectionTimer);
  setupDraft.installDirectory = setupInstallDirectory.value.trim();
  setupInspection = null;
  setupPathStatus.hidden = true;
  element("#setup-folder-requirement").textContent = setupDraft.installDirectory
    ? "Checking folder…"
    : "Requires approximately 110 GiB";
  setupInspectionTimer = window.setTimeout(() => {
    void inspectSetupDirectory(setupDraft.installDirectory);
  }, 350);
}

// The Proton page always occupies DOM/indicator slot 2 and review always occupies
// slot 3, whether or not Proton is shown; on non-Linux, slot 2 is simply skipped,
// which is why the step *count* needs its own position lookup below.
const REVIEW_STEP = 3;

function renderSetupStep() {
  const protonStep = isLinux();
  const totalSteps = protonStep ? 4 : 3;
  const stepOrder = protonStep ? [0, 1, 2, 3] : [0, 1, 3];
  setupProtonPage.hidden = !protonStep;
  element<HTMLElement>("#setup-proton-indicator").hidden = !protonStep;

  document.querySelectorAll<HTMLElement>("[data-setup-page]").forEach((page) => {
    page.hidden = Number(page.dataset.setupPage) !== setupStep;
  });
  document.querySelectorAll<HTMLElement>("[data-setup-indicator]").forEach((indicator) => {
    const index = Number(indicator.dataset.setupIndicator);
    indicator.classList.toggle("current", index === setupStep);
    indicator.classList.toggle("complete", index < setupStep);
  });
  setupBack.hidden = setupStep === 0;
  const existingGame = Boolean(setupInspection?.gameFound);
  setupNext.textContent = setupStep === REVIEW_STEP
    ? existingGame ? "Verify & install" : "Install Sunrise"
    : "Continue";
  const position = stepOrder.indexOf(setupStep) + 1;
  element("#setup-step-count").textContent = `${position} / ${totalSteps}`;
  element("#setup-kicker-location").textContent = `Step 1 of ${totalSteps}`;
  element("#setup-kicker-steam").textContent = `Step 2 of ${totalSteps}`;
  if (protonStep) element("#setup-kicker-proton").textContent = `Step 3 of ${totalSteps}`;
  element("#setup-kicker-review").textContent = `Step ${totalSteps} of ${totalSteps}`;
  if (setupStep === 0) {
    setupInstallDirectory.value = setupDraft.installDirectory;
    renderSetupInspection(setupInspection);
  }
  if (setupStep === 1) {
    setupSteamUsername.value = setupDraft.steamUsername;
    setupGameLanguage.value = setupDraft.steamLanguage;
    updateLanguageWarnings();
    document.querySelectorAll<HTMLInputElement>('input[name="setup-auth-method"]').forEach((input) => {
      input.checked = input.value === setupDraft.authMethod;
    });
  }
  if (protonStep && setupStep === 2) {
    renderProtonOptions(setupProtonVersion, setupDraft.protonVersion);
    setupProtonCustom.value = setupDraft.protonCustomPath;
    const status = snapshot?.proton ?? null;
    element("#setup-proton-hint").textContent = status?.ready
      ? "Proton is already prepared"
      : protonCatalogError ?? (setupProtonCustom.value.trim()
        ? "Your custom Proton folder will be used"
        : "Downloaded from GitHub and prepared when setup finishes");
    const ready = Boolean(status?.ready);
    element<HTMLElement>("#setup-proton-status").hidden = !ready;
  }
  if (setupStep === REVIEW_STEP) {
    setupView.dataset.installDirectory = setupDraft.installDirectory;
    setupView.dataset.steamUsername = setupDraft.steamUsername;
    setupView.dataset.steamLanguage = setupDraft.steamLanguage;
    setupView.dataset.authMethod = setupDraft.authMethod;
    element("#setup-review-title").textContent = existingGame ? "Existing installation found" : "Ready to install";
    element("#setup-review-description").textContent = existingGame
      ? "DepotDownloader will verify the selected Destiny 2 files, download anything missing, then install Sunrise."
      : "Confirm the destination and Steam account before downloading.";
    element("#setup-review-directory").textContent = setupDraft.installDirectory || "—";
    element("#setup-review-game-files").textContent = existingGame
      ? "Verify existing Destiny 2 files"
      : "Download Destiny 2";
    element("#setup-review-language").textContent = languageLabel(setupDraft.steamLanguage);
    element("#setup-review-username").textContent = setupDraft.steamUsername || "—";
    element("#setup-review-auth").textContent = setupDraft.authMethod === "qr" ? "QR code" : "Steam Guard code";
    element<HTMLElement>("#setup-review-proton-row").hidden = !protonStep;
    element("#setup-review-proton").textContent = setupDraft.protonCustomPath
      ? `Custom: ${setupDraft.protonCustomPath}`
      : setupDraft.protonVersion || "Managed GE-Proton";
    element("#setup-review-space").textContent = existingGame
      ? "Approximately 5 GiB free"
      : "Approximately 110 GiB";
    element("#setup-review-release").textContent = snapshot?.latestRelease?.tag ?? "Latest available";
  }
}

function openSetup() {
  closeSettings();
  setupDraft = {
    installDirectory: installDirectory.value.trim(),
    steamUsername: steamUsername.value.trim(),
    steamLanguage: snapshot?.preferences.steamLanguage ?? "english",
    authMethod: snapshot?.preferences.authMethod ?? "qr",
    ...crossoverPreferences(),
    protonVersion: snapshot?.preferences.protonVersion ?? "",
    protonSource: snapshot?.preferences.protonSource ?? "managed",
    protonCustomPath: snapshot?.preferences.protonCustomPath ?? "",
    umuSource: snapshot?.preferences.umuSource ?? "managed",
    protonPrefix: snapshot?.preferences.protonPrefix ?? "",
    launchOptions: snapshot?.preferences.launchOptions ?? "",
    gameId: snapshot?.preferences.gameId ?? "sunrise",
    envOverrides: snapshot?.preferences.envOverrides ?? "",
    protonDebugLogging: snapshot?.preferences.protonDebugLogging ?? false,
  };
  setupInspection = snapshot?.installation ?? null;
  setupLanguageFolder = "";
  setupStep = 0;
  renderSetupStep();
  setOverlay(setupView, true);
  void inspectSetupDirectory(setupDraft.installDirectory);
  void loadProtonReleases();
  window.requestAnimationFrame(() => setupInstallDirectory.focus());
}

function closeSetup() {
  setOverlay(setupView, false);
}

async function chooseInstallDirectory(fallback: string) {
  return inTauri
    ? await open({ directory: true, multiple: false, title: "Choose a Sunrise installation folder" })
    : fallback;
}

async function advanceSetup() {
  if (setupStep === 0) {
    if (!setupInstallDirectory.value.trim()) {
      showToast("Choose an installation folder to continue.", "error");
      setupInstallDirectory.focus();
      return;
    }
    setupDraft.installDirectory = setupInstallDirectory.value.trim();
    const installation = await inspectSetupDirectory(setupDraft.installDirectory, true);
    if (!installation) return;
    if (installation.steamLanguage && setupLanguageFolder !== setupDraft.installDirectory) {
      setupDraft.steamLanguage = installation.steamLanguage;
    }
    setupLanguageFolder = setupDraft.installDirectory;
    setupStep = 1;
    renderSetupStep();
    window.requestAnimationFrame(() => setupSteamUsername.focus());
    return;
  }
  if (setupStep === 1) {
    if (!setupSteamUsername.value.trim()) {
      showToast("Enter the Steam account name that owns Destiny 2.", "error");
      setupSteamUsername.focus();
      return;
    }
    setupDraft.steamUsername = setupSteamUsername.value.trim();
    setupDraft.steamLanguage = setupGameLanguage.value;
    setupDraft.authMethod = element<HTMLInputElement>('input[name="setup-auth-method"]:checked').value as AuthMethod;
    setupStep = isLinux() ? 2 : REVIEW_STEP;
    renderSetupStep();
    if (setupStep === REVIEW_STEP) window.requestAnimationFrame(() => element<HTMLElement>("#setup-review-title").focus());
    return;
  }
  if (isLinux() && setupStep === 2) {
    setupDraft.protonVersion = setupProtonVersion.value;
    setupDraft.protonCustomPath = setupProtonCustom.value.trim();
    setupDraft.protonSource = setupDraft.protonCustomPath ? "custom" : "managed";
    setupStep = REVIEW_STEP;
    renderSetupStep();
    window.requestAnimationFrame(() => element<HTMLElement>("#setup-review-title").focus());
    return;
  }

  const setupPreferences: Preferences = {
    installDirectory: setupView.dataset.installDirectory ?? setupDraft.installDirectory,
    steamUsername: setupView.dataset.steamUsername ?? setupDraft.steamUsername,
    steamLanguage: setupView.dataset.steamLanguage ?? setupDraft.steamLanguage,
    authMethod: (setupView.dataset.authMethod as AuthMethod | undefined) ?? setupDraft.authMethod,
    crossoverBottle: setupDraft.crossoverBottle,
    crossoverBackend: setupDraft.crossoverBackend,
    protonVersion: setupDraft.protonVersion,
    protonSource: setupDraft.protonSource,
    protonCustomPath: setupDraft.protonCustomPath,
    umuSource: setupDraft.umuSource,
    protonPrefix: setupDraft.protonPrefix,
    launchOptions: setupDraft.launchOptions,
    gameId: setupDraft.gameId,
    envOverrides: setupDraft.envOverrides,
    protonDebugLogging: setupDraft.protonDebugLogging,
  };
  installDirectory.value = setupPreferences.installDirectory;
  steamUsername.value = setupPreferences.steamUsername;
  gameLanguage.value = setupPreferences.steamLanguage;
  // run_operation only persists the install-related fields; save the Proton
  // choice explicitly first so it survives and is ready to hand to
  // completeLinuxProtonSetup below.
  try {
    await invokeCommand("save_preferences", { preferences: setupPreferences });
  } catch (error) {
    showToast(String(error), "error");
  }
  closeSetup();
  await runOperation(
    "install",
    setupPreferences,
    isLinux() ? (onEvent) => completeLinuxProtonSetup(setupPreferences, onEvent) : undefined,
  );
}

function hideAuth() {
  window.clearTimeout(authHideTimer);
  authSessionReady = false;
  activeAuthPrompt = null;
  authLoadingPanel.classList.remove("complete");
  setOverlay(authView, false);
}

function updateAuthMethodSwitch() {
  document.querySelectorAll<HTMLButtonElement>("[data-auth-switch]").forEach((button) => {
    button.classList.toggle("active", button.dataset.authSwitch === activeAuthMethod);
  });
}

function showAuthLoading(title: string, detail: string, status: string) {
  window.clearTimeout(authHideTimer);
  authLoadingPanel.hidden = false;
  authLoadingPanel.classList.remove("complete");
  authLoadingTitle.textContent = title;
  authLoadingDetail.textContent = detail;
  element("#auth-status").textContent = status;
  element<HTMLElement>("#qr-auth-panel").hidden = true;
  authResponseForm.hidden = true;
  authPrivacy.hidden = true;
}

function prepareAuth(method: AuthMethod) {
  activeAuthMethod = method;
  activeAuthPrompt = null;
  authSessionReady = false;
  passwordPromptSeen = false;
  updateAuthMethodSwitch();
  element("#auth-title").textContent = "Connecting to Steam";
  element("#auth-description").textContent = method === "qr"
    ? "DepotDownloader will reuse a saved Steam session or show a QR code when sign-in is needed."
    : "DepotDownloader will reuse a saved Steam session or request your password and Steam Guard code.";
  authResponse.disabled = true;
  authResponse.value = "";
  authQrCode.setAttribute("hidden", "");
  element("#qr-placeholder").hidden = false;
  showAuthLoading(
    "Preparing DepotDownloader",
    "Checking the local tool before opening a secure Steam connection.",
    "Starting Steam authentication…",
  );
  setOverlay(settingsView, false);
  setOverlay(setupView, false);
  setOverlay(authView, true);
}

function renderQrCode(rows: string[]) {
  const svg = authQrCode;
  const parsedRows = rows.map((row) => {
    const characters = [...row];
    const modules: boolean[] = [];
    for (let index = 0; index < characters.length; index += 2) {
      modules.push(characters.slice(index, index + 2).some((character) => character !== " "));
    }
    return modules;
  });
  const width = Math.max(1, ...parsedRows.map((row) => row.length));
  const height = Math.max(1, parsedRows.length);
  const path: string[] = [];
  parsedRows.forEach((row, y) => row.forEach((dark, x) => {
    if (dark) path.push(`M${x} ${y}h1v1h-1z`);
  }));
  const namespace = "http://www.w3.org/2000/svg";
  const background = document.createElementNS(namespace, "rect");
  background.setAttribute("width", String(width));
  background.setAttribute("height", String(height));
  background.setAttribute("fill", "#fff");
  const modules = document.createElementNS(namespace, "path");
  modules.setAttribute("d", path.join(""));
  modules.setAttribute("fill", "#090b11");
  svg.setAttribute("viewBox", `0 0 ${width} ${height}`);
  svg.replaceChildren(background, modules);
  svg.removeAttribute("hidden");
  element("#qr-placeholder").hidden = true;
}

function showQrCode(rows: string[]) {
  window.clearTimeout(authHideTimer);
  activeAuthMethod = "qr";
  activeAuthPrompt = null;
  authSessionReady = false;
  authLoadingPanel.hidden = true;
  authLoadingPanel.classList.remove("complete");
  updateAuthMethodSwitch();
  element("#auth-title").textContent = "Scan with Steam";
  element("#auth-description").textContent = "Approve this sign-in from the Steam Mobile app.";
  element("#auth-status").textContent = "QR code ready · waiting for approval";
  element<HTMLElement>("#qr-auth-panel").hidden = false;
  authResponseForm.hidden = true;
  authPrivacy.hidden = true;
  renderQrCode(rows);
  setOverlay(authView, true);
}

function showAuthPrompt(kind: "password" | "twoFactor" | "emailCode", message: string) {
  window.clearTimeout(authHideTimer);
  authSessionReady = false;
  const password = kind === "password";
  const reusedSession = !password && !passwordPromptSeen;
  if (password) passwordPromptSeen = true;
  activeAuthPrompt = kind;
  authLoadingPanel.hidden = true;
  authLoadingPanel.classList.remove("complete");
  element("#auth-title").textContent = password ? "Enter Steam password" : "Enter Steam Guard code";
  element("#auth-description").textContent = reusedSession
    ? "DepotDownloader reused your saved Steam session, so only a Steam Guard code is required."
    : message;
  element("#auth-response-label").textContent = password ? "Steam password" : "Steam Guard code";
  element("#auth-status").textContent = reusedSession
    ? "Saved Steam session found · waiting for your code"
    : "Waiting for your response";
  element<HTMLElement>("#qr-auth-panel").hidden = true;
  authResponseForm.hidden = false;
  authPrivacy.hidden = false;
  authResponse.type = password ? "password" : "text";
  authResponse.inputMode = "text";
  authResponse.autocapitalize = password ? "off" : "characters";
  authResponse.autocomplete = password ? "current-password" : "one-time-code";
  authResponse.readOnly = false;
  authResponse.disabled = false;
  authResponse.tabIndex = 0;
  authResponse.setAttribute("aria-disabled", "false");
  authResponse.value = "";
  authResponseForm.querySelector<HTMLButtonElement>("button")!.disabled = false;
  setOverlay(authView, true);
  window.requestAnimationFrame(() => authResponse.focus());
}

async function restartAuthentication(method: AuthMethod) {
  if (!operationRunning || method === activeAuthMethod) return;
  pendingAuthRestart = method;
  prepareAuth(method);
  showAuthLoading(
    "Restarting DepotDownloader",
    "Stopping the current sign-in attempt and switching authentication methods.",
    "Restarting with this sign-in method…",
  );
  element("#operation-message").textContent = "Restarting Steam authentication…";
  const cancelled = await invokeCommand<boolean>("cancel_operation");
  if (!cancelled) {
    pendingAuthRestart = null;
    showToast("Steam authentication could not be restarted because DepotDownloader is no longer running.", "error");
  }
}

function resetOperation(kind: OperationKind) {
  activeOperationKind = kind;
  updateOperationStep("preflight", "Starting preflight checks…");
  element("#operation-message").textContent = "Starting preflight checks…";
  element("#operation-percent").textContent = "0%";
  element("#progress-fill").style.width = "0%";
  element<HTMLElement>(".progress-track").setAttribute("aria-valuenow", "0");
  window.clearTimeout(operationHideTimer);
  operationStrip.hidden = false;
  consoleOutput.textContent = "";
  element("#operation-notice").textContent = kind !== "install" && kind !== "repair"
    ? ""
    : activeAuthMethod === "qr"
      ? "The first depot will ask you to scan a Steam QR code."
      : "DepotDownloader will ask for your password and Steam Guard code.";
}

function updateOperationStep(stage: string, message: string) {
  let label = "INSTALLING SUNRISE";

  const downloadsGame = activeOperationKind === "install" || activeOperationKind === "repair";

  if (activeOperationKind === "missions") {
    label = "UPDATING MISSIONS";
  } else if (stage === "missions") {
    label = "INSTALLING MISSIONS";
  } else if (stage === "proton") {
    label = "PREPARING PROTON";
  } else if (stage === "depots") {
    label = message.trimStart().toLowerCase().startsWith("validating")
      ? "VALIDATING DESTINY 2"
      : "DOWNLOADING DESTINY 2";
  } else if (stage === "tool" || (stage === "preflight" && downloadsGame)) {
    label = "DOWNLOADING DEPOTDOWNLOADER";
  } else if (stage === "repair") {
    label = "REPAIRING SUNRISE";
  } else if (stage === "language") {
    label = "UPDATING GAME LANGUAGE";
  }

  element("#operation-kind").textContent = label;
}

/** Restarts the visible progress bar for a new phase of the same operation
 * strip (e.g. moving from the Sunrise install into Proton setup) without
 * wiping the console log the user has been watching. */
function resetOperationProgress(stage: string, message: string) {
  updateOperationStep(stage, message);
  element("#operation-message").textContent = message;
  element("#operation-percent").textContent = "0%";
  element("#progress-fill").style.width = "0%";
  element<HTMLElement>(".progress-track").setAttribute("aria-valuenow", "0");
  appendConsole(`[${stage}] ${message}\n`, "notice");
}

function appendConsole(text: string, stream = "stdout") {
  if (consoleOutput.textContent === "Waiting for an operation…") consoleOutput.textContent = "";
  const line = document.createElement("span");
  line.className = `console-${stream}`;
  line.textContent = text;
  consoleOutput.append(line);
  consoleOutput.scrollTop = consoleOutput.scrollHeight;
}

function updateAuthLoadingStage(stage: string, message: string) {
  if (authSessionReady || authView.hidden || authLoadingPanel.hidden || activeAuthPrompt) return;

  if (stage === "preflight") {
    showAuthLoading("Preparing installation", message, "Getting Steam sign-in ready…");
  } else if (stage === "tool") {
    showAuthLoading("Preparing DepotDownloader", message, "Starting DepotDownloader…");
  } else if (stage === "depots") {
    showAuthLoading(
      "Checking saved Steam session",
      "DepotDownloader is connecting to Steam. You will only be asked to sign in if needed.",
      "Checking cached authentication…",
    );
  }
}

function handleOperationEvent(message: OperationEvent) {
  if (message.event === "qrCode") {
    showQrCode(message.rows);
    return;
  }
  if (message.event === "authPrompt") {
    showAuthPrompt(message.kind, message.message);
    return;
  }
  if (message.event === "authComplete") {
    authSessionReady = true;
    activeAuthPrompt = null;
    showAuthLoading(
      "Steam session ready",
      "Authentication is complete. Continuing the depot download.",
      "Steam authenticated · continuing…",
    );
    authLoadingPanel.classList.add("complete");
    authHideTimer = window.setTimeout(() => {
      if (!authView.hidden && authSessionReady) hideAuth();
    }, 650);
    element("#operation-notice").textContent = "Steam authenticated · continuing the depot download.";
    return;
  }
  if (message.event === "terminal") {
    appendConsole(message.text, message.stream);
    return;
  }
  if (message.event === "notice") {
    element("#operation-notice").textContent = message.message;
    appendConsole(`[launcher] ${message.message}\n`, "notice");
    return;
  }
  const percent = Math.max(0, Math.min(100, message.percent));
  const percentLabel = Number.isInteger(percent) ? String(percent) : percent.toFixed(2);
  updateAuthLoadingStage(message.stage, message.message);
  updateOperationStep(message.stage, message.message);
  element("#operation-message").textContent = message.message;
  element("#operation-percent").textContent = `${percentLabel}%`;
  element("#progress-fill").style.width = `${percent}%`;
  element<HTMLElement>(".progress-track").setAttribute("aria-valuenow", percent.toFixed(1));
  if (message.stage !== "depots") {
    const line = `[${message.stage}] ${message.message}`;
    if (lastConsoleLine !== line) {
      lastConsoleLine = line;
      appendConsole(`${line}\n`, "notice");
    }
  }
}

/** Downloads/prepares Proton so the setup wizard's Proton step actually does
 * what it says, instead of only saving preferences. Linux-only; a no-op
 * everywhere else. Runs as a continuation of the install operation, using the
 * same progress strip and event channel. */
async function completeLinuxProtonSetup(preferences: Preferences, onEvent: EventSink) {
  if (!isLinux()) return;

  const status = await invokeCommand<ProtonStatus | null>("get_proton_status", { preferences });
  if (status?.ready) {
    if (snapshot) snapshot = { ...snapshot, proton: status };
    return;
  }

  const tag = preferences.protonSource === "managed" ? preferences.protonVersion.trim() : "";
  const alreadyInstalled = tag !== "" && (status?.installedTags ?? []).includes(tag);
  if (tag && !alreadyInstalled) {
    resetOperationProgress("proton", `Downloading GE-Proton ${tag}…`);
    await invokeCommand("install_proton", { tag, onEvent });
  }

  resetOperationProgress("proton", "Preparing the Proton environment…");
  await invokeCommand("initialize_proton_prefix", { onEvent });

  const refreshed = await invokeCommand<ProtonStatus | null>("get_proton_status", { preferences });
  if (refreshed && snapshot) snapshot = { ...snapshot, proton: refreshed };
}

async function runOperation(
  kind: OperationKind,
  requestedPreferences?: Preferences,
  afterSuccess?: (onEvent: EventSink) => Promise<void>,
) {
  if (operationRunning) return;
  let operationCompleted = false;
  const operationPreferences: Preferences = requestedPreferences ?? {
    ...(snapshot?.preferences ?? setupDraft),
    installDirectory: installDirectory.value.trim(),
    steamUsername: steamUsername.value.trim(),
    steamLanguage: gameLanguage.value,
    authMethod: snapshot?.preferences.authMethod ?? "qr",
    ...crossoverPreferences(),
  };
  activeAuthMethod = operationPreferences.authMethod;
  operationRunning = true;
  operationCancellationRequested = false;
  resetOperation(kind);
  closeSettings();
  if (kind === "install" || kind === "repair") prepareAuth(operationPreferences.authMethod);
  element<HTMLButtonElement>("#cancel-action").disabled = false;
  primaryAction.disabled = true;
  repairAction.disabled = true;
  missionsAction.disabled = true;
  launchAction.disabled = true;
  installDirectory.disabled = true;
  steamUsername.disabled = true;
  gameLanguage.disabled = true;
  element<HTMLButtonElement>("#browse-directory").disabled = true;

  const onEvent: EventSink = inTauri ? new Channel<OperationEvent>() : {};
  onEvent.onmessage = handleOperationEvent;
  try {
    const result = await invokeCommand<{ changed: boolean; releaseTag: string; message: string }>("run_operation", {
      request: {
        kind,
        installDirectory: operationPreferences.installDirectory,
        steamUsername: operationPreferences.steamUsername,
        steamLanguage: operationPreferences.steamLanguage,
        authMethod: operationPreferences.authMethod,
      },
      onEvent,
    });
    operationCompleted = true;
    if (!operationCancellationRequested) showToast(result.message);
    if (afterSuccess && !operationCancellationRequested) {
      try {
        await afterSuccess(onEvent);
      } catch (protonError) {
        showToast(`Sunrise installed, but Proton setup needs attention: ${protonError}`, "error");
      }
    }
  } catch (error) {
    const message = String(error);
    if (pendingAuthRestart) {
      element("#operation-message").textContent = "Restarting Steam authentication…";
      appendConsole("\n[launcher] Restarting DepotDownloader with a different sign-in method.\n", "notice");
    } else if (operationCancellationRequested) {
      element("#operation-message").textContent = "Installation cancelled.";
      appendConsole("\n[launcher] Installation cancelled.\n", "notice");
    } else {
      element("#operation-message").textContent = message;
      appendConsole(`\n[error] ${message}\n`, "stderr");
      showToast(message, "error");
    }
  } finally {
    operationRunning = false;
    if (pendingAuthRestart) {
      const restartPreferences = { ...operationPreferences, authMethod: pendingAuthRestart };
      pendingAuthRestart = null;
      window.setTimeout(() => runOperation(kind, restartPreferences), 0);
      return;
    }
    hideAuth();
    element<HTMLButtonElement>("#cancel-action").disabled = true;
    installDirectory.disabled = false;
    steamUsername.disabled = false;
    gameLanguage.disabled = false;
    installDirectory.value = operationPreferences.installDirectory;
    steamUsername.value = operationPreferences.steamUsername;
    gameLanguage.value = operationPreferences.steamLanguage;
    element<HTMLButtonElement>("#browse-directory").disabled = false;
    await loadSnapshot(true);
    if (operationCompleted && canLaunch()) {
      primaryMode = "launch";
      primaryLabel.textContent = "PLAY";
      primaryAction.disabled = false;
    }
    if (operationCancellationRequested || operationCompleted) {
      operationStrip.hidden = true;
      operationCancellationRequested = false;
    } else {
      operationHideTimer = window.setTimeout(() => {
        if (!operationRunning) {
          operationStrip.hidden = true;
        }
      }, 4000);
    }
  }
}

function stopGamePolling() {
  window.clearInterval(gameStatusTimer);
  gameStatusTimer = undefined;
}

function markGameStopped() {
  if (!gameLaunching) return;
  gameLaunching = false;
  stopGamePolling();
  if (snapshot) renderSnapshot(snapshot);
}

async function pollGameStatus() {
  try {
    if (!(await invokeCommand<boolean>("game_status"))) markGameStopped();
  } catch {
    // A transient failure must not fake a stop; keep showing RUNNING.
  }
}

function startGamePolling() {
  stopGamePolling();
  gameStatusTimer = window.setInterval(() => void pollGameStatus(), GAME_POLL_MS);
}

async function launch() {
  if (gameLaunching || operationRunning) return;

  if (isLinux() && !protonReady()) {
    openSettings();
    showToast("Prepare Proton in Settings before launching.", "error");
    return;
  }

  gameLaunching = true;
  if (snapshot) renderSnapshot(snapshot);

  try {
    await invokeCommand("launch_game", { installDirectory: installDirectory.value.trim() });
    startGamePolling();
  } catch (error) {
    markGameStopped();
    showToast(String(error), "error");
  }
}

async function closeLauncher() {
  stopGamePolling();
  stopProtonReconcile();
  if (operationRunning) {
    await invokeCommand<boolean>("cancel_operation").catch(() => false);
  }
  await appWindow?.close();
}

window.addEventListener("DOMContentLoaded", () => {
  element(".launcher-shell").addEventListener("mousedown", (event) => {
    if (!appWindow || event.button !== 0 || event.buttons !== 1) return;
    const target = event.target;
    if (!(target instanceof Element)) return;
    if (target.closest("button, input, textarea, select, option, a, label, summary, [contenteditable='true']")) return;
    void appWindow.startDragging().catch((error) => console.error("Could not start dragging the launcher window", error));
  });
  element("#settings-action").addEventListener("click", openSettings);
  element("#close-settings").addEventListener("click", closeSettings);
  element("[data-close-settings]").addEventListener("click", closeSettings);
  element("#window-minimize").addEventListener("click", () => appWindow?.minimize());
  element("#window-maximize").addEventListener("click", () => appWindow?.toggleMaximize());
  element("#window-close").addEventListener("click", closeLauncher);
  element(".launcher-header").addEventListener("dblclick", (event) => {
    if (appWindow && !(event.target as HTMLElement).closest(".window-button")) appWindow.toggleMaximize();
  });
  element("#open-project").addEventListener("click", () => {
    if (inTauri) openUrl("https://github.com/stanuwu/Sunrise");
    else window.open("https://github.com/stanuwu/Sunrise", "_blank", "noopener");
  });
  element("#browse-directory").addEventListener("click", async () => {
    const selected = await chooseInstallDirectory("C:\\Games\\Project Sunrise");
    if (typeof selected === "string") {
      installDirectory.value = selected;
      await saveAndInspect();
    }
  });
  element("#setup-browse-directory").addEventListener("click", async () => {
    const selected = await chooseInstallDirectory("C:\\Games\\Project Sunrise");
    if (typeof selected === "string") {
      setupInstallDirectory.value = selected;
      setupDraft.installDirectory = selected;
      await inspectSetupDirectory(selected);
    }
  });
  setupInstallDirectory.addEventListener("input", queueSetupInspection);
  element("#close-setup").addEventListener("click", closeSetup);
  element("[data-close-setup]").addEventListener("click", closeSetup);
  document.querySelectorAll<HTMLButtonElement>("[data-auth-switch]").forEach((button) => {
    button.addEventListener("click", () => restartAuthentication(button.dataset.authSwitch as AuthMethod));
  });
  document.querySelectorAll<HTMLInputElement>('input[name="setup-auth-method"]').forEach((input) => {
    input.addEventListener("change", () => {
      if (input.checked) setupDraft.authMethod = input.value as AuthMethod;
    });
  });
  setupGameLanguage.addEventListener("change", () => {
    setupDraft.steamLanguage = setupGameLanguage.value;
    updateLanguageWarnings();
  });
  setupBack.addEventListener("click", () => {
    setupStep = Math.max(0, setupStep - 1);
    renderSetupStep();
  });
  setupNext.addEventListener("click", advanceSetup);
  installDirectory.addEventListener("input", () => {
    window.clearTimeout(statusTimer);
    statusTimer = window.setTimeout(saveAndInspect, 450);
  });
  steamUsername.addEventListener("change", saveAndInspect);
  gameLanguage.addEventListener("change", () => {
    updateLanguageWarnings();
    void saveAndInspect();
  });
  element("#refresh-status").addEventListener("click", () => loadSnapshot(true));
  element("#crossover-refresh").addEventListener("click", () => loadSnapshot(true));
  const refreshCrossover = async () => {
    await saveAndInspect();
    if (!snapshot || snapshot.platform.os !== "macos") return;
    try {
      const status = await invokeCommand<CrossOverStatus | null>("get_crossover_status", { preferences: snapshot.preferences });
      renderSnapshot({ ...snapshot, crossover: status, platform: { ...snapshot.platform, canLaunch: Boolean(status?.ready) } });
    } catch (error) {
      showToast(String(error), "error");
    }
  };
  crossoverBottle.addEventListener("change", refreshCrossover);
  crossoverBackend.addEventListener("change", refreshCrossover);
  crossoverPrepare.addEventListener("click", async () => {
    if (operationRunning || gameLaunching || !snapshot) return;
    crossoverPrepare.disabled = true;
    crossoverPrepare.textContent = "Preparing…";
    const preferences: Preferences = { ...snapshot.preferences, ...crossoverPreferences() };
    try {
      const status = await invokeCommand<CrossOverStatus>("prepare_crossover", { preferences });
      renderSnapshot({ ...snapshot, preferences, crossover: status, platform: { ...snapshot.platform, canLaunch: status.ready } });
      showToast(status.ready ? "The CrossOver bottle is ready." : "The bottle could not be prepared.", status.ready ? "info" : "error");
    } catch (error) {
      if (snapshot) renderSnapshot(snapshot);
      showToast(String(error), "error");
    }
  });
  primaryAction.addEventListener("click", () => {
    if (primaryMode === "install") {
      openSetup();
      return;
    }
    if (!installDirectory.value.trim() && primaryMode !== "launch") {
      openSettings();
      showToast("Choose an installation folder first.");
      return;
    }
    if (primaryMode === "launch") void launch();
    else runOperation(primaryMode);
  });
  repairAction.addEventListener("click", () => runOperation("repair"));
  missionsAction.addEventListener("click", () => runOperation("missions"));
  launchAction.addEventListener("click", () => {
    closeSettings();
    void launch();
  });
  element("#cancel-action").addEventListener("click", async () => {
    operationCancellationRequested = true;
    try {
      if (await invokeCommand<boolean>("cancel_operation")) {
        element("#operation-message").textContent = "Cancelling safely…";
      } else {
        operationCancellationRequested = false;
      }
    } catch (error) {
      operationCancellationRequested = false;
      showToast(String(error), "error");
    }
  });
  element("#auth-cancel").addEventListener("click", async () => {
    pendingAuthRestart = null;
    operationCancellationRequested = true;
    try {
      if (await invokeCommand<boolean>("cancel_operation")) {
        element("#auth-status").textContent = "Cancelling installation…";
      } else {
        operationCancellationRequested = false;
      }
    } catch (error) {
      operationCancellationRequested = false;
      showToast(String(error), "error");
    }
  });
  authResponseForm.addEventListener("submit", async (event) => {
    event.preventDefault();
    if (!authResponse.value) return;
    const submittedPrompt = activeAuthPrompt;
    const response = authResponse.value;
    authResponse.value = "";
    authResponse.disabled = true;
    authResponseForm.querySelector<HTMLButtonElement>("button")!.disabled = true;
    try {
      const sent = await invokeCommand<boolean>("send_terminal_input", { input: response });
      if (!sent) {
        authResponse.disabled = false;
        authResponseForm.querySelector<HTMLButtonElement>("button")!.disabled = false;
        showToast("DepotDownloader is not waiting for a response.", "error");
        return;
      }
      if (!authSessionReady) {
        element("#auth-status").textContent = submittedPrompt === "password"
          ? "Password sent · waiting for Steam Guard…"
          : "Code sent · waiting for Steam…";
      }
    } catch (error) {
      authResponse.disabled = false;
      authResponseForm.querySelector<HTMLButtonElement>("button")!.disabled = false;
      showToast(String(error), "error");
    }
  });
  authResponseForm.addEventListener("pointerdown", (event) => {
    const target = event.target as HTMLElement;
    if (!authResponse.disabled && target.closest("label")) {
      window.requestAnimationFrame(() => authResponse.focus());
    }
  });
  element("#clear-console").addEventListener("click", () => consoleOutput.replaceChildren());
  element("#proton-refresh").addEventListener("click", () => loadProtonReleases(true));
  element("#proton-toggle").addEventListener("click", () => {
    const tag = protonVersion.value;
    if (!tag) return;
    const installed = (snapshot?.proton?.installedTags ?? []).includes(tag);
    if (installed) void runProtonCommand("remove_proton", { tag }, "remove");
    else void runProtonCommand("install_proton", { tag }, "install");
  });
  element("#proton-prepare").addEventListener("click", () => {
    void runProtonCommand("initialize_proton_prefix", {}, "prepare");
  });
  protonProgressCancel.addEventListener("click", async () => {
    if (activeProtonOperation === "remove") return;
    protonProgressCancel.disabled = true;
    protonProgressMessage.textContent = "Cancelling safely…";
    try {
      if (!(await invokeCommand<boolean>("cancel_operation"))) {
        protonProgressCancel.disabled = false;
        showToast("The Proton operation could not be cancelled.", "error");
      }
    } catch (error) {
      protonProgressCancel.disabled = false;
      showToast(String(error), "error");
    }
  });
  element("#proton-browse").addEventListener("click", async () => {
    const selected = await chooseInstallDirectory("/home/user/.steam/steam/compatibilitytools.d");
    if (typeof selected === "string") {
      protonCustomPath.value = selected;
      await saveProtonPreferences();
    }
  });
  element("#setup-proton-browse").addEventListener("click", async () => {
    const selected = await chooseInstallDirectory("/home/user/.steam/steam/compatibilitytools.d");
    if (typeof selected === "string") setupProtonCustom.value = selected;
  });
  protonVersion.addEventListener("change", () => void saveProtonPreferences());
  protonCustomPath.addEventListener("change", () => void saveProtonPreferences());
  protonUmuSource.addEventListener("change", () => void saveProtonPreferences());
  launchOptions.addEventListener("change", () => void saveProtonPreferences());
  protonGameId.addEventListener("change", () => void saveProtonPreferences());
  protonEnv.addEventListener("change", () => void saveProtonPreferences());
  protonDebug.addEventListener("change", () => void saveProtonPreferences());
  window.addEventListener("keydown", (event) => {
    if (event.key !== "Escape") return;
    if (!authView.hidden) return;
    if (!setupView.hidden) closeSetup();
    else if (!settingsView.hidden) closeSettings();
  });
  void loadSnapshot().then(() => loadProtonReleases());
});
