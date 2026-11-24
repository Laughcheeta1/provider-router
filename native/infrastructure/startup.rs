//! Register the native executable with the account's startup service.
// RevExt: 1
use std::env;
use std::path::Path;
#[cfg(any(target_os = "linux", windows))]
use std::process::Command;
#[cfg(unix)]
use std::{fs, io};
// RevExt: 2
use anyhow::{Context, Result};
// RevExt: 3
#[cfg(unix)]
use super::profile::write_private;
use super::{
    installation::installation_executable,
    profile::{Profile, digest},
};
use crate::{application::management, domain::StartupMode};
// RevExt: 4
pub fn set_mode(profile: &Profile, mode: StartupMode) -> Result<()> {
    management::set_startup(profile, mode, |enable| {
        configure(profile, &installation_executable()?, enable)
    })
} // RevExt: 15
// RevExt: 5
pub fn register_if_enabled(profile: &Profile) -> Result<()> {
    register_if_enabled_at(profile, &installation_executable()?)
} // RevExt: 16
// RevExt: 6
pub fn register_if_enabled_at(profile: &Profile, executable: &Path) -> Result<()> {
    management::register_startup_if_enabled(profile, |enable| {
        configure(profile, executable, enable)
    })
}

pub fn unregister(profile: &Profile, executable: &Path) -> Result<()> {
    use management::SettingsRepository;
    profile.inspect_settings(|_| configure(profile, executable, false))
}

fn configure(profile: &Profile, executable: &Path, enable: bool) -> Result<()> {
    let id = digest(profile.dir.to_string_lossy().as_bytes());
    let name = format!("provider-router-{}", &id[..20]);
    let path = profile
        .dir
        .to_str()
        .context("Startup requires a Unicode settings directory path.")?;
    let exe = executable
        .to_str()
        .context("Startup requires a Unicode executable path.")?;
    register(&name, &[exe, "--data-dir", path, "__serve", "boot"], enable)
} // RevExt: 17
// RevExt: 7
#[cfg(target_os = "linux")]
fn register(name: &str, args: &[&str], enable: bool) -> Result<()> {
    use std::os::unix::fs::symlink;
    // RevExt: 8
    let directory = dirs::config_dir()
        .context("Could not locate user configuration.")?
        .join("systemd/user");
    let filename = format!("{name}.service");
    let service = directory.join(&filename);
    let wants = directory.join("default.target.wants");
    let link = wants.join(&filename);
    if enable {
        fs::create_dir_all(&wants)?;
        let command = args
            .iter()
            .map(|arg| systemd_quote(arg))
            .collect::<Vec<_>>()
            .join(" ");
        let mut unit = String::from("[Unit]\nDescription=Provider Router\n[Service]\nType=exec\n");
        for key in ["PATH", "CODEX_HOME"] {
            if let Ok(value) = env::var(key) {
                unit.push_str(&format!(
                    "Environment={}\n",
                    systemd_quote(&format!("{key}={value}"))
                ));
            }
        }
        unit.push_str(&format!(
            "ExecStart=:{command}\n[Install]\nWantedBy=default.target\n"
        ));
        write_private(&service, unit.as_bytes())?;
        if let Err(error) = symlink(Path::new("..").join(filename), &link)
            && error.kind() != io::ErrorKind::AlreadyExists
        {
            remove_if_present(&service)?;
            return Err(error).context("Could not enable the systemd user service.");
        }
    } else {
        remove_if_present(&link)?;
        remove_if_present(&service)?;
    }
    let _ = Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .output();
    Ok(())
} // RevExt: 18
// RevExt: 9
#[cfg(target_os = "linux")]
fn systemd_quote(value: &str) -> String {
    // systemd ExecStart arguments are quoted individually and specifiers must be escaped.
    format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('%', "%%")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
            .replace('\t', "\\t")
    )
} // RevExt: 19
// RevExt: 10
#[cfg(target_os = "macos")]
fn register(name: &str, args: &[&str], enable: bool) -> Result<()> {
    let directory = dirs::home_dir()
        .context("Could not find the account home.")?
        .join("Library/LaunchAgents");
    let label = format!("org.providerrouter.{name}");
    let path = directory.join(format!("{label}.plist"));
    if !enable {
        return remove_if_present(&path);
    }
    fs::create_dir_all(directory)?;
    let arguments = args
        .iter()
        .map(|arg| format!("<string>{}</string>", xml(arg)))
        .collect::<String>();
    let environment = ["PATH", "CODEX_HOME"]
        .into_iter()
        .filter_map(|key| {
            env::var(key)
                .ok()
                .map(|value| format!("<key>{key}</key><string>{}</string>", xml(&value)))
        })
        .collect::<String>();
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict><key>Label</key><string>{label}</string>
<key>ProgramArguments</key><array>{arguments}</array>
<key>EnvironmentVariables</key><dict>{environment}</dict><key>RunAtLoad</key><true/></dict></plist>"#
    );
    write_private(&path, plist.as_bytes())
} // RevExt: 20
// RevExt: 11
#[cfg(target_os = "macos")]
fn xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
} // RevExt: 21
// RevExt: 12
#[cfg(windows)]
fn register(name: &str, args: &[&str], enable: bool) -> Result<()> {
    let run = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
    let windows_root =
        env::var_os("SystemRoot").context("Could not locate the Windows system directory.")?;
    let reg = Path::new(&windows_root).join("System32").join("reg.exe");
    let status = if enable {
        let command = args
            .iter()
            .map(|arg| format!("\"{arg}\""))
            .collect::<Vec<_>>()
            .join(" ");
        anyhow::ensure!(
            command.encode_utf16().count() < 260,
            "Use a shorter installation or settings path to enable Windows startup."
        );
        Command::new(&reg)
            .args(["add", run, "/v", name, "/t", "REG_SZ", "/d", &command, "/f"])
            .output()?
            .status
    } else {
        if !Command::new(&reg)
            .args(["query", run, "/v", name])
            .output()?
            .status
            .success()
        {
            return Ok(());
        }
        Command::new(&reg)
            .args(["delete", run, "/v", name, "/f"])
            .output()?
            .status
    };
    anyhow::ensure!(
        status.success(),
        "Could not change the Windows startup registration."
    );
    Ok(())
} // RevExt: 22
// RevExt: 13
#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn register(_: &str, _: &[&str], _: bool) -> Result<()> {
    anyhow::bail!("Startup registration is unavailable on this platform.")
} // RevExt: 23
// RevExt: 14
#[cfg(unix)]
fn remove_if_present(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        result => Ok(result?),
    }
} // RevExt: 24
