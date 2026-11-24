//! Own the account's settings, credential vault, and cross-process write lock.
use crate::{
    application::management::SettingsRepository,
    domain::{Client, Connection, Network, State, validated_name},
};
use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit, OsRng, rand_core::RngCore},
};
#[cfg(any(target_os = "macos", windows))]
use anyhow::bail;
use anyhow::{Context, Result, anyhow, ensure};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};
use uuid::Uuid;
#[derive(Default, Deserialize, Serialize)]
struct StoredState {
    #[serde(flatten)]
    settings: State,
    credentials: std::collections::BTreeMap<Uuid, String>,
}
#[derive(Clone, Debug)]
pub struct Profile {
    pub dir: PathBuf,
    // The vault key never changes once created, so each process unlocks it at most once.
    vault: Arc<OnceLock<[u8; 32]>>,
}
impl Profile {
    pub fn open(dir: Option<PathBuf>) -> Result<Self> {
        let dir = match dir {
            Some(dir) => dir,
            None => dirs::config_dir()
                .context("Could not locate the account settings directory.")?
                .join("provider-router-rust"),
        };
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(&dir)
            .context("Could not create the router settings directory.")?;
        let profile = Self {
            dir: dir.canonicalize()?,
            vault: Arc::default(),
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let metadata = fs::metadata(&profile.dir)?;
            // geteuid reads the identity of this process and takes no pointers.
            ensure!(
                metadata.uid() == unsafe { libc::geteuid() } && metadata.mode() & 0o077 == 0,
                "The settings directory must belong to this account and have permissions 0700."
            );
        }
        let _lock = profile.write_lock()?;
        if !profile.dir.join("router.json").exists() {
            profile.persist(&StoredState::default())?;
        }
        Ok(profile)
    }
    pub fn read(&self) -> Result<State> {
        Ok(self.read_stored()?.settings)
    }
    fn read_stored(&self) -> Result<StoredState> {
        // Reading the whole file avoids one system call per byte from an unbuffered reader.
        serde_json::from_slice(&fs::read(self.dir.join("router.json"))?)
            .context("The router settings could not be read.")
    }
    pub fn update<T>(&self, change: impl FnOnce(&mut State) -> Result<T>) -> Result<T> {
        self.update_stored(|stored| change(&mut stored.settings))
    }
    fn update_stored<T>(&self, change: impl FnOnce(&mut StoredState) -> Result<T>) -> Result<T> {
        let _lock = self.write_lock()?;
        let mut state = self.read_stored()?;
        let result = change(&mut state)?;
        self.persist(&state)?;
        Ok(result)
    }
    pub fn save_network(&self, network: Network) -> Result<()> {
        self.update_network(false, |saved| {
            *saved = network;
        })
        .map(|_| ())
    }
    pub fn update_network(
        &self,
        validate_bridge_host: bool,
        change: impl FnOnce(&mut Network),
    ) -> Result<Network> {
        self.update(|state| {
            let network = &mut state.network;
            let previous_bridge_host = network.container_bridge_host;
            change(network);
            network.normalize()?;
            if validate_bridge_host || network.container_bridge_host != previous_bridge_host {
                super::network::container_bridge_address(network)?;
            }
            Ok(network.clone())
        })
    }
    pub fn create_client(&self, name: &str) -> Result<(Uuid, String)> {
        let name = validated_name(name)?;
        let token = random_token();
        let id = Uuid::now_v7();
        self.update(|state| {
            state.clients.push(Client {
                id,
                name,
                created_at: chrono::Utc::now().to_rfc3339(),
                token_hash: digest(token.as_bytes()),
            });
            Ok(())
        })?;
        Ok((id, token))
    }
    pub fn revoke_client(&self, id: Uuid) -> Result<()> {
        self.update(|state| {
            state.clients.retain(|client| client.id != id);
            Ok(())
        })
    }
    pub fn save_connection(&self, mut connection: Connection, key: Option<&str>) -> Result<()> {
        connection.normalize(cfg!(windows))?;
        self.update_stored(|state| {
            if let Some(key) = key {
                connection.validate_key(key)?;
                state
                    .credentials
                    .insert(connection.id, self.encrypt(key.trim())?);
            }
            match state
                .settings
                .connections
                .iter_mut()
                .find(|item| item.id == connection.id)
            {
                Some(existing) => *existing = connection,
                None => state.settings.connections.push(connection),
            }
            Ok(())
        })
    }
    pub fn remove_connection(&self, id: Uuid) -> Result<()> {
        self.update_stored(|state| {
            state
                .settings
                .connections
                .retain(|connection| connection.id != id);
            state.credentials.remove(&id);
            Ok(())
        })
    }
    pub fn secret(&self, id: Uuid) -> Result<Option<String>> {
        let state = self.read_stored()?;
        let Some(encoded) = state.credentials.get(&id) else {
            return Ok(None);
        };
        let bytes = URL_SAFE_NO_PAD
            .decode(encoded)
            .context("The saved provider key could not be read.")?;
        ensure!(
            bytes.len() >= 28,
            "The saved provider key could not be read."
        );
        let (nonce, encrypted) = bytes.split_at(12);
        let cipher = Aes256Gcm::new_from_slice(&self.unlocked_vault(false)?)
            .map_err(|_| anyhow!("Invalid vault key."))?;
        let plain = cipher
            .decrypt(Nonce::from_slice(nonce), encrypted)
            .map_err(|_| anyhow!("The saved provider key could not be unlocked."))?;
        Ok(Some(
            String::from_utf8(plain).context("The saved provider key is invalid.")?,
        ))
    }
    fn encrypt(&self, value: &str) -> Result<String> {
        let cipher = Aes256Gcm::new_from_slice(&self.unlocked_vault(true)?)
            .map_err(|_| anyhow!("Invalid vault key."))?;
        let nonce: [u8; 12] = random_bytes();
        let encrypted = cipher
            .encrypt(Nonce::from_slice(&nonce), value.as_bytes())
            .map_err(|_| anyhow!("Could not encrypt the provider key."))?;
        Ok(URL_SAFE_NO_PAD.encode([nonce.as_slice(), &encrypted].concat()))
    }
    fn persist(&self, state: &StoredState) -> Result<()> {
        write_private(
            &self.dir.join("router.json"),
            &serde_json::to_vec_pretty(state)?,
        )
    }
    fn unlocked_vault(&self, create: bool) -> Result<[u8; 32]> {
        if let Some(key) = self.vault.get() {
            return Ok(*key);
        }
        let key = self.vault_key(create)?;
        Ok(*self.vault.get_or_init(|| key))
    }
    fn write_lock(&self) -> Result<File> {
        let file = private_file(&self.dir.join("settings.lock"))?;
        fs2::FileExt::lock_exclusive(&file).context("Could not lock the router settings.")?;
        Ok(file)
    }
    #[cfg(target_os = "linux")]
    fn vault_key(&self, create: bool) -> Result<[u8; 32]> {
        use std::os::unix::fs::MetadataExt;
        let path = self.dir.join("vault.key");
        match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                ensure!(
                    metadata.is_file()
                        && metadata.uid() == unsafe { libc::geteuid() }
                        && metadata.mode() & 0o077 == 0,
                    "The vault key must be a private file owned by this account."
                );
                fs::read(path)?
                    .try_into()
                    .map_err(|_| anyhow!("The vault key is invalid."))
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound && create => {
                let key = random_bytes();
                write_private(&path, &key)?;
                Ok(key)
            }
            Err(err) => Err(err).context("The provider credential vault could not be opened."),
        }
    }
    #[cfg(any(target_os = "macos", windows))]
    fn vault_key(&self, create: bool) -> Result<[u8; 32]> {
        let entry = keyring::Entry::new(
            "Provider Router",
            &digest(self.dir.to_string_lossy().as_bytes()),
        )
        .context("The platform credential vault could not be opened.")?;
        match entry.get_password() {
            Ok(encoded) => URL_SAFE_NO_PAD
                .decode(encoded)?
                .try_into()
                .map_err(|_| anyhow!("The vault key is invalid.")),
            Err(keyring::Error::NoEntry) if create => {
                let key = random_bytes();
                entry
                    .set_password(&URL_SAFE_NO_PAD.encode(key))
                    .context("The platform credential vault could not save the vault key.")?;
                Ok(key)
            }
            Err(_) => bail!("The platform credential vault could not be unlocked."),
        }
    }
}
pub fn private_file(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).truncate(false).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    Ok(options.open(path)?)
}
pub fn write_private(path: &Path, content: &[u8]) -> Result<()> {
    let directory = path
        .parent()
        .context("The configuration path must have a parent directory.")?;
    let mut temp = tempfile::NamedTempFile::new_in(directory)?;
    temp.write_all(content)?;
    temp.as_file().sync_all()?;
    temp.persist(path)?;
    Ok(())
}
pub fn digest(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}
pub fn random_token() -> String {
    URL_SAFE_NO_PAD.encode(random_bytes::<32>())
}
fn random_bytes<const N: usize>() -> [u8; N] {
    let mut value = [0; N];
    OsRng.fill_bytes(&mut value);
    value
}
impl SettingsRepository for Profile {
    fn read_settings(&self) -> Result<State> {
        self.read()
    }
    fn change_settings<T>(&self, change: impl FnOnce(&mut State) -> Result<T>) -> Result<T> {
        self.update(change)
    }
    fn inspect_settings<T>(&self, inspect: impl FnOnce(&State) -> Result<T>) -> Result<T> {
        let _lock = self.write_lock()?;
        inspect(&self.read()?)
    }
    fn provider_key(&self, id: Uuid) -> Result<Option<String>> {
        self.secret(id)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn saves_network_edits_while_the_configured_bridge_is_unavailable() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let profile = Profile::open(Some(temp.path().join("profile")))?;
        let unavailable_host = "fd4d:530d:76d4:2fb9::1407".parse()?;
        profile.update(|state| {
            state.network.container_bridge_host = Some(unavailable_host);
            Ok(())
        })?;
        let mut network = profile.read()?.network;
        network.require_api_key = true;
        profile.save_network(network.clone())?;
        assert_eq!(profile.read()?.network, network);
        let mut invalid = network.clone();
        invalid.container_bridge_host = Some("127.0.0.1".parse()?);
        assert!(profile.save_network(invalid).is_err());
        assert_eq!(profile.read()?.network, network);
        Ok(())
    }
}
