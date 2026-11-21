//! Apply an installer payload to a stable executable while retaining the user's profile.

use std::{
    fs::{self, File},
    io,
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use tempfile::NamedTempFile;
use tokio::{process::Command, time};

use crate::infrastructure::{
    profile::{Profile, private_file},
    startup, supervisor,
};

use super::installation::{VERSION, installation_executable, version_key};

#[cfg(windows)]
const EXECUTABLE: &str = "provider-router.exe";
#[cfg(not(windows))]
const EXECUTABLE: &str = "provider-router";

pub async fn install(profile: &Profile, destination: &Path) -> Result<()> {
    let _installation_lock = lock_installation(profile)?;
    let destination = install_path(destination)?;
    let source = std::env::current_exe()?.canonicalize()?;
    ensure!(
        source != destination,
        "Run setup from its downloaded installer payload."
    );

    let existing = match fs::symlink_metadata(&destination) {
        Ok(metadata) => {
            ensure!(
                metadata.is_file(),
                "The installation path is not a regular file."
            );
            check_version(&installed_version(&destination).await?)?;
            Some(destination.as_path())
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error).context("Could not inspect the installed executable."),
    };
    check_profile_version(profile).await?;

    let parent = destination
        .parent()
        .context("An installation directory is required.")?;
    let payload = stage_copy(&source, parent).context("Could not stage the new executable.")?;
    let backup = existing
        .map(|path| stage_copy(path, parent))
        .transpose()
        .context("Could not preserve the installed executable.")?;
    let (runtime_lock, was_running) = if existing.is_some() {
        supervisor::pause_for_installation(profile).await?
    } else {
        let lock = supervisor::daemon_lock(profile)?.context(
            "Provider Router is running from another location. Run provider-router stop before installing to a new location.",
        )?;
        (lock, false)
    };

    let replaced = async {
        // An existing live-updating installation can advance while the payload is being staged.
        check_profile_version(profile).await?;
        replace(payload, &destination).await
    }
    .await;
    if let Err(error) = replaced {
        drop(runtime_lock);
        return Err(recovery_error(
            error,
            restart(profile, &destination, was_running).await,
        ));
    }

    let registered = startup::register_if_enabled_at(profile, &destination);
    drop(runtime_lock);
    let activated = match registered {
        Ok(()) => restart(profile, &destination, was_running).await,
        Err(error) => Err(error.context("Could not register Provider Router for startup.")),
    };
    if let Err(error) = activated {
        let recovery = async {
            let (lock, _) = supervisor::pause_for_installation(profile).await?;
            if let Some(backup) = backup {
                replace(backup, &destination).await?;
            } else {
                fs::remove_file(&destination)
                    .context("Could not remove the unsuccessful first installation.")?;
            }
            drop(lock);
            restart(profile, &destination, was_running).await
        }
        .await;
        return Err(recovery_error(error, recovery));
    }
    Ok(())
}

pub async fn uninstall(profile: &Profile) -> Result<()> {
    let _installation_lock = lock_installation(profile)?;
    let executable = installation_executable()?;
    let (runtime_lock, was_running) = supervisor::pause_for_installation(profile).await?;
    let unregistered = startup::unregister(profile, &executable);
    drop(runtime_lock);
    if let Err(error) = unregistered {
        return Err(recovery_error(
            error,
            restart(profile, &executable, was_running).await,
        ));
    }
    Ok(())
}

fn lock_installation(profile: &Profile) -> Result<File> {
    let lock = private_file(&profile.dir.join("installation.lock"))?;
    fs2::FileExt::try_lock_exclusive(&lock)
        .context("Could not acquire the Provider Router installation lock.")?;
    Ok(lock)
}

fn install_path(path: &Path) -> Result<PathBuf> {
    ensure!(
        path.is_absolute() && path.file_name().is_some_and(|name| name == EXECUTABLE),
        "Use an absolute installation path ending in {EXECUTABLE}."
    );
    let parent = path
        .parent()
        .context("An installation directory is required.")?;
    fs::create_dir_all(parent).context("Could not create the installation directory.")?;
    Ok(parent.canonicalize()?.join(EXECUTABLE))
}

async fn installed_version(executable: &Path) -> Result<String> {
    let mut command = Command::new(executable);
    command
        .arg("--version")
        .stdin(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    let output = time::timeout(Duration::from_secs(5), command.output())
        .await
        .context("The installed executable did not respond to a version check.")?
        .context("Could not check the installed executable.")?;
    ensure!(
        output.status.success(),
        "The installed executable could not report its version."
    );
    Ok(std::str::from_utf8(&output.stdout)?
        .trim()
        .strip_prefix("provider-router ")
        .context("The installation path contains an unrecognized executable.")?
        .into())
}

async fn check_profile_version(profile: &Profile) -> Result<()> {
    let status = supervisor::status(profile).await?;
    for version in [
        status.supervisor_version,
        status.worker_version,
        supervisor::installed_release_version(profile)?,
    ]
    .into_iter()
    .flatten()
    {
        check_version(&version)?;
    }
    Ok(())
}

fn check_version(installed: &str) -> Result<()> {
    ensure!(
        version_key(VERSION)? >= version_key(installed)?,
        "Provider Router {installed} is newer than this installer ({VERSION})."
    );
    Ok(())
}

fn stage_copy(source: &Path, directory: &Path) -> Result<NamedTempFile> {
    let mut original = File::open(source)?;
    let mut staged = NamedTempFile::new_in(directory)?;
    io::copy(&mut original, &mut staged)?;
    #[cfg(unix)]
    staged
        .as_file()
        .set_permissions(original.metadata()?.permissions())?;
    staged.as_file().sync_all()?;
    Ok(staged)
}

async fn replace(mut staged: NamedTempFile, destination: &Path) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match staged.persist(destination) {
            Ok(_) => return Ok(()),
            Err(error)
                if cfg!(windows)
                    && matches!(error.error.raw_os_error(), Some(5 | 32 | 33))
                    && Instant::now() < deadline =>
            {
                staged = error.file;
                time::sleep(Duration::from_millis(100)).await;
            }
            Err(error) => {
                let locked =
                    cfg!(windows) && matches!(error.error.raw_os_error(), Some(5 | 32 | 33));
                let message = if locked {
                    "Could not replace the executable. Close any open Provider Router dashboard and retry."
                } else {
                    "Could not replace the Provider Router executable."
                };
                return Err(error.error).context(message);
            }
        }
    }
}

async fn restart(profile: &Profile, executable: &Path, was_running: bool) -> Result<()> {
    if was_running {
        supervisor::start_background_at(profile, executable).await?;
        let status = supervisor::status(profile).await?;
        ensure!(
            status.endpoint.is_some(),
            "The router restarted without a serving endpoint. {}",
            status
                .error
                .as_deref()
                .unwrap_or("The local API did not respond.")
        );
    }
    Ok(())
}

fn recovery_error(error: anyhow::Error, recovery: Result<()>) -> anyhow::Error {
    match recovery {
        Ok(()) => error,
        Err(recovery) => error.context(format!(
            "The previous installation could not be restored: {recovery:#}"
        )),
    }
}
