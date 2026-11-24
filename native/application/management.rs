//! Shared rules for managing providers and startup settings.
use crate::domain::{Connection, Method, Model, StartupMode, State};
use anyhow::{Context, Result};
use serde::Serialize;
use uuid::Uuid;
pub trait SettingsRepository {
    fn read_settings(&self) -> Result<State>;
    fn change_settings<T>(&self, change: impl FnOnce(&mut State) -> Result<T>) -> Result<T>;
    fn inspect_settings<T>(&self, inspect: impl FnOnce(&State) -> Result<T>) -> Result<T>;
    fn provider_key(&self, id: Uuid) -> Result<Option<String>>;
}
pub fn set_startup(
    store: &impl SettingsRepository,
    mode: StartupMode,
    register: impl FnOnce(bool) -> Result<()>,
) -> Result<()> {
    store.change_settings(|state| {
        if mode == StartupMode::Startup || state.startup_mode == StartupMode::Startup {
            register(mode == StartupMode::Startup)?;
        }
        state.startup_mode = mode;
        Ok(())
    })
}
pub fn register_startup_if_enabled(
    store: &impl SettingsRepository,
    register: impl FnOnce(bool) -> Result<()>,
) -> Result<()> {
    store.inspect_settings(|state| {
        if state.startup_mode == StartupMode::Startup {
            register(true)?;
        }
        Ok(())
    })
}
#[derive(Clone, Serialize)]
pub struct Discovered {
    pub connection: Connection,
    pub status: &'static str,
    pub detail: &'static str,
}
pub fn configured_installation(connection: &Connection, candidate: &Connection) -> bool {
    connection.method == Method::Codex
        && connection.wsl_distribution == candidate.wsl_distribution
        && connection.codex_home.is_none()
        && connection.executable.is_none()
}
pub fn connection(store: &impl SettingsRepository, id: Uuid) -> Result<Connection> {
    store
        .read_settings()?
        .connections
        .into_iter()
        .find(|connection| connection.id == id)
        .context("The provider is no longer configured.")
}
pub fn change_connection<T>(
    store: &impl SettingsRepository,
    id: Uuid,
    change: impl FnOnce(&mut Connection) -> T,
) -> Result<T> {
    store.change_settings(|state| {
        let connection = state
            .connections
            .iter_mut()
            .find(|connection| connection.id == id)
            .context("The provider is no longer configured.")?;
        Ok(change(connection))
    })
}
pub async fn catalog(
    store: &impl SettingsRepository,
    connection: &Connection,
    fetch: impl AsyncFnOnce(&Connection, Option<&str>) -> Result<Vec<Model>>,
) -> Result<Vec<Model>> {
    let key = store.provider_key(connection.id)?;
    fetch(connection, key.as_deref()).await
}
