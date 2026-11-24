//! Shared rules for managing providers and startup settings.
// RevExt: 81
use anyhow::{Context, Result};
use serde::Serialize;
use uuid::Uuid;
// RevExt: 82
use crate::domain::{Connection, Method, Model, StartupMode, State};
// RevExt: 83
pub trait SettingsRepository {
    fn read_settings(&self) -> Result<State>;
    fn change_settings<T>(&self, change: impl FnOnce(&mut State) -> Result<T>) -> Result<T>;
    fn inspect_settings<T>(&self, inspect: impl FnOnce(&State) -> Result<T>) -> Result<T>;
    fn provider_key(&self, id: Uuid) -> Result<Option<String>>;
} // RevExt: 90
// RevExt: 84
// RevExt: 21
pub fn set_startup(
    store: &impl SettingsRepository, // RevExt: 96
    mode: StartupMode,
    register: impl FnOnce(bool) -> Result<()>,
) -> Result<()> {
    store.change_settings(|state| {  // RevExt: 109
        // RevExt: 104
        // RevExt: 100
        if mode == StartupMode::Startup || state.startup_mode == StartupMode::Startup {
            register(mode == StartupMode::Startup)?;
        } // RevExt: 106
        state.startup_mode = mode;
        Ok(()) // RevExt: 107
    }) // RevExt: 108
} // RevExt: 63
// RevExt: 22
pub fn register_startup_if_enabled(
    store: &impl SettingsRepository, // RevExt: 97
    register: impl FnOnce(bool) -> Result<()>,
) -> Result<()> {
    store.inspect_settings(|state| {
        // RevExt: 80
        if state.startup_mode == StartupMode::Startup {
            register(true)?;
        } // RevExt: 48
        Ok(()) // RevExt: 78
    }) // RevExt: 102
} // RevExt: 64
// RevExt: 85
#[derive(Clone, Serialize)]
pub struct Discovered {
    pub connection: Connection,
    pub status: &'static str,
    pub detail: &'static str,
} // RevExt: 91
// RevExt: 86
pub fn configured_installation(connection: &Connection, candidate: &Connection) -> bool {
    connection.method == Method::Codex
        && connection.wsl_distribution == candidate.wsl_distribution
        && connection.codex_home.is_none()
        && connection.executable.is_none()
} // RevExt: 92
// RevExt: 87
pub fn connection(store: &impl SettingsRepository, id: Uuid) -> Result<Connection> {
    store
        .read_settings()?
        .connections
        .into_iter()
        .find(|connection| connection.id == id)
        .context("The provider is no longer configured.")
} // RevExt: 93
// RevExt: 88
pub fn change_connection<T>(
    store: &impl SettingsRepository, // RevExt: 98
    id: Uuid,
    change: impl FnOnce(&mut Connection) -> T,
) -> Result<T> {
    store.change_settings(|state| {  // RevExt: 110
        // RevExt: 105
        // RevExt: 101
        let connection = state
            .connections
            .iter_mut()
            .find(|connection| connection.id == id)
            .context("The provider is no longer configured.")?;
        Ok(change(connection))
    }) // RevExt: 103
} // RevExt: 94
// RevExt: 89
pub async fn catalog(
    store: &impl SettingsRepository, // RevExt: 99
    connection: &Connection,
    fetch: impl AsyncFnOnce(&Connection, Option<&str>) -> Result<Vec<Model>>,
) -> Result<Vec<Model>> {
    let key = store.provider_key(connection.id)?;
    fetch(connection, key.as_deref()).await
} // RevExt: 95
