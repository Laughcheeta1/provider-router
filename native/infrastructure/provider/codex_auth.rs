//! Reuse a signed-in Codex session and commit any rotating grant under Codex's lock.
use super::{bounded_json, failure};
use crate::domain::Connection;
use anyhow::{Result, bail, ensure};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use reqwest::{Client, Url};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::env;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, Weak};
use std::time::Duration;
use tokio::sync::Mutex as AsyncMutex;
use tokio::time::{Instant, sleep};
const DEFAULT_CLIENT: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const DEFAULT_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const MAX_CREDENTIAL_BYTES: u64 = 2 * 1024 * 1024;
const REFRESH_MARKER: &str = "provider-router-refresh.json";
static REFRESH_LOCKS: LazyLock<Mutex<HashMap<PathBuf, Weak<AsyncMutex<()>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
/// Resolving a WSL credential directory starts a WSL process, so requests reuse a recent result.
const WSL_HOME_REUSE: Duration = Duration::from_secs(300);
type WslHomes = HashMap<(String, Option<PathBuf>), (Instant, PathBuf, String)>;
static WSL_HOMES: LazyLock<Mutex<WslHomes>> = LazyLock::new(|| Mutex::new(HashMap::new()));
#[derive(Clone)]
pub(super) struct CredentialHome {
    directory: PathBuf,
    #[cfg(windows)]
    wsl: Option<WslHome>,
}
#[cfg(windows)]
#[derive(Clone)]
struct WslHome {
    distribution: String,
    directory: String,
}
#[derive(Eq, PartialEq)]
struct CredentialFile {
    path: PathBuf,
    #[cfg(windows)]
    linux_path: Option<String>,
}
pub(super) struct Authorization {
    pub access_token: String,
    pub account_id: Option<String>,
}
#[derive(Clone, Copy, PartialEq)]
enum Backend {
    File,
    Keyring,
}
struct Credentials {
    document: Value,
    backend: Backend,
}
impl Credentials {
    fn new(document: Value, backend: Backend) -> Result<Self> {
        let mode = document.get("auth_mode");
        ensure!(
            mode.is_none_or(|mode| mode.as_str() == Some("chatgpt"))
                && ["access_token", "refresh_token"].into_iter().all(|name| {
                    document["tokens"][name]
                        .as_str()
                        .is_some_and(|token| !token.is_empty())
                }),
            login_unavailable()
        );
        Ok(Self { document, backend })
    }
    fn access(&self) -> &str {
        self.document["tokens"]["access_token"].as_str().unwrap()
    }
    fn expired(&self) -> bool {
        let claims = self
            .access()
            .split('.')
            .nth(1)
            .and_then(|payload| URL_SAFE_NO_PAD.decode(payload).ok())
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
        let now = Utc::now();
        if let Some(expiration) = claims.and_then(|claims| claims["exp"].as_i64()) {
            return expiration <= now.timestamp() + 60;
        }
        self.document["last_refresh"]
            .as_str()
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
            .is_none_or(|last| now.signed_duration_since(last).num_days() >= 7)
    }
    fn generation(&self) -> String {
        let tokens = &self.document["tokens"];
        let identity = json!([
            tokens["access_token"],
            tokens["refresh_token"],
            tokens["account_id"],
            tokens["login_owner_id"],
        ]);
        format!("{:x}", Sha256::digest(identity.to_string().as_bytes()))
    }
    fn same_account(&self, other: &Credentials) -> bool {
        ["account_id", "login_owner_id", "oauth_provenance"]
            .into_iter()
            .all(|key| self.document["tokens"][key] == other.document["tokens"][key])
    }
    fn authorization(&self) -> Authorization {
        Authorization {
            access_token: self.access().to_owned(),
            account_id: self.document["tokens"]["account_id"]
                .as_str()
                .map(str::to_owned),
        }
    }
}
impl CredentialHome {
    pub(super) async fn for_connection(connection: &Connection) -> Result<Self> {
        if let Some(distribution) = connection.wsl_distribution.as_deref() {
            let (directory, linux_directory) =
                wsl_home(distribution, connection.codex_home.as_deref()).await?;
            #[cfg(not(windows))]
            let _ = linux_directory;
            return Ok(Self {
                directory,
                #[cfg(windows)]
                wsl: Some(WslHome {
                    distribution: distribution.to_owned(),
                    directory: linux_directory,
                }),
            });
        }
        let directory = connection
            .codex_home
            .clone()
            .or_else(|| {
                env::var_os("CODEX_HOME")
                    .filter(|path| !path.is_empty())
                    .map(PathBuf::from)
            })
            .or_else(|| dirs::home_dir().map(|home| home.join(".codex")))
            .ok_or_else(login_unavailable)?;
        Ok(Self {
            directory: directory.canonicalize().unwrap_or(directory),
            #[cfg(windows)]
            wsl: None,
        })
    }
    pub(super) async fn authorize(
        &self,
        client: &Client,
        rejected: Option<&str>,
    ) -> Result<Authorization> {
        let credentials = self.load_async().await?;
        if !credentials.expired() && rejected != Some(credentials.access()) {
            return Ok(credentials.authorization());
        }
        // A caller disconnect can cancel inference, but a rotating grant needs a durable commit.
        let home = self.clone();
        let client = client.clone();
        let rejected = rejected.map(str::to_owned);
        tokio::spawn(async move {
            let lock = home.refresh_mutex();
            let _guard = lock.lock().await;
            let current = home.load_async().await?;
            ensure!(credentials.same_account(&current), account_changed());
            if !current.expired() && rejected.as_deref() != Some(current.access()) {
                return Ok(current.authorization());
            }
            home.refresh(&client, current).await
        })
        .await
        .map_err(|_| refresh_failed())?
    }
    fn refresh_mutex(&self) -> Arc<AsyncMutex<()>> {
        let mut locks = REFRESH_LOCKS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        locks.retain(|_, lock| lock.strong_count() != 0);
        let handle = locks.entry(self.directory.clone()).or_default();
        if let Some(lock) = handle.upgrade() {
            return lock;
        }
        let lock = Arc::new(AsyncMutex::new(()));
        *handle = Arc::downgrade(&lock);
        lock
    }
    async fn refresh(&self, client: &Client, expected: Credentials) -> Result<Authorization> {
        let (authority, client_id) = authority(&expected)?;
        let target = self.credential_file().await?;
        let marker = match expected.backend {
            Backend::File => target.path.with_extension(REFRESH_MARKER),
            Backend::Keyring => self
                .directory
                .join("auth.json")
                .with_extension(REFRESH_MARKER),
        };
        let generation = expected.generation();
        {
            let _lock = self.lock(&target).await?;
            ensure!(
                self.credential_file().await? == target,
                credentials_changed()
            );
            let current = self.load_async().await?;
            ensure!(expected.same_account(&current), account_changed());
            if current.access() != expected.access() {
                return Ok(current.authorization());
            }
            ensure!(
                current.document == expected.document && current.backend == expected.backend,
                credentials_changed()
            );
            if expected.backend == Backend::File {
                check_replaceable(&target.path).map_err(|_| refresh_failed())?;
            }
            let old_marker = read_json(&marker).map_err(|_| refresh_uncertain())?;
            ensure!(
                old_marker.is_none_or(|marker| marker["generation"] != generation),
                refresh_uncertain()
            );
            write_private_json(
                &marker,
                &json!({
                    "generation": generation, "started_at": Utc::now().to_rfc3339()
                }),
            )
            .map_err(|_| refresh_uncertain())?;
        }
        let response = client
            .post(authority)
            .timeout(Duration::from_secs(30))
            .json(&json!({
                "grant_type": "refresh_token",
                "client_id": client_id,
                "refresh_token": expected.document["tokens"]["refresh_token"],
            }))
            .send()
            .await
            .map_err(|_| refresh_failed())?;
        if !response.status().is_success() {
            let latest = self.load_async().await?;
            ensure!(expected.same_account(&latest), account_changed());
            if latest.access() != expected.access() {
                return Ok(latest.authorization());
            }
            bail!(refresh_failed());
        }
        #[derive(Deserialize)]
        struct Grant {
            access_token: String,
            refresh_token: Option<String>,
            id_token: Option<String>,
        }
        let grant: Grant = bounded_json(response).await.map_err(|_| refresh_failed())?;
        ensure!(!grant.access_token.is_empty(), refresh_failed());
        let mut updated = expected.document.clone();
        updated["tokens"]["access_token"] = json!(grant.access_token);
        if let Some(refresh) = grant.refresh_token {
            ensure!(!refresh.is_empty(), refresh_failed());
            updated["tokens"]["refresh_token"] = json!(refresh);
        }
        if let Some(id) = grant.id_token {
            updated["tokens"]["id_token"] = json!(id);
        }
        updated["last_refresh"] = json!(Utc::now().to_rfc3339());
        let replacement = Credentials::new(updated, expected.backend)?;
        let _lock = self.lock(&target).await?;
        ensure!(
            self.credential_file().await? == target,
            credentials_changed()
        );
        let current = self.load_async().await?;
        ensure!(expected.same_account(&current), account_changed());
        if current.access() != expected.access() {
            return Ok(current.authorization());
        }
        ensure!(
            current.document == expected.document && current.backend == expected.backend,
            credentials_changed()
        );
        self.save_async(&replacement, &target.path).await?;
        if read_json(&marker)
            .ok()
            .flatten()
            .is_some_and(|marker| marker["generation"] == generation)
        {
            let _ = fs::remove_file(marker);
        }
        Ok(replacement.authorization())
    }
    async fn load_async(&self) -> Result<Credentials> {
        let home = self.clone();
        tokio::task::spawn_blocking(move || home.load())
            .await
            .map_err(|_| login_unavailable())?
    }
    fn load(&self) -> Result<Credentials> {
        let settings = self.auth_config()?;
        let backend = settings.get("cli_auth_credentials_store");
        let pattern = backend
            .map(|value| value.as_str().unwrap_or_default())
            .unwrap_or("file");
        match pattern {
            "file" => {}
            "keyring" | "auto" => {
                let encrypted = settings
                    .get("features")
                    .and_then(|features| features.get("secret_auth_storage"))
                    .and_then(toml::Value::as_bool)
                    .unwrap_or(false);
                #[cfg(windows)]
                let encrypted = encrypted || self.wsl.is_some();
                ensure!(!encrypted, unsupported_store());
                match self.keyring()?.get_password() {
                    Ok(document) => {
                        ensure!(
                            document.len() as u64 <= MAX_CREDENTIAL_BYTES,
                            login_unavailable()
                        );
                        let value =
                            serde_json::from_str(&document).map_err(|_| login_unavailable())?;
                        return Credentials::new(value, Backend::Keyring);
                    }
                    Err(keyring::Error::NoEntry) if pattern == "auto" => {}
                    Err(_) => bail!(login_unavailable()),
                }
            }
            _ => bail!(unsupported_store()),
        }
        let document = read_json(&self.directory.join("auth.json"))
            .map_err(|_| login_unavailable())?
            .ok_or_else(login_unavailable)?;
        Credentials::new(document, Backend::File)
    }
    fn auth_config(&self) -> Result<toml::Value> {
        let config = self.directory.join("config.toml");
        let data = match read_small(&config) {
            Ok(Some(data)) => data,
            Ok(None) => return Ok(toml::Value::Table(Default::default())),
            Err(_) => {
                bail!(failure(
                    400,
                    "CODEX_CONFIG_UNAVAILABLE",
                    "Codex authentication configuration could not be read."
                ))
            }
        };
        std::str::from_utf8(&data)
            .ok()
            .and_then(|text| toml::from_str(text).ok())
            .ok_or_else(unsupported_store)
    }
    fn keyring(&self) -> Result<keyring::Entry> {
        let canonical = self
            .directory
            .canonicalize()
            .unwrap_or_else(|_| self.directory.clone());
        let canonical = canonical.to_string_lossy();
        let hash = format!("{:x}", Sha256::digest(canonical.as_bytes()));
        keyring::Entry::new("Codex Auth", &format!("cli|{}", &hash[..16]))
            .map_err(|_| login_unavailable())
    }
    async fn save_async(&self, credentials: &Credentials, target: &Path) -> Result<()> {
        let home = self.clone();
        let document = credentials.document.clone();
        let backend = credentials.backend;
        let target = target.to_owned();
        tokio::task::spawn_blocking(move || match backend {
            Backend::File => check_replaceable(&target)
                .and_then(|()| write_private_json(&target, &document))
                .map_err(|_| refresh_failed()),
            Backend::Keyring => home
                .keyring()?
                .set_password(&document.to_string())
                .map_err(|_| refresh_failed()),
        })
        .await
        .map_err(|_| refresh_failed())?
    }
    async fn credential_file(&self) -> Result<CredentialFile> {
        #[cfg(windows)]
        if let Some(wsl) = &self.wsl {
            let filename = format!("{}/auth.json", wsl.directory);
            let (path, linux_path) =
                super::discovery::wsl_realpath(&wsl.distribution, Some(Path::new(&filename)))
                    .await?;
            return Ok(CredentialFile {
                path,
                linux_path: Some(linux_path),
            });
        }
        let path = self.directory.join("auth.json");
        let path = match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                path.canonicalize().map_err(|_| credentials_changed())?
            }
            Ok(_) => path,
            Err(error) if error.kind() == io::ErrorKind::NotFound => path,
            Err(_) => return Err(login_unavailable()),
        };
        Ok(CredentialFile {
            path,
            #[cfg(windows)]
            linux_path: None,
        })
    }
    async fn lock(&self, target: &CredentialFile) -> Result<CredentialLock> {
        #[cfg(windows)]
        if let (Some(wsl), Some(linux_path)) = (&self.wsl, &target.linux_path) {
            return wsl.lock(linux_path).await;
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options
            .open(target.path.with_extension("json.lock"))
            .map_err(|_| login_unavailable())?;
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(CredentialLock::Native { _file: file }),
                Err(TryLockError::WouldBlock) if Instant::now() < until => {
                    sleep(Duration::from_millis(25)).await;
                }
                Err(_) => return Err(storage_locked()),
            }
        }
    }
}
enum CredentialLock {
    Native {
        _file: File,
    },
    #[cfg(windows)]
    Wsl {
        _stdin: tokio::process::ChildStdin,
        _child: tokio::process::Child,
    },
}
#[cfg(windows)]
impl WslHome {
    async fn lock(&self, linux_path: &str) -> Result<CredentialLock> {
        use std::process::Stdio;
        use tokio::io::AsyncReadExt;
        use tokio::time::timeout;
        let lock_file = Path::new(linux_path).with_extension("json.lock");
        let lock_file = lock_file.to_string_lossy();
        let mut command = super::discovery::wsl_command(&self.distribution);
        let mut child = command
            .args([
                "flock",
                "-x",
                "-w",
                "10",
                &lock_file,
                "/bin/sh",
                "-c",
                "printf ready; cat >/dev/null",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| storage_locked())?;
        let stdin = child.stdin.take().ok_or_else(storage_locked)?;
        let mut stdout = child.stdout.take().ok_or_else(storage_locked)?;
        let mut ready = [0; 5];
        timeout(Duration::from_secs(12), stdout.read_exact(&mut ready))
            .await
            .map_err(|_| storage_locked())?
            .map_err(|_| storage_locked())?;
        ensure!(&ready == b"ready", storage_locked());
        Ok(CredentialLock::Wsl {
            _stdin: stdin,
            _child: child,
        })
    }
}
async fn wsl_home(distribution: &str, home: Option<&Path>) -> Result<(PathBuf, String)> {
    let key = (distribution.to_owned(), home.map(Path::to_path_buf));
    let homes = || {
        WSL_HOMES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    };
    if let Some((resolved, windows, linux)) = homes().get(&key)
        && resolved.elapsed() < WSL_HOME_REUSE
    {
        return Ok((windows.clone(), linux.clone()));
    }
    let (windows, linux) = super::discovery::wsl_realpath(distribution, home).await?;
    homes().insert(key, (Instant::now(), windows.clone(), linux.clone()));
    Ok((windows, linux))
}
fn authority(credentials: &Credentials) -> Result<(Url, String)> {
    let provenance = credentials.document["tokens"].get("oauth_provenance");
    let invalid = || {
        failure(
            400,
            "CODEX_AUTHORITY_UNSUPPORTED",
            "This Codex OAuth authority requires a secure endpoint.",
        )
    };
    let Some(provenance) = provenance else {
        return Ok((
            Url::parse(DEFAULT_TOKEN_URL).expect("static OAuth URL"),
            DEFAULT_CLIENT.into(),
        ));
    };
    let issuer = provenance["issuer"].as_str().ok_or_else(invalid)?;
    let mut url = Url::parse(issuer).map_err(|_| invalid())?;
    ensure!(
        url.scheme() == "https"
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        invalid()
    );
    url.path_segments_mut()
        .map_err(|_| invalid())?
        .pop_if_empty()
        .extend(["oauth", "token"]);
    let client = provenance["client_id"]
        .as_str()
        .filter(|client| !client.is_empty())
        .ok_or_else(invalid)?;
    Ok((url, client.into()))
}
fn read_small(path: &Path) -> io::Result<Option<Vec<u8>>> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "expected file"));
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(MAX_CREDENTIAL_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_CREDENTIAL_BYTES {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "file too large"));
    }
    Ok(Some(bytes))
}
fn read_json(path: &Path) -> io::Result<Option<Value>> {
    read_small(path)?
        .map(|bytes| serde_json::from_slice(&bytes).map_err(io::Error::other))
        .transpose()
}
fn write_private_json(path: &Path, value: &Value) -> io::Result<()> {
    let directory = path
        .parent()
        .ok_or_else(|| io::Error::other("missing parent"))?;
    let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
    serde_json::to_writer(&mut temporary, value).map_err(io::Error::other)?;
    temporary.flush()?;
    temporary.as_file().sync_all()?;
    #[cfg(not(windows))]
    temporary.persist(path).map_err(|error| error.error)?;
    #[cfg(windows)]
    {
        // std::fs::rename can replace files opened for shared reads on Windows.
        let (file, temporary) = temporary.keep().map_err(|error| error.error)?;
        drop(file);
        let temporary = tempfile::TempPath::try_from_path(temporary)?;
        fs::rename(&temporary, path)?;
    }
    Ok(())
}
fn check_replaceable(path: &Path) -> io::Result<()> {
    let metadata = fs::metadata(path)?;
    if !metadata.is_file() || metadata.permissions().readonly() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "credential file must be writable",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "credential file has multiple hard links",
            ));
        }
    }
    OpenOptions::new().write(true).open(path)?;
    Ok(())
}
fn login_unavailable() -> anyhow::Error {
    failure(
        401,
        "CODEX_LOGIN_UNAVAILABLE",
        "No readable Codex subscription login was found. Sign in with Codex and try again.",
    )
}
fn unsupported_store() -> anyhow::Error {
    failure(
        400,
        "CODEX_STORAGE_UNSUPPORTED",
        "Choose a supported Codex file or native keyring credential store.",
    )
}
fn refresh_failed() -> anyhow::Error {
    failure(
        401,
        "CODEX_AUTH_EXPIRED",
        "The existing Codex login could not be refreshed. Sign in with Codex and try again.",
    )
}
fn refresh_uncertain() -> anyhow::Error {
    failure(
        409,
        "CODEX_REFRESH_UNCERTAIN",
        "A previous credential refresh did not finish. Sign in with Codex before retrying.",
    )
}
fn storage_locked() -> anyhow::Error {
    failure(
        409,
        "CODEX_STORAGE_LOCKED",
        "The Codex credentials are busy. Try again.",
    )
}
fn account_changed() -> anyhow::Error {
    failure(
        409,
        "CODEX_ACCOUNT_CHANGED",
        "The existing Codex account changed. Try again.",
    )
}
fn credentials_changed() -> anyhow::Error {
    failure(
        409,
        "CODEX_CREDENTIALS_CHANGED",
        "The existing Codex credentials changed before refresh could finish. Try again.",
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    fn session(expires: i64) -> Value {
        let claims = URL_SAFE_NO_PAD.encode(json!({"exp": expires}).to_string());
        json!({
            "auth_mode": "chatgpt",
            "tokens": {
                "access_token": format!("opaque.{claims}.example"),
                "refresh_token": "synthetic-refresh",
                "account_id": "synthetic-account",
            },
        })
    }
    fn home() -> (TempDir, CredentialHome) {
        let directory = tempfile::tempdir().unwrap();
        let home = CredentialHome {
            directory: directory.path().to_owned(),
            #[cfg(windows)]
            wsl: None,
        };
        (directory, home)
    }
    #[tokio::test]
    async fn reads_a_live_credential_without_rewriting_codex_storage() {
        let (_directory, home) = home();
        let document = session(Utc::now().timestamp() + 300);
        let auth_file = home.directory.join("auth.json");
        write_private_json(&auth_file, &document).unwrap();
        let auth = home.authorize(&Client::new(), None).await.unwrap();
        assert_eq!(auth.account_id.as_deref(), Some("synthetic-account"));
        assert_eq!(auth.access_token, document["tokens"]["access_token"]);
        assert_eq!(read_json(&auth_file).unwrap(), Some(document));
        assert!(!auth_file.with_extension(REFRESH_MARKER).is_file());
    }
    #[tokio::test]
    async fn refuses_to_replay_an_uncertain_refresh_grant() {
        let (_directory, home) = home();
        #[cfg(unix)]
        let auth_file = {
            let target = home.directory.join("session.json");
            std::os::unix::fs::symlink(&target, home.directory.join("auth.json")).unwrap();
            target
        };
        #[cfg(not(unix))]
        let auth_file = home.directory.join("auth.json");
        let document = session(Utc::now().timestamp() - 1);
        write_private_json(&auth_file, &document).unwrap();
        write_private_json(
            &auth_file.with_extension(REFRESH_MARKER),
            &json!({"generation": Credentials::new(document.clone(), Backend::File).unwrap().generation()}),
        )
        .unwrap();
        let error = home.authorize(&Client::new(), None).await.err().unwrap();
        assert_eq!(
            error
                .downcast_ref::<super::super::ProviderError>()
                .unwrap()
                .code,
            "CODEX_REFRESH_UNCERTAIN"
        );
        assert_eq!(read_json(&auth_file).unwrap(), Some(document));
        assert!(auth_file.with_extension("json.lock").is_file());
        #[cfg(unix)]
        assert!(home.directory.join("auth.json").is_symlink());
    }
}
