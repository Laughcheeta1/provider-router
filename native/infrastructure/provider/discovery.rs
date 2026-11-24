//! Inspect local Codex installations without spending tokens or emitting account details.
use std::collections::HashMap;
use std::env;
#[cfg(windows)]
use std::ffi::OsStr;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use anyhow::Result;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::time::timeout;
use uuid::Uuid;

use super::failure;
use crate::{
    application::management::Discovered,
    domain::{Connection, Method},
};

static CANDIDATES: OnceLock<Mutex<HashMap<Option<String>, Uuid>>> = OnceLock::new();

pub async fn discover() -> Result<Vec<Discovered>> {
    let mut probes = tokio::task::JoinSet::new();
    probes.spawn(async { candidate(None).await });
    for distribution in wsl_distributions().await {
        probes.spawn(async move { candidate(Some(distribution)).await });
    }
    let mut candidates = Vec::new();
    while let Some(probe) = probes.join_next().await {
        candidates.push(probe?);
    }
    candidates.sort_by(|left, right| {
        let left = left.connection.wsl_distribution.as_deref();
        let right = right.connection.wsl_distribution.as_deref();
        left.cmp(&right)
    });
    Ok(candidates)
}

async fn candidate(distribution: Option<String>) -> Discovered {
    let id = *CANDIDATES
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .entry(distribution.clone())
        .or_insert_with(Uuid::now_v7);
    let connection = Connection {
        id,
        name: distribution
            .as_ref()
            .map_or_else(|| "Codex".into(), |name| format!("Codex · {name}")),
        method: Method::Codex,
        exposed: false,
        models: vec![],
        codex_home: None,
        executable: None,
        wsl_distribution: distribution,
    };
    let installed = run_codex_probe(&connection, &["--version"]).await.is_some();
    let (status, detail) = if !installed {
        (
            "not_installed",
            "Install Codex CLI to connect this provider.",
        )
    } else if run_codex_probe(&connection, &["login", "status"])
        .await
        .is_some()
    {
        ("ready", "Existing Codex login detected.")
    } else {
        (
            "login_required",
            "Run codex login in this environment, then refresh.",
        )
    };
    Discovered {
        connection,
        status,
        detail,
    }
}

pub(super) async fn codex_client_version(connection: &Connection) -> String {
    if let Some(output) = run_codex_probe(connection, &["--version"]).await {
        for word in String::from_utf8_lossy(&output).split_whitespace() {
            let version = word.trim_start_matches('v');
            let core = version.split_once('-').map_or(version, |(core, _)| core);
            if core.matches('.').count() == 2
                && core.split('.').all(|part| {
                    !part.is_empty()
                        && part.len() <= 8
                        && part.bytes().all(|byte| byte.is_ascii_digit())
                })
            {
                return core.into();
            }
        }
    }
    "0.167.0".into()
}

async fn run_codex_probe(connection: &Connection, arguments: &[&str]) -> Option<Vec<u8>> {
    let command = match connection.wsl_distribution.as_deref() {
        Some(distribution) => wsl_codex(distribution, connection, arguments)?,
        None => native_codex(connection, arguments)?,
    };
    bounded_output(command, Duration::from_secs(5)).await
}

async fn bounded_output(mut command: Command, deadline: Duration) -> Option<Vec<u8>> {
    timeout(deadline, async {
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .ok()?;
        let mut output = vec![];
        const LIMIT: u64 = 1024 * 1024;
        child
            .stdout
            .take()?
            .take(LIMIT + 1)
            .read_to_end(&mut output)
            .await
            .ok()?;
        if output.len() as u64 > LIMIT {
            return None;
        }
        child.wait().await.ok()?.success().then_some(output)
    })
    .await
    .ok()
    .flatten()
}

fn native_codex(connection: &Connection, arguments: &[&str]) -> Option<Command> {
    let executable = connection
        .executable
        .as_deref()
        .unwrap_or(Path::new("codex"));
    let (program, prefix, search_path) = executable_command(executable)?;
    let mut command = Command::new(program);
    command.args(prefix).args(arguments);
    if let Some(path) = search_path {
        command.env("PATH", path);
    }
    if let Some(home) = &connection.codex_home {
        command.env("CODEX_HOME", home);
    }
    #[cfg(windows)]
    hide_window(&mut command);
    Some(command)
}

type ExecutableCommand = (PathBuf, Vec<OsString>, Option<OsString>);

#[cfg(not(windows))]
fn executable_command(executable: &Path) -> Option<ExecutableCommand> {
    use std::os::unix::fs::PermissionsExt;
    let is_executable = |path: &Path| {
        path.metadata()
            .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
    };
    if executable.components().count() > 1 {
        return is_executable(executable).then(|| (executable.into(), vec![], None));
    }

    let base_path = env::var_os("PATH").unwrap_or_default();
    let mut directories: Vec<_> = env::split_paths(&base_path).collect();
    if let Some(home) = dirs::home_dir() {
        for path in [".local/bin", ".bun/bin", ".npm-global/bin", ".volta/bin"] {
            directories.push(home.join(path));
        }
        if let Ok(versions) = home.join(".nvm/versions/node").read_dir() {
            let mut versions: Vec<_> = versions.flatten().map(|entry| entry.path()).collect();
            versions.sort();
            directories.extend(
                versions
                    .into_iter()
                    .rev()
                    .map(|version| version.join("bin")),
            );
        }
    }
    directories.extend([
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
    ]);
    directories.into_iter().find_map(|directory| {
        let path = directory.join(executable);
        let preferred_path =
            env::join_paths(std::iter::once(directory).chain(env::split_paths(&base_path))).ok();
        is_executable(&path).then_some((path, vec![], preferred_path))
    })
}

#[cfg(windows)]
fn executable_command(executable: &Path) -> Option<ExecutableCommand> {
    let path = env::var_os("PATH").unwrap_or_default();
    let name = executable.file_name()?;
    let directories: Vec<PathBuf> = if executable.components().count() > 1 {
        vec![executable.parent()?.into()]
    } else {
        env::split_paths(&path).collect()
    };
    for directory in directories {
        let base = directory.join(name);
        let extension = base.extension().and_then(OsStr::to_str).unwrap_or_default();
        if extension.is_empty() || extension.eq_ignore_ascii_case("exe") {
            let binary = if extension.is_empty() {
                base.with_extension("exe")
            } else {
                base.clone()
            };
            if binary.is_file() {
                return Some((binary, vec![], None));
            }
        }
        // Upstream package shims use the installed Codex wrapper.
        if (extension.is_empty() && base.with_extension("cmd").is_file())
            || (extension.eq_ignore_ascii_case("cmd") && base.is_file())
        {
            let script = directory
                .join("node_modules")
                .join("@openai")
                .join("codex")
                .join("bin")
                .join("codex.js");
            if script.is_file() {
                return Some(("node.exe".into(), vec![script.into_os_string()], None));
            }
        }
    }
    None
}

#[cfg(windows)]
pub(super) fn hide_window(command: &mut Command) {
    command.creation_flags(0x08000000); // CREATE_NO_WINDOW
}

#[cfg(not(windows))]
async fn wsl_distributions() -> Vec<String> {
    vec![]
}

#[cfg(windows)]
async fn wsl_distributions() -> Vec<String> {
    let mut command = Command::new("wsl.exe");
    hide_window(&mut command);
    command.args(["--list", "--quiet"]);
    let Some(output) = bounded_output(command, Duration::from_secs(5)).await else {
        return vec![];
    };
    let text = if output.contains(&0) {
        String::from_utf16_lossy(
            &output
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect::<Vec<_>>(),
        )
    } else {
        String::from_utf8_lossy(&output).into_owned()
    };
    let mut names: Vec<String> = text
        .trim_start_matches('\u{feff}')
        .lines()
        .map(str::trim)
        .filter(|name| valid_distribution(name))
        .map(str::to_owned)
        .collect();
    names.sort();
    names.dedup();
    names
}

#[cfg(windows)]
pub(super) fn wsl_command(distribution: &str) -> Command {
    let mut command = Command::new("wsl.exe");
    hide_window(&mut command);
    command.args(["--distribution", distribution, "--exec"]);
    command
}

#[cfg(windows)]
fn valid_distribution(name: &str) -> bool {
    (1..=128).contains(&name.len())
        && name.as_bytes()[0].is_ascii_alphanumeric()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._ ()-".contains(&byte))
}

#[cfg(windows)]
fn wsl_codex(distribution: &str, connection: &Connection, arguments: &[&str]) -> Option<Command> {
    if !valid_distribution(distribution) {
        return None;
    }
    let mut command = wsl_command(distribution);
    let script = r#"
        set -eu
        cli="$1"
        if [ -z "$cli" ]; then cli=$(command -v codex || true); fi
        case "$cli" in /mnt/[a-zA-Z]/*) cli="";; esac
        if [ -z "$cli" ]; then
            for candidate in "$HOME/.local/bin/codex" "$HOME/.bun/bin/codex" "$HOME/.npm-global/bin/codex" "$HOME"/.nvm/versions/node/*/bin/codex; do
                if [ -x "$candidate" ]; then cli="$candidate"; fi
            done
        fi
        [ -n "$cli" ] && [ -x "$cli" ]
        export CODEX_HOME="${2:-${CODEX_HOME:-$HOME/.codex}}"
        export PATH="$(dirname -- "$cli"):$PATH"
        shift 2
        exec "$cli" "$@"
    "#;
    command
        .args(["/bin/sh", "-c", script, "provider-router"])
        .arg(
            connection
                .executable
                .as_deref()
                .map_or(OsStr::new(""), Path::as_os_str),
        )
        .arg(
            connection
                .codex_home
                .as_deref()
                .map_or(OsStr::new(""), Path::as_os_str),
        )
        .args(arguments);
    Some(command)
}

#[cfg(not(windows))]
fn wsl_codex(
    _distribution: &str,
    _connection: &Connection,
    _arguments: &[&str],
) -> Option<Command> {
    None
}

#[cfg(windows)]
pub(super) async fn wsl_realpath(
    distribution: &str,
    home: Option<&Path>,
) -> Result<(PathBuf, String)> {
    if !valid_distribution(distribution) {
        return Err(failure(
            400,
            "WSL_DISTRIBUTION_INVALID",
            "Choose an installed WSL distribution.",
        ));
    }
    let mut command = wsl_command(distribution);
    command
        .args([
            "/bin/sh",
            "-c",
            r#"set -eu; realpath -m -- "${1:-${CODEX_HOME:-$HOME/.codex}}""#,
            "provider-router",
        ])
        .arg(home.map_or(OsStr::new(""), Path::as_os_str));
    let output = bounded_output(command, Duration::from_secs(15))
        .await
        .and_then(|output| String::from_utf8(output).ok())
        .map(|text| text.trim_end_matches(['\n', '\r']).to_owned())
        .filter(|text| text.starts_with('/') && !text.contains(['\n', '\r', '\0', '\\']))
        .ok_or_else(|| {
            failure(
                503,
                "WSL_PATH_INVALID",
                "The WSL Codex credential directory could not be resolved.",
            )
        })?;
    let windows = PathBuf::from(format!(
        r"\\wsl.localhost\{distribution}{}",
        output.replace('/', r"\")
    ));
    Ok((windows, output))
}

#[cfg(not(windows))]
pub(super) async fn wsl_realpath(
    _distribution: &str,
    _home: Option<&Path>,
) -> Result<(PathBuf, String)> {
    Err(failure(
        400,
        "WSL_WINDOWS_ONLY",
        "WSL connections require Windows.",
    ))
}
