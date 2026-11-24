//! Verify and stage signed native releases while the API continues to serve.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{io::AsyncWriteExt, sync::Mutex, time};
use tokio_util::{sync::CancellationToken, task::AbortOnDropHandle};
mod release;
use super::worker::{Proxy, Worker};
use crate::infrastructure::{
    installation::{INSTALL_EXECUTABLE, VERSION, installation_executable, version_key},
    profile::{Profile, private_file, write_private},
};
use release::{BINARY_LIMIT, bounded_response, transport, verified_release};
const CHECK_INTERVAL: Duration = Duration::from_secs(15 * 60);
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    feed: String,
    public_key: String,
}
#[derive(Clone, Deserialize, Serialize)]
pub struct Installed {
    pub version: String,
    sha256: String,
}
#[derive(Clone, Default, Deserialize, Serialize)]
struct State {
    highest: Option<String>,
    active: Option<Installed>,
    previous: Option<Installed>,
}
#[derive(Clone, Deserialize, Serialize)]
pub struct Installation {
    executable: PathBuf,
    version: String,
}
#[derive(Clone)]
pub struct Updates {
    profile: Profile,
    directory: PathBuf,
    state: Arc<Mutex<State>>,
    pub installation: Installation,
    _supervisor_pin: Option<Arc<fs::File>>,
}
impl Updates {
    pub fn open(profile: &Profile, installation: Option<Installation>) -> Result<Self> {
        let directory = profile.dir.join("updates");
        let state = if cfg!(feature = "manual-installers") {
            State::default()
        } else {
            read_state(&directory)?
        };
        let executable = std::env::current_exe()?;
        let supervisor_pin = if executable.parent() == Some(directory.as_path()) {
            Some(Arc::new(pin(&executable)?))
        } else {
            None
        };
        Ok(Self {
            profile: profile.clone(),
            directory,
            _supervisor_pin: supervisor_pin,
            state: Arc::new(Mutex::new(state)),
            installation: installation.unwrap_or(Installation {
                executable: std::env::current_exe()?,
                version: VERSION.into(),
            }),
        })
    }
    pub async fn start(&self) -> Result<Arc<Worker>> {
        let mut state = self.state.lock().await;
        if let Some(active) = state.active.as_ref()
            && let Ok(worker) = self.launch(active).await
        {
            return Ok(worker);
        }
        if state.active.is_some() {
            return self.restore(&mut state).await;
        }
        Worker::start(
            &self.profile,
            &self.installation.executable,
            &self.installation.version,
            None,
        )
        .await
    }
    pub async fn reject(&self, proxy: &Proxy, version: &str) -> Result<()> {
        let mut state = self.state.lock().await;
        if state
            .active
            .as_ref()
            .is_some_and(|active| active.version == version)
        {
            let worker = self.restore(&mut state).await?;
            proxy.switch(worker);
        }
        Ok(())
    }
    pub async fn rollback_after_supervisor_failure(&self) -> Result<()> {
        let mut state = self.state.lock().await;
        if state.active.is_some() {
            let restored = State {
                active: state.previous.clone(),
                previous: None,
                highest: state.highest.clone(),
            };
            self.save(&mut state, restored)?;
        }
        Ok(())
    }
    async fn launch(&self, release: &Installed) -> Result<Arc<Worker>> {
        let binary = self.path(release)?;
        let expected = release.sha256.clone();
        let verified = binary.clone();
        let pin = tokio::task::spawn_blocking(move || {
            let guard = pin(&verified)?;
            verify_file(&verified, &expected)?;
            Ok::<_, anyhow::Error>(guard)
        })
        .await??;
        Worker::start(&self.profile, &binary, &release.version, Some(pin)).await
    }
    async fn restore(&self, state: &mut State) -> Result<Arc<Worker>> {
        let previous = state.previous.clone();
        let (active, worker) = if let Some(previous) = previous
            && let Ok(worker) = self.launch(&previous).await
        {
            (Some(previous), worker)
        } else {
            (
                None,
                Worker::start(
                    &self.profile,
                    &self.installation.executable,
                    &self.installation.version,
                    None,
                )
                .await?,
            )
        };
        self.save(
            state,
            State {
                active,
                previous: None,
                highest: state.highest.clone(),
            },
        )?;
        Ok(worker)
    }
    pub async fn run(self, proxy: Proxy, shutdown: CancellationToken) {
        let mut check = time::interval(CHECK_INTERVAL);
        let mut heartbeat = time::interval(Duration::from_millis(500));
        let mut download = None;
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                _ = check.tick() => {
                    if download.is_none() {
                        let updater = self.clone();
                        download = Some(AbortOnDropHandle::new(tokio::spawn(async move { updater.fetch().await })));
                    }
                }
                _ = heartbeat.tick() => {
                    proxy.prune();
                    if download.as_ref().is_some_and(AbortOnDropHandle::is_finished)
                        && let Some(task) = download.take()
                    {
                        match task.await {
                            Ok(Ok(Some(release))) => {
                                if let Err(error) = self.activate(&proxy, release).await {
                                    eprintln!("Provider Router update was rejected: {error:#}");
                                }
                            }
                            Ok(Ok(None)) => {}
                            Ok(Err(error)) => eprintln!("Provider Router update check failed: {error:#}"),
                            Err(error) => eprintln!("Provider Router update task failed: {error}"),
                        }
                    }
                    if !proxy.active().running() {
                        let mut state = self.state.lock().await;
                        let result = if state.active.is_some() {
                            self.restore(&mut state).await
                        } else {
                            Worker::start(&self.profile, &self.installation.executable, &self.installation.version, None).await
                        };
                        match result {
                            Ok(worker) => proxy.switch(worker),
                            Err(error) => {
                                eprintln!("Provider Router could not restart its API worker: {error:#}");
                                time::sleep(Duration::from_secs(2)).await;
                            }
                        }
                    }
                }
            }
        }
        if let Some(task) = download {
            task.abort();
        }
    }
    async fn activate(&self, proxy: &Proxy, release: Installed) -> Result<()> {
        let mut state = self.state.lock().await;
        let worker = match self.launch(&release).await {
            Ok(worker) => worker,
            Err(error) => {
                let failed = State {
                    highest: Some(release.version),
                    ..state.clone()
                };
                self.save(&mut state, failed)?;
                return Err(error);
            }
        };
        let activated = State {
            highest: Some(release.version.clone()),
            active: Some(release),
            previous: state.active.clone(),
        };
        self.save(&mut state, activated)?;
        proxy.switch(worker);
        self.remove_unused(&state, proxy);
        Ok(())
    }
    async fn fetch(&self) -> Result<Option<Installed>> {
        let Some(config) = self.config()? else {
            return Ok(None);
        };
        let feed = reqwest::Url::parse(&config.feed)?;
        transport(&feed)?;
        let key = hex::decode(&config.public_key)?;
        ensure!(key.len() == 32, "Invalid Ed25519 update key.");
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(120))
            .build()?;
        let response = http.get(feed.clone()).send().await?.error_for_status()?;
        let release = verified_release(&bounded_response(response).await?, &key)?;
        let version = version_key(&release.version)?;
        let highest = self.state.lock().await.highest.clone();
        if version <= version_key(VERSION)?
            || highest
                .as_deref()
                .map(version_key)
                .transpose()?
                .is_some_and(|highest| version <= highest)
        {
            return Ok(None);
        }
        let installed = Installed {
            version: release.version,
            sha256: release.sha256,
        };
        let destination = self.path(&installed)?;
        fs::create_dir_all(&self.directory)?;
        if verify_file(&destination, &installed.sha256).is_ok() {
            return Ok(Some(installed));
        }
        let response = http
            .get(feed.join(&release.artifact)?)
            .send()
            .await?
            .error_for_status()?;
        let (file, temporary) = tempfile::NamedTempFile::new_in(&self.directory)?.into_parts();
        let mut file = tokio::fs::File::from_std(file);
        let mut response = response;
        let mut length = 0;
        let mut sha = Sha256::new();
        while let Some(chunk) = response.chunk().await? {
            length += chunk.len() as u64;
            ensure!(
                length <= release.size,
                "The update artifact exceeds its signed length."
            );
            sha.update(&chunk);
            file.write_all(&chunk).await?;
        }
        ensure!(
            length == release.size && format!("{:x}", sha.finalize()) == installed.sha256,
            "The update artifact does not match its signed digest or length."
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o700))
                .await?;
        }
        file.sync_all().await?;
        drop(file);
        temporary.persist(&destination)?;
        Ok(Some(installed))
    }
    fn config(&self) -> Result<Option<Config>> {
        if cfg!(feature = "manual-installers") {
            return Ok(None);
        }
        match fs::read(self.profile.dir.join("updates.toml")) {
            Ok(bytes) => Ok(Some(toml::from_str(std::str::from_utf8(&bytes)?)?)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match (
                    option_env!("PROVIDER_ROUTER_UPDATE_FEED"),
                    option_env!("PROVIDER_ROUTER_UPDATE_PUBLIC_KEY"),
                ) {
                    (Some(feed), Some(public_key)) => Ok(Some(Config {
                        feed: feed.into(),
                        public_key: public_key.into(),
                    })),
                    (None, None) => Ok(None),
                    _ => anyhow::bail!("The update feed and key must be configured together."),
                }
            }
            Err(error) => Err(error.into()),
        }
    }
    fn path(&self, release: &Installed) -> Result<PathBuf> {
        version_key(&release.version)?;
        ensure!(
            release.sha256.len() == 64
                && release
                    .sha256
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "Invalid release digest."
        );
        let name = format!(
            "router-{}-{}{}",
            release.version,
            release.sha256,
            std::env::consts::EXE_SUFFIX
        );
        Ok(self.directory.join(name))
    }
    fn save(&self, state: &mut State, next: State) -> Result<()> {
        fs::create_dir_all(&self.directory)?;
        write_private(
            &self.directory.join("state.json"),
            &serde_json::to_vec(&next)?,
        )?;
        *state = next;
        Ok(())
    }
    fn remove_unused(&self, state: &State, proxy: &Proxy) {
        // Windows will leave running images in place. A later successful update retries cleanup.
        let mut keep: Vec<_> = [&state.active, &state.previous]
            .into_iter()
            .flatten()
            .filter_map(|release| self.path(release).ok())
            .collect();
        keep.extend(proxy.live_paths());
        if let Ok(current) = std::env::current_exe() {
            keep.push(current);
        }
        if let Ok(entries) = fs::read_dir(&self.directory) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.starts_with("router-")
                    && !name.ends_with(".pin")
                    && !keep.contains(&entry.path())
                    && let Ok(lock) = pin_file(&entry.path())
                    && fs2::FileExt::try_lock_exclusive(&lock).is_ok()
                {
                    let _ = fs::remove_file(entry.path());
                }
            }
        }
    }
}
fn verify_file(path: &Path, expected: &str) -> Result<()> {
    let mut file = fs::File::open(path)?;
    ensure!(
        file.metadata()?.len() <= BINARY_LIMIT,
        "The installed artifact is too large."
    );
    let mut sha = Sha256::new();
    let mut buffer = [0; 32768];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        sha.update(&buffer[..count]);
    }
    ensure!(
        format!("{:x}", sha.finalize()) == expected,
        "The staged artifact has changed."
    );
    Ok(())
}
pub async fn forward_cli(profile: &Profile) -> Result<()> {
    if cfg!(feature = "manual-installers") {
        return Ok(());
    }
    let updater = Updates::open(profile, None)?;
    let state = updater.state.lock().await;
    let Some(active) = &state.active else {
        return Ok(());
    };
    if version_key(&active.version)? <= version_key(VERSION)? {
        return Ok(());
    }
    let executable = updater.path(active)?;
    let Ok(pin_guard) = pin(&executable) else {
        return Ok(());
    };
    if verify_file(&executable, &active.sha256).is_err() {
        return Ok(());
    }
    let status = std::process::Command::new(executable)
        .args(std::env::args_os().skip(1))
        .env(INSTALL_EXECUTABLE, installation_executable()?)
        .status()?;
    drop(pin_guard);
    std::process::exit(status.code().unwrap_or(1));
}
pub fn pin(executable: &Path) -> Result<fs::File> {
    let lock = pin_file(executable)?;
    fs2::FileExt::lock_shared(&lock)?;
    Ok(lock)
}
fn pin_file(executable: &Path) -> Result<fs::File> {
    let mut path = executable.as_os_str().to_owned();
    path.push(".pin");
    private_file(Path::new(&path))
}

fn read_state(directory: &Path) -> Result<State> {
    match fs::read(directory.join("state.json")) {
        Ok(bytes) => serde_json::from_slice(&bytes).context("Invalid update state."),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
        Err(error) => Err(error.into()),
    }
}

pub fn installed_release_version(profile: &Profile) -> Result<Option<String>> {
    Ok(read_state(&profile.dir.join("updates"))?
        .active
        .map(|active| active.version))
}
