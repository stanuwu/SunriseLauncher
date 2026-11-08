# Project Sunrise Launcher

Official launcher for [Project Sunrise](https://github.com/stanuwu/Sunrise), the Destiny 2 preservation mod.

Made by [zeex64](https://github.com/zeex64), maintained by [stanuwu](https://github.com/stanuwu)

This repository is a rewrite of [SunriseInstaller](https://github.com/stanuwu/SunriseInstaller). The interface, filesystem access, downloads, hashing, and child-process orchestration are split across a TypeScript frontend and a Rust backend. Steam credentials are handled by DepotDownloader and are never persisted by the launcher.

## Platform support

| Platform | Install / repair / update | Launch |
| --- | --- | --- |
| Windows x64 / ARM64 | Supported | Supported |
| Linux x64 / ARM64 | Experimental | Through GE-Proton and umu-launcher |
| macOS x64 / ARM64 | Experimental | Through CrossOver 26 or newer (D3DMetal or DXMT) |

"Cross-platform" applies to the installer. Sunrise remains a Windows DLL for a Windows build of Destiny 2.

On macOS the launcher runs the game inside a CrossOver bottle it creates for the purpose, with
the graphics backend chosen in Settings. Deny CrossOver the microphone in System Settings before
the first launch, and play in fullscreen; see the CrossOver section of Settings for the reasons.

On Linux the launcher runs the game through Proton with umu-launcher. It downloads pinned
builds of both, GE-Proton11-11 and umu-launcher 1.4.4, and checks each against a SHA-256 in its
code. Settings can use a Proton folder you already have, or `umu-run` from your `PATH` instead.

## Current installer flow

1. Validate the target folder, write access, and free space.
2. Download the pinned DepotDownloader 3.4.0 build for the host OS/architecture and check its SHA-256.
3. Stream DepotDownloader output into the in-app console for QR/Steam Guard authentication.
4. Download the pinned shared depot and the selected language depot.
5. Download `steam_api64.dll` from the latest Sunrise GitHub release.
6. Install the latest commit of [SunriseMissions](https://github.com/stanuwu/SunriseMissions) into `bin/x64/Sunrise/scripts`. A scripts folder that is a git checkout is left alone.
7. Verify GitHub's SHA-256 digest, preserve rollback/original copies, and install the DLL.
8. Save a compatible `.sunrise/install-state.json` for update and integrity checks.

Settings has an **Update missions** button that replaces the scripts folder with the latest missions at any time.

The shared Windows depot is `1085661`, manifest `7180122903232116872`.
One language depot is selected during Install or Repair:

| Language | Steam language | Depot | Manifest |
| --- | --- | ---: | ---: |
| English | `english` | `1085662` | `2210332166360342287` |
| French | `french` | `1085663` | `2934940253687559290` |
| German | `german` | `1085664` | `2207989571290186153` |
| Italian | `italian` | `1085665` | `6668232053215128229` |
| Japanese | `japanese` | `1085666` | `7430022397683116838` |
| Portuguese (Brazil) | `brazilian` | `1085667` | `9037238175838085860` |
| Spanish (Spain) | `spanish` | `1085668` | `3424833900894552134` |
| Russian | `russian` | `1085669` | `4539277942371480381` |
| Polish | `polish` | `1085670` | `6407581507105256731` |
| Chinese (Simplified) | `schinese` | `1085671` | `4397663774546719308` |
| Chinese (Traditional) | `tchinese` | `1085672` | `3906738704604711877` |
| Spanish (Latin America) | `latam` | `1085673` | `4773170998099699561` |
| Korean | `koreana` | `1085674` | `7148196199569436690` |

When a managed installation changes language, the launcher downloads the new
depot first, compares the old/shared/new manifests, and removes only files that
are unique to the previous language. It also updates
`bin/x64/Sunrise/settings.json` with the selected Steam language.

## Development

Requirements: Node.js, Rust, and the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) for your operating system.

```powershell
npm install
npm run tauri dev
```

The frontend can also be previewed without native commands. It automatically uses mock data under `npm run dev`; packaged Tauri builds always call the Rust backend.

### Checks

```powershell
npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo test --manifest-path src-tauri/Cargo.toml
```

### Package locally

```powershell
npm run tauri build
```

## Distribution notes

- Production Windows and macOS downloads should be code-signed before broad distribution.
- The GitHub workflow builds native installers from a `launcher-v*` tag and leaves the release as a draft for review.
- Test the complete Steam authentication/download flow on each target OS before marking a build stable; automated tests intentionally do not download the ~110 GiB game payload.

## License and credits

GPL-2.0-only. DepotDownloader is downloaded on demand as a separate GPL-2.0 program and is not linked into this application. Logo artwork is credited to Solus, matching the existing Sunrise installer attribution.
