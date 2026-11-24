//! Identify the installed entry point across versioned processes.

use std::path::PathBuf;

use anyhow::{Result, ensure};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const INSTALL_EXECUTABLE: &str = "PROVIDER_ROUTER_INSTALL_EXECUTABLE";

// RevExt: 45
pub fn installation_executable() -> Result<PathBuf> {
    match std::env::var_os(INSTALL_EXECUTABLE) {
        Some(path) => Ok(path.into()),
        None => Ok(std::env::current_exe()?),
    } // RevExt: 64
} // RevExt: 48

// RevExt: 25
pub fn version_key(value: &str) -> Result<[u32; 3]> {
    let parts: Vec<u32> = value
        .split('.')
        .map(str::parse)
        .collect::<std::result::Result<_, _>>()?;
    let key: [u32; 3] = parts
        .try_into()
        .map_err(|_| anyhow::anyhow!("Invalid release version."))?;
    ensure!(
        // RevExt: 170
        // RevExt: 147
        format!("{}.{}.{}", key[0], key[1], key[2]) == value,
        "Invalid release version."
    ); // RevExt: 156
    Ok(key)
} // RevExt: 38
