//! Exercise manual installation with the same executable commands used by the packages.
#![cfg(target_os = "linux")]

use std::{
    fs,
    io::Write,
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    time::Duration,
};

use anyhow::{Result, ensure};
use serde_json::Value;

const BIN: &str = env!("CARGO_BIN_EXE_provider-router");

struct Installation {
    temp: tempfile::TempDir,
    config: PathBuf,
    profile: PathBuf,
    executable: PathBuf,
}

impl Installation {
    fn new() -> Result<Self> {
        let temp = tempfile::tempdir()?;
        let config = temp.path().join("config");
        let profile = temp.path().join("profile");
        let executable = temp.path().join("installed apps/provider-router");
        Ok(Self {
            temp,
            config,
            profile,
            executable,
        })
    }

    fn command(&self, binary: impl AsRef<Path>) -> Command {
        let mut command = Command::new(binary.as_ref());
        command
            .env("XDG_CONFIG_HOME", &self.config)
            .env("XDG_RUNTIME_DIR", self.temp.path())
            .env(
                "DBUS_SESSION_BUS_ADDRESS",
                format!("unix:path={}/no-session", self.temp.path().display()),
            )
            .arg("--data-dir")
            .arg(&self.profile);
        command
    }

    fn setup(&self) -> Result<Output> {
        Ok(self
            .command(BIN)
            .arg("__install")
            .arg("--destination")
            .arg(&self.executable)
            .output()?)
    }

    fn settings(&self) -> Result<Vec<u8>> {
        Ok(fs::read(self.profile.join("router.json"))?)
    }

    fn status(&self) -> Result<Value> {
        let output = self.command(BIN).arg("status").output()?;
        success(&output)?;
        Ok(serde_json::from_slice(&output.stdout)?)
    }

    fn set_unused_port(&self) -> Result<()> {
        let reservation = TcpListener::bind(("127.0.0.1", 0))?;
        let port = reservation.local_addr()?.port();
        drop(reservation);
        success(
            &self
                .command(BIN)
                .args(["network", "set", "--port", &port.to_string()])
                .output()?,
        )
    }

    fn serve(&self) -> Result<Running> {
        Ok(Running(
            self.command(&self.executable)
                .arg("serve")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .spawn()?,
        ))
    }

    async fn wait_for_endpoint(&self) -> Result<Value> {
        tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                let status = self.status()?;
                if status["endpoint"].is_string() {
                    return Ok::<_, anyhow::Error>(status);
                }
                tokio::time::sleep(Duration::from_millis(60)).await;
            }
        })
        .await?
    }
}

impl Drop for Installation {
    fn drop(&mut self) {
        let _ = self.command(BIN).arg("stop").output();
    }
}

struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn success(output: &Output) -> Result<()> {
    ensure!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

#[tokio::test]
async fn fresh_install_and_live_upgrade_preserve_settings_and_startup() -> Result<()> {
    let install = Installation::new()?;
    install.set_unused_port()?;
    let before = install.settings()?;
    success(&install.setup()?)?;
    assert_eq!(install.settings()?, before);
    assert!(install.status()?["endpoint"].is_null());

    let services = install.config.join("systemd/user");
    let service = fs::read_dir(&services)?
        .filter_map(Result::ok)
        .find(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|value| value == "service")
        })
        .expect("startup service was registered")
        .path();
    assert!(fs::read_to_string(&service)?.contains(install.executable.to_str().unwrap()));

    let old = install.serve()?;
    let previous = install.wait_for_endpoint().await?;
    success(&install.setup()?)?;
    let current = install.wait_for_endpoint().await?;
    assert_eq!(current["endpoint"], previous["endpoint"]);
    assert_eq!(install.settings()?, before);
    drop(old);

    success(
        &install
            .command(&install.executable)
            .arg("__uninstall")
            .output()?,
    )?;
    assert!(install.status()?["endpoint"].is_null());
    assert!(!service.exists());
    assert_eq!(install.settings()?, before);
    Ok(())
}

#[tokio::test]
async fn unsuccessful_upgrade_restores_the_previous_executable_and_service() -> Result<()> {
    let install = Installation::new()?;
    install.set_unused_port()?;
    success(&install.setup()?)?;
    fs::OpenOptions::new()
        .append(true)
        .open(&install.executable)?
        .write_all(b"previous installation marker")?;
    let before = install.settings()?;
    let old = install.serve()?;
    let previous = install.wait_for_endpoint().await?;
    let services = install.config.join("systemd/user");
    fs::remove_dir_all(&services)?;
    fs::write(&services, b"force startup registration to fail")?;

    let failure = install.setup()?;
    assert!(!failure.status.success());
    assert!(
        String::from_utf8_lossy(&failure.stderr)
            .contains("Could not register Provider Router for startup")
    );
    assert!(fs::read(&install.executable)?.ends_with(b"previous installation marker"));
    let current = install.wait_for_endpoint().await?;
    assert_eq!(current["endpoint"], previous["endpoint"]);
    assert_eq!(install.settings()?, before);
    drop(old);
    Ok(())
}

#[test]
fn an_older_installer_leaves_a_newer_executable_in_place() -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let install = Installation::new()?;
    fs::create_dir_all(install.executable.parent().unwrap())?;
    let newer = b"#!/bin/sh\nprintf 'provider-router 999.0.0\\n'\n";
    fs::write(&install.executable, newer)?;
    fs::set_permissions(&install.executable, fs::Permissions::from_mode(0o700))?;

    let failure = install.setup()?;
    assert!(!failure.status.success());
    assert!(String::from_utf8_lossy(&failure.stderr).contains("999.0.0 is newer"));
    assert_eq!(fs::read(&install.executable)?, newer);
    Ok(())
}

#[tokio::test]
async fn a_router_running_from_another_location_is_left_running() -> Result<()> {
    let install = Installation::new()?;
    install.set_unused_port()?;
    let mut original = Running(
        install
            .command(BIN)
            .arg("serve")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()?,
    );
    let before = install.wait_for_endpoint().await?;

    let failure = install.setup()?;
    assert!(!failure.status.success());
    assert!(String::from_utf8_lossy(&failure.stderr).contains("running from another location"));
    assert!(original.0.try_wait()?.is_none());
    assert!(!install.executable.exists());
    assert_eq!(install.status()?["endpoint"], before["endpoint"]);
    Ok(())
}
