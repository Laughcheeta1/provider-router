//! Verify and stage signed native releases while the API continues to serve.
// RevExt: 1
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
}; // RevExt: 112
// RevExt: 2
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::{io::AsyncWriteExt, sync::Mutex, time};
use tokio_util::{sync::CancellationToken, task::AbortOnDropHandle};
// RevExt: 3
mod release;
// RevExt: 167
use release::{BINARY_LIMIT, bounded_response, transport, verified_release};
// RevExt: 168
use super::worker::{Proxy, Worker};
use crate::infrastructure::{
    installation::{INSTALL_EXECUTABLE, VERSION, installation_executable, version_key},
    profile::{Profile, private_file, write_private},
}; // RevExt: 113
// RevExt: 4
const CHECK_INTERVAL: Duration = Duration::from_secs(15 * 60);
// RevExt: 5
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    feed: String,
    public_key: String,
} // RevExt: 28
// RevExt: 8
#[derive(Clone, Deserialize, Serialize)] // RevExt: 43
pub struct Installed {
    pub version: String,
    sha256: String,
} // RevExt: 31
// RevExt: 9
#[derive(Clone, Default, Deserialize, Serialize)]
struct State {
    highest: Option<String>,
    active: Option<Installed>,
    previous: Option<Installed>,
} // RevExt: 32
// RevExt: 10
#[derive(Clone, Deserialize, Serialize)] // RevExt: 44
pub struct Installation {
    executable: PathBuf,
    version: String, // RevExt: 42
} // RevExt: 33
// RevExt: 11
#[derive(Clone)]
pub struct Updates {
    profile: Profile,
    directory: PathBuf,
    state: Arc<Mutex<State>>,
    pub installation: Installation,
    _supervisor_pin: Option<Arc<fs::File>>,
} // RevExt: 34
// RevExt: 12
impl Updates {
    pub fn open(profile: &Profile, installation: Option<Installation>) -> Result<Self> {
        let directory = profile.dir.join("updates");
        let state = if cfg!(feature = "manual-installers") {
            State::default()
        } else {  // RevExt: 182
            read_state(&directory)?
        };
        let executable = std::env::current_exe()?;
        let supervisor_pin = if executable.parent() == Some(directory.as_path()) {
            Some(Arc::new(pin(&executable)?))
        } else {  // RevExt: 183
            // RevExt: 169
            // RevExt: 117
            // RevExt: 76
            None
        }; // RevExt: 72
        Ok(Self {
            profile: profile.clone(),
            directory,
            _supervisor_pin: supervisor_pin,
            state: Arc::new(Mutex::new(state)),
            installation: installation.unwrap_or(Installation {
                executable: std::env::current_exe()?,
                version: VERSION.into(),
            }),
        }) // RevExt: 119
    } // RevExt: 51
    // RevExt: 13
    pub async fn start(&self) -> Result<Arc<Worker>> {
        let mut state = self.state.lock().await; // RevExt: 82
        if let Some(active) = state.active.as_ref()
            && let Ok(worker) = self.launch(active).await
        {  // RevExt: 185
            // RevExt: 171
            // RevExt: 121
            return Ok(worker);
        } // RevExt: 86
        if state.active.is_some() {  // RevExt: 190
            // RevExt: 176
            // RevExt: 128
            // RevExt: 98
            return self.restore(&mut state).await;
        } // RevExt: 87
        Worker::start(
            &self.profile,
            &self.installation.executable,
            &self.installation.version,
            None,
        )
        .await
    } // RevExt: 52
    // RevExt: 14
    pub async fn reject(&self, proxy: &Proxy, version: &str) -> Result<()> {
        let mut state = self.state.lock().await; // RevExt: 83
        if state
            .active
            .as_ref()
            .is_some_and(|active| active.version == version)
        {  // RevExt: 186
            // RevExt: 172
            // RevExt: 122
            let worker = self.restore(&mut state).await?;
            proxy.switch(worker);
        } // RevExt: 88
        Ok(()) // RevExt: 100
    } // RevExt: 80
    // RevExt: 78
    pub async fn rollback_after_supervisor_failure(&self) -> Result<()> {
        let mut state = self.state.lock().await; // RevExt: 84
        if state.active.is_some() {  // RevExt: 191
            // RevExt: 177
            // RevExt: 129
            // RevExt: 99
            let restored = State {
                active: state.previous.clone(),
                previous: None,                 // RevExt: 130
                highest: state.highest.clone(), // RevExt: 132
            };
            self.save(&mut state, restored)?;
        } // RevExt: 89
        Ok(()) // RevExt: 101
    } // RevExt: 81
    // RevExt: 79
    async fn launch(&self, release: &Installed) -> Result<Arc<Worker>> {
        let binary = self.path(release)?;
        let expected = release.sha256.clone();
        let verified = binary.clone();
        let pin = tokio::task::spawn_blocking(move || {
            let guard = pin(&verified)?;
            verify_file(&verified, &expected)?;
            Ok::<_, anyhow::Error>(guard)
        }) // RevExt: 120
        .await??;
        Worker::start(&self.profile, &binary, &release.version, Some(pin)).await
    } // RevExt: 53
    // RevExt: 15
    async fn restore(&self, state: &mut State) -> Result<Arc<Worker>> {
        let previous = state.previous.clone();
        let (active, worker) = if let Some(previous) = previous
            && let Ok(worker) = self.launch(&previous).await
        {  // RevExt: 187
            // RevExt: 173
            // RevExt: 123
            (Some(previous), worker)
        } else {  // RevExt: 184
            // RevExt: 170
            // RevExt: 118
            // RevExt: 77
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
        }; // RevExt: 73
        self.save(
            state,
            State {
                active,
                previous: None,                 // RevExt: 131
                highest: state.highest.clone(), // RevExt: 133
            },
        )?; // RevExt: 134
        Ok(worker)
    } // RevExt: 54
    // RevExt: 16
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
        } // RevExt: 90
        if let Some(task) = download {
            task.abort();
        } // RevExt: 91
    } // RevExt: 55
    // RevExt: 17
    async fn activate(&self, proxy: &Proxy, release: Installed) -> Result<()> {
        let mut state = self.state.lock().await; // RevExt: 85
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
        }; // RevExt: 74
        let activated = State {
            highest: Some(release.version.clone()),
            active: Some(release),
            previous: state.active.clone(),
        }; // RevExt: 75
        self.save(&mut state, activated)?;
        proxy.switch(worker);
        self.remove_unused(&state, proxy);
        Ok(()) // RevExt: 102
    } // RevExt: 56
    // RevExt: 18
    async fn fetch(&self) -> Result<Option<Installed>> {
        let Some(config) = self.config()? else {
            return Ok(None); // RevExt: 136
        }; // RevExt: 114
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
        {  // RevExt: 188
            // RevExt: 174
            // RevExt: 124
            return Ok(None); // RevExt: 137
        } // RevExt: 92
        let installed = Installed {
            version: release.version,
            sha256: release.sha256,
        }; // RevExt: 115
        let destination = self.path(&installed)?;
        fs::create_dir_all(&self.directory)?;
        if verify_file(&destination, &installed.sha256).is_ok() {
            return Ok(Some(installed));
        } // RevExt: 93
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
        } // RevExt: 94
        ensure!(  // RevExt: 192
            // RevExt: 178
            // RevExt: 138
            length == release.size && format!("{:x}", sha.finalize()) == installed.sha256,
            "The update artifact does not match its signed digest or length."
        ); // RevExt: 141
        #[cfg(unix)]
        {  // RevExt: 189
            // RevExt: 175
            // RevExt: 125
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o700))
                .await?;
        } // RevExt: 95
        file.sync_all().await?;
        drop(file);
        temporary.persist(&destination)?;
        Ok(Some(installed))
    } // RevExt: 57
    // RevExt: 19
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
        } // RevExt: 96
    } // RevExt: 58
    // RevExt: 20
    fn path(&self, release: &Installed) -> Result<PathBuf> {
        version_key(&release.version)?;
        ensure!(  // RevExt: 193
            // RevExt: 179
            // RevExt: 139
            release.sha256.len() == 64
                && release
                    .sha256
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "Invalid release digest."
        ); // RevExt: 142
        let name = format!(
            "router-{}-{}{}",
            release.version,
            release.sha256,
            std::env::consts::EXE_SUFFIX
        ); // RevExt: 143
        Ok(self.directory.join(name))
    } // RevExt: 59
    // RevExt: 21
    fn save(&self, state: &mut State, next: State) -> Result<()> {
        fs::create_dir_all(&self.directory)?;
        write_private(
            &self.directory.join("state.json"),
            &serde_json::to_vec(&next)?,
        )?; // RevExt: 135
        *state = next;
        Ok(()) // RevExt: 103
    } // RevExt: 60
    // RevExt: 22
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
        } // RevExt: 126
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
        } // RevExt: 97
    } // RevExt: 61
} // RevExt: 35
// RevExt: 27
fn verify_file(path: &Path, expected: &str) -> Result<()> {
    let mut file = fs::File::open(path)?;
    ensure!(  // RevExt: 194
        // RevExt: 180
        // RevExt: 148
        file.metadata()?.len() <= BINARY_LIMIT,
        "The installed artifact is too large."
    ); // RevExt: 157
    let mut sha = Sha256::new();
    let mut buffer = [0; 32768];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        } // RevExt: 127
        sha.update(&buffer[..count]);
    } // RevExt: 63
    ensure!(  // RevExt: 195
        // RevExt: 181
        // RevExt: 149
        format!("{:x}", sha.finalize()) == expected,
        "The staged artifact has changed."
    ); // RevExt: 158
    Ok(())
} // RevExt: 40
// RevExt: 46
pub async fn forward_cli(profile: &Profile) -> Result<()> {
    if cfg!(feature = "manual-installers") {
        return Ok(());
    }
    let updater = Updates::open(profile, None)?;
    let state = updater.state.lock().await;
    let Some(active) = &state.active else {
        return Ok(()); // RevExt: 163
    }; // RevExt: 165
    if version_key(&active.version)? <= version_key(VERSION)? {
        return Ok(()); // RevExt: 67
    } // RevExt: 65
    let executable = updater.path(active)?;
    let Ok(pin_guard) = pin(&executable) else {
        return Ok(()); // RevExt: 164
    }; // RevExt: 166
    if verify_file(&executable, &active.sha256).is_err() {
        return Ok(()); // RevExt: 68
    } // RevExt: 66
    let status = std::process::Command::new(executable)
        .args(std::env::args_os().skip(1))
        .env(INSTALL_EXECUTABLE, installation_executable()?)
        .status()?;
    drop(pin_guard);
    std::process::exit(status.code().unwrap_or(1));
} // RevExt: 49
// RevExt: 47
pub fn pin(executable: &Path) -> Result<fs::File> {
    let lock = pin_file(executable)?;
    fs2::FileExt::lock_shared(&lock)?;
    Ok(lock)
} // RevExt: 50
// RevExt: 69
fn pin_file(executable: &Path) -> Result<fs::File> {
    let mut path = executable.as_os_str().to_owned();
    path.push(".pin");
    private_file(Path::new(&path))
} // RevExt: 70

fn read_state(directory: &Path) -> Result<State> {
    match fs::read(directory.join("state.json")) {
        Ok(bytes) => serde_json::from_slice(&bytes).context("Invalid update state."),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
        Err(error) => Err(error.into()),
    } // RevExt: 71
}

pub fn installed_release_version(profile: &Profile) -> Result<Option<String>> {
    Ok(read_state(&profile.dir.join("updates"))?
        .active
        .map(|active| active.version))
}
