//! Ment to be as simular as possible to the steam launch args
use std::collections::BTreeMap;
use std::path::Path;

use crate::error::{AppError, AppResult};
use crate::models::Preferences;

pub const STEAM_DLL_OVERRIDE: &str = "steam_api64=n,b";

const COMMAND_PLACEHOLDER: &str = "%command%";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchOptionStyle {
    Wrapper,
    Arguments,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchPlan {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: String,
    pub env: BTreeMap<String, String>,
    pub style: LaunchOptionStyle,
    pub wrappers: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchToken {
    pub text: String,
    pub quoted: bool,
}

pub fn tokenize(options: &str) -> AppResult<Vec<LaunchToken>> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut current_quoted = false;
    let mut started = false;
    let mut single = false;
    let mut double = false;
    let mut escaped = false;

    for character in options.chars() {
        if escaped {
            current.push(character);
            if single || double {
                current_quoted = true;
            }
            escaped = false;
            started = true;
            continue;
        }
        if double && character == '\\' {
            escaped = true;
            started = true;
            continue;
        }
        match character {
            '\'' if !double => {
                single = !single;
                started = true;
            }
            '"' if !single => {
                double = !double;
                started = true;
            }
            character if character.is_whitespace() && !single && !double => {
                if started {
                    tokens.push(LaunchToken {
                        text: std::mem::take(&mut current),
                        quoted: current_quoted,
                    });
                    current_quoted = false;
                    started = false;
                }
            }
            character => {
                current.push(character);
                if single || double {
                    current_quoted = true;
                }
                started = true;
            }
        }
    }

    if single || double {
        return Err(AppError::message(
            "The launch options have an unclosed quote.",
        ));
    }
    if started {
        tokens.push(LaunchToken {
            text: current,
            quoted: current_quoted,
        });
    }
    Ok(tokens)
}

fn env_assignment(token: &LaunchToken) -> Option<(String, String)> {
    if token.quoted {
        return None;
    }
    let (key, value) = token.text.split_once('=')?;
    let mut characters = key.chars();
    match characters.next() {
        Some(character) if character.is_ascii_alphabetic() || character == '_' => {}
        _ => return None,
    }
    if !characters.all(|character| character.is_ascii_alphanumeric() || character == '_') {
        return None;
    }
    Some((key.to_owned(), value.to_owned()))
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn shell_command(argv: &[String]) -> String {
    argv.iter()
        .map(|element| shell_quote(element))
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn build_argv(
    program: &str,
    game_args: &[String],
    options: &str,
) -> AppResult<(
    Vec<String>,
    LaunchOptionStyle,
    Vec<String>,
    Vec<(String, String)>,
)> {
    let base = {
        let mut argv = vec![program.to_owned()];
        argv.extend(game_args.iter().cloned());
        argv
    };

    let tokens = tokenize(options)?;
    let has_placeholder = tokens
        .iter()
        .any(|token| token.text.contains(COMMAND_PLACEHOLDER));

    let mut env_prefix = Vec::new();
    let mut cursor = 0;
    if has_placeholder {
        while let Some(assignment) = tokens.get(cursor).and_then(env_assignment) {
            env_prefix.push(assignment);
            cursor += 1;
        }
    }
    let tokens = &tokens[cursor..];

    let Some(position) = tokens
        .iter()
        .position(|token| token.text.contains(COMMAND_PLACEHOLDER))
    else {
        let mut argv = base;
        argv.extend(tokens.iter().map(|token| token.text.clone()));
        return Ok((argv, LaunchOptionStyle::Arguments, Vec::new(), env_prefix));
    };

    let placeholder = &tokens[position];
    if placeholder.quoted {
        let command = shell_command(&base);
        let splice = placeholder.text.replace(COMMAND_PLACEHOLDER, &command);
        let mut argv: Vec<String> = tokens[..position]
            .iter()
            .map(|token| token.text.clone())
            .collect();
        argv.push(splice.clone());
        argv.extend(
            tokens[position + 1..]
                .iter()
                .map(|token| token.text.clone()),
        );
        let mut wrappers: Vec<String> = tokens[..position]
            .iter()
            .map(|token| token.text.clone())
            .collect();
        wrappers.push(splice);
        return Ok((argv, LaunchOptionStyle::Wrapper, wrappers, env_prefix));
    }

    let mut wrappers: Vec<String> = tokens[..position]
        .iter()
        .map(|token| token.text.clone())
        .collect();
    let splice = placeholder.text.replace(COMMAND_PLACEHOLDER, program);
    wrappers.push(splice);

    let mut argv = wrappers.clone();
    argv.extend(game_args.iter().cloned());
    argv.extend(
        tokens[position + 1..]
            .iter()
            .map(|token| token.text.clone()),
    );
    Ok((argv, LaunchOptionStyle::Wrapper, wrappers, env_prefix))
}

pub fn merge_dll_overrides(user: &str) -> String {
    let mut entries = vec![STEAM_DLL_OVERRIDE.to_owned()];
    for part in user.split(';') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let Some((name, _)) = part.split_once('=') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case("steam_api64") {
            return user.trim().to_owned();
        }
        entries.push(part.to_owned());
    }
    entries.join(";")
}

pub fn parse_env_overrides(text: &str) -> AppResult<BTreeMap<String, String>> {
    let mut values = BTreeMap::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(AppError::message(format!(
                "Environment override line {} is not KEY=value.",
                index + 1
            )));
        };
        let key = key.trim();
        if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(AppError::message(format!(
                "Environment override line {} has an invalid name.",
                index + 1
            )));
        }
        values.insert(key.to_owned(), value.trim().to_owned());
    }
    Ok(values)
}

pub fn plan(
    umu_run: &Path,
    proton_path: &Path,
    prefix: &Path,
    game_executable: &Path,
    preferences: &Preferences,
) -> AppResult<LaunchPlan> {
    let program = umu_run.to_string_lossy().into_owned();
    let exe = game_executable.to_string_lossy().into_owned();
    let (argv, style, wrappers, env_prefix) =
        build_argv(&program, &[exe], &preferences.launch_options)?;

    let mut env = BTreeMap::new();

    env.extend(env_prefix);
    env.insert(
        "WINEPREFIX".to_owned(),
        prefix.to_string_lossy().into_owned(),
    );
    env.insert(
        "PROTONPATH".to_owned(),
        proton_path.to_string_lossy().into_owned(),
    );

    env.insert("PROTON_VERB".to_owned(), "waitforexitandrun".to_owned());
    env.insert("STORE".to_owned(), "none".to_owned());

    let game_id = preferences.game_id.trim();
    env.insert(
        "GAMEID".to_owned(),
        if game_id.is_empty() {
            "sunrise".to_owned()
        } else {
            game_id.to_owned()
        },
    );

    env.extend(parse_env_overrides(&preferences.env_overrides)?);
    let user_overrides = env.remove("WINEDLLOVERRIDES").unwrap_or_default();
    env.insert(
        "WINEDLLOVERRIDES".to_owned(),
        merge_dll_overrides(&user_overrides),
    );

    if preferences.proton_debug_logging {
        env.insert("UMU_LOG".to_owned(), "1".to_owned());
        env.insert("PROTON_LOG".to_owned(), "1".to_owned());
    }

    let mut args = argv;
    let program = args.remove(0);
    let cwd = game_executable
        .parent()
        .ok_or_else(|| AppError::message("The game executable has no parent folder."))?
        .to_string_lossy()
        .into_owned();

    Ok(LaunchPlan {
        program,
        args,
        cwd,
        env,
        style,
        wrappers,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        LaunchOptionStyle, LaunchToken, STEAM_DLL_OVERRIDE, build_argv, merge_dll_overrides,
        parse_env_overrides, plan, tokenize,
    };
    use crate::models::Preferences;
    use std::path::Path;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    fn texts(tokens: Vec<LaunchToken>) -> Vec<String> {
        tokens.into_iter().map(|token| token.text).collect()
    }

    fn preferences(options: &str, env: &str) -> Preferences {
        Preferences {
            launch_options: options.into(),
            env_overrides: env.into(),
            ..Preferences::default()
        }
    }

    #[test]
    fn tokenizer_handles_quotes_and_escapes() {
        assert_eq!(
            texts(tokenize("-novid 'Project Sunrise' \"a\\\"b\"").expect("tokens")),
            strings(&["-novid", "Project Sunrise", "a\"b"])
        );
        assert_eq!(tokenize("").expect("empty"), Vec::<LaunchToken>::new());
        assert_eq!(texts(tokenize("a'b'c").expect("tokens")), strings(&["abc"]));

        let quoted = tokenize("plain \"a b\"").expect("tokens");
        assert!(!quoted[0].quoted);
        assert!(quoted[1].quoted);

        assert!(tokenize("'oops").is_err());
        assert!(tokenize("\"oops\\").is_err());
    }

    #[test]
    fn build_argv_handles_wrapper_and_quoted_placeholder() {
        let (argv, style, wrappers, _) =
            build_argv("umu-run", &strings(&["destiny2.exe"]), "-novid -windowed").expect("argv");
        assert_eq!(
            argv,
            strings(&["umu-run", "destiny2.exe", "-novid", "-windowed"])
        );
        assert_eq!(style, LaunchOptionStyle::Arguments);
        assert!(wrappers.is_empty());

        let (argv, style, wrappers, _) = build_argv(
            "umu-run",
            &strings(&["destiny2.exe"]),
            "gamemoderun mangohud %command% -novid",
        )
        .expect("argv");
        assert_eq!(
            argv,
            strings(&[
                "gamemoderun",
                "mangohud",
                "umu-run",
                "destiny2.exe",
                "-novid"
            ])
        );
        assert_eq!(style, LaunchOptionStyle::Wrapper);
        assert_eq!(wrappers, strings(&["gamemoderun", "mangohud", "umu-run"]));

        let (argv, _, _, _) = build_argv(
            "/tools/umu/umu-run",
            &strings(&["/games/it's here/destiny2.exe"]),
            "sh -c \"%command%\"",
        )
        .expect("argv");
        assert_eq!(
            argv,
            strings(&[
                "sh",
                "-c",
                "'/tools/umu/umu-run' '/games/it'\"'\"'s here/destiny2.exe'"
            ])
        );
    }

    #[test]
    fn build_argv_extracts_leading_env_assignment_only_in_wrapper_form() {
        let (argv, _, _, env) = build_argv(
            "umu-run",
            &strings(&["destiny2.exe"]),
            "NIXPKGS_ALLOW_UNFREE=1 nix-shell --run \"%command%\"",
        )
        .expect("argv");
        assert_eq!(
            argv,
            strings(&["nix-shell", "--run", "'umu-run' 'destiny2.exe'"])
        );
        assert_eq!(
            env,
            vec![("NIXPKGS_ALLOW_UNFREE".to_owned(), "1".to_owned())]
        );

        let (argv, style, _, env) =
            build_argv("umu-run", &strings(&["destiny2.exe"]), "FOO=bar").expect("argv");
        assert_eq!(argv, strings(&["umu-run", "destiny2.exe", "FOO=bar"]));
        assert_eq!(style, LaunchOptionStyle::Arguments);
        assert!(env.is_empty());

        let (argv, _, _, env) = build_argv(
            "umu-run",
            &strings(&["destiny2.exe"]),
            "\"FOO=bar\" sh -c \"%command%\"",
        )
        .expect("argv");
        assert_eq!(argv[0], "FOO=bar");
        assert!(env.is_empty());
    }

    #[test]
    fn merge_dll_overrides_keeps_steam_api64_but_respects_explicit_override() {
        assert_eq!(merge_dll_overrides(""), STEAM_DLL_OVERRIDE);
        assert_eq!(
            merge_dll_overrides("d3d11=n;dxgi=n"),
            "steam_api64=n,b;d3d11=n;dxgi=n"
        );
        assert_eq!(merge_dll_overrides("steam_api64=b"), "steam_api64=b");
        assert_eq!(merge_dll_overrides("STEAM_API64=n"), "STEAM_API64=n");
        assert_eq!(
            merge_dll_overrides("nonsense;d3d11=n"),
            "steam_api64=n,b;d3d11=n"
        );
    }

    #[test]
    fn plan_builds_the_full_environment_and_wrapper_argv() {
        let base = plan(
            Path::new("/tools/umu/umu-run"),
            Path::new("/tools/GE-Proton11-7"),
            Path::new("/data/prefix"),
            Path::new("/games/Sunrise/destiny2.exe"),
            &preferences("", ""),
        )
        .expect("plan");
        assert_eq!(base.program, "/tools/umu/umu-run");
        assert_eq!(base.args, strings(&["/games/Sunrise/destiny2.exe"]));
        assert_eq!(base.cwd, "/games/Sunrise");
        assert_eq!(
            base.env.get("WINEPREFIX").map(String::as_str),
            Some("/data/prefix")
        );
        assert_eq!(
            base.env.get("PROTONPATH").map(String::as_str),
            Some("/tools/GE-Proton11-7")
        );
        assert_eq!(base.env.get("GAMEID").map(String::as_str), Some("sunrise"));
        assert_eq!(base.env.get("STORE").map(String::as_str), Some("none"));
        assert_eq!(
            base.env.get("PROTON_VERB").map(String::as_str),
            Some("waitforexitandrun")
        );
        assert_eq!(
            base.env.get("WINEDLLOVERRIDES").map(String::as_str),
            Some(STEAM_DLL_OVERRIDE)
        );
        assert!(!base.env.contains_key("UMU_LOG"));

        // User environment overrides survive, merged with the DLL override
        // rather than replacing it, and a blank game ID falls back to "sunrise".
        let merged = plan(
            Path::new("/umu-run"),
            Path::new("/proton"),
            Path::new("/prefix"),
            Path::new("/games/destiny2.exe"),
            &preferences("", "WINEDLLOVERRIDES=d3d11=n"),
        )
        .expect("plan");
        assert_eq!(
            merged.env.get("WINEDLLOVERRIDES").map(String::as_str),
            Some("steam_api64=n,b;d3d11=n")
        );

        // Debug logging adds the log variables.
        let debug_preferences = Preferences {
            proton_debug_logging: true,
            ..Preferences::default()
        };
        let debug = plan(
            Path::new("/umu-run"),
            Path::new("/proton"),
            Path::new("/prefix"),
            Path::new("/games/destiny2.exe"),
            &debug_preferences,
        )
        .expect("plan");
        assert_eq!(debug.env.get("UMU_LOG").map(String::as_str), Some("1"));
        assert_eq!(debug.env.get("PROTON_LOG").map(String::as_str), Some("1"));

        // A wrapper launch option runs before umu, with game arguments after the exe.
        let wrapped = plan(
            Path::new("/umu-run"),
            Path::new("/proton"),
            Path::new("/prefix"),
            Path::new("/games/Sunrise/destiny2.exe"),
            &preferences("gamemoderun mangohud %command% -novid", ""),
        )
        .expect("plan");
        assert_eq!(wrapped.program, "gamemoderun");
        assert_eq!(
            wrapped.args,
            strings(&[
                "mangohud",
                "/umu-run",
                "/games/Sunrise/destiny2.exe",
                "-novid"
            ])
        );
        assert_eq!(wrapped.style, LaunchOptionStyle::Wrapper);
        // The managed Proton variables survive alongside the launch prefix.
        assert_eq!(
            wrapped.env.get("PROTONPATH").map(String::as_str),
            Some("/proton")
        );

        // parse_env_overrides rejects malformed lines instead of silently ignoring them.
        assert!(parse_env_overrides("NOT_A_PAIR").is_err());
    }
}
