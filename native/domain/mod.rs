//! Router settings, identifiers, and rules shared across entry points.
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    net::{IpAddr, SocketAddr},
    path::PathBuf,
};
use uuid::Uuid;
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct State {
    pub instance_id: Uuid,
    pub connections: Vec<Connection>,
    pub clients: Vec<Client>,
    pub network: Network,
    pub startup_mode: StartupMode,
}
impl Default for State {
    fn default() -> Self {
        Self {
            instance_id: Uuid::now_v7(),
            connections: Vec::new(),
            clients: Vec::new(),
            network: Network::default(),
            startup_mode: StartupMode::Startup,
        }
    }
}
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum StartupMode {
    Startup,
    KeepAfterOpen,
    WhileOpen,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Network {
    pub host: String,
    pub port: u16,
    #[serde(default)]
    pub container_bridge_host: Option<IpAddr>,
    pub require_api_key: bool,
    pub browser_origins: Vec<String>,
}
impl Default for Network {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".into(),
            port: 47831,
            container_bridge_host: None,
            require_api_key: false,
            browser_origins: Vec::new(),
        }
    }
}
impl Network {
    /// Rewrite each approved origin to its exact form, such as `https://app.example.com/` to
    /// `https://app.example.com`, drop repeats, and validate the result.
    pub fn normalize(&mut self) -> Result<()> {
        for origin in &mut self.browser_origins {
            if let Ok(url) = url::Url::parse(origin.trim())
                && url.path() == "/"
                && url.query().is_none()
                && url.fragment().is_none()
                && url.username().is_empty()
                && url.password().is_none()
            {
                *origin = url.origin().ascii_serialization();
            }
        }
        let mut seen = std::collections::HashSet::new();
        self.browser_origins
            .retain(|origin| seen.insert(origin.clone()));
        self.validate()
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            matches!(self.host.as_str(), "127.0.0.1" | "::1" | "localhost"),
            "Use a loopback host (127.0.0.1, ::1, or localhost)."
        );
        ensure!(self.port > 0, "The port must be between 1 and 65535.");
        ensure!(
            self.browser_origins.len() <= 50,
            "Approve up to 50 origins."
        );
        for origin in &self.browser_origins {
            let url = url::Url::parse(origin).context("Enter an exact website origin.")?;
            let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
            ensure!(
                url.origin().ascii_serialization() == *origin
                    && (url.scheme() == "https" || url.scheme() == "http" && loopback),
                "Use an HTTPS website origin or a local HTTP origin, such as https://app.example.com."
            );
        }
        Ok(())
    }
    pub fn loopback_authority(&self) -> String {
        let host = if self.host == "::1" {
            "[::1]"
        } else {
            "127.0.0.1"
        };
        format!("{host}:{}", self.port)
    }
    pub fn container_bridge_address(&self) -> Result<Option<SocketAddr>> {
        let Some(host) = self.container_bridge_host else {
            return Ok(None);
        };
        let private = match host {
            IpAddr::V4(ip) => ip.is_private(),
            IpAddr::V6(ip) => ip.is_unique_local(),
        };
        ensure!(
            private,
            "Container bridge host {host} must be an RFC 1918 IPv4 or unique-local IPv6 address."
        );
        Ok(Some(SocketAddr::new(host, self.port)))
    }
}
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Method {
    #[serde(rename = "codex_http")]
    Codex,
    #[serde(rename = "openrouter_http")]
    Openrouter,
}
impl Method {
    pub fn provider(&self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Openrouter => "openrouter",
        }
    }
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Model {
    pub id: String,
    pub name: String,
    pub reasoning_efforts: Vec<String>,
    pub default_reasoning_effort: Option<String>,
    pub context_window: u64,
    pub max_output_tokens: u64,
    pub tools: bool,
    pub structured_output: bool,
    pub input_modalities: Vec<String>,
}
/// Codex catalog entries that are internal to Codex and never offered to clients.
pub fn hidden_codex_model(id: &str) -> bool {
    matches!(id, "gpt-reserve" | "codex-auto-review")
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Connection {
    pub id: Uuid,
    pub name: String,
    pub method: Method,
    pub exposed: bool,
    pub models: Vec<Model>,
    pub codex_home: Option<PathBuf>,
    pub executable: Option<PathBuf>,
    pub wsl_distribution: Option<String>,
}
impl Connection {
    pub fn new(method: Method) -> Self {
        Self {
            id: Uuid::now_v7(),
            name: if method == Method::Codex {
                "Codex"
            } else {
                "OpenRouter"
            }
            .into(),
            method,
            exposed: false,
            models: Vec::new(),
            codex_home: None,
            executable: None,
            wsl_distribution: None,
        }
    }
    pub fn normalize(&mut self, wsl_supported: bool) -> Result<()> {
        ensure!(
            self.id.get_version_num() == 7,
            "Provider IDs must be UUIDv7."
        );
        self.name = validated_name(&self.name)?;
        ensure!(
            self.wsl_distribution.is_none() || (wsl_supported && self.method == Method::Codex),
            "WSL connections require Codex on Windows."
        );
        if let Some(distribution) = &self.wsl_distribution {
            ensure!(
                !distribution.is_empty()
                    && distribution.len() <= 128
                    && !distribution.contains(['\r', '\n', '\0']),
                "Choose a valid WSL distribution."
            );
        }
        self.models
            .retain(|model| self.method != Method::Codex || !hidden_codex_model(&model.id));
        for model in &self.models {
            ensure!(
                !model.id.is_empty()
                    && !model.name.is_empty()
                    && model.context_window > 0
                    && model.max_output_tokens > 0
                    && model
                        .default_reasoning_effort
                        .as_ref()
                        .is_none_or(|effort| model.reasoning_efforts.contains(effort)),
                "A selected model has invalid capabilities."
            );
        }
        Ok(())
    }
    pub fn validate_key(&self, key: &str) -> Result<()> {
        ensure!(
            self.method == Method::Openrouter
                && !key.trim().is_empty()
                && key.len() <= 1024
                && !key.contains(['\r', '\n', '\0']),
            "Enter a valid OpenRouter API key."
        );
        Ok(())
    }
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Client {
    pub id: Uuid,
    pub name: String,
    pub created_at: String,
    pub token_hash: String,
}
pub fn validated_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 100 || name.chars().any(char::is_control) {
        bail!("Enter a name between 1 and 100 characters.");
    }
    Ok(name.into())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bridge_addresses_are_rfc_1918_or_unique_local() {
        for host in [
            "10.0.0.1",
            "172.16.0.1",
            "192.168.0.1",
            "fc00::1",
            "fd00::1",
        ] {
            let network = Network {
                container_bridge_host: Some(host.parse().unwrap()),
                ..Network::default()
            };
            assert_eq!(
                network.container_bridge_address().unwrap(),
                network
                    .container_bridge_host
                    .map(|host| SocketAddr::new(host, network.port))
            );
        }
        for host in [
            "0.0.0.0",
            "::",
            "127.0.0.1",
            "::1",
            "169.254.1.1",
            "fe80::1",
            "172.32.0.1",
            "8.8.8.8",
            "2001:4860::1",
            "::ffff:10.0.0.1",
        ] {
            let network = Network {
                container_bridge_host: Some(host.parse().unwrap()),
                ..Network::default()
            };
            assert!(network.container_bridge_address().is_err(), "{host}");
        }
    }
    #[test]
    fn origins_are_saved_in_their_exact_form() {
        let mut network = Network {
            browser_origins: vec![
                "https://App.Example.com/".into(),
                "http://localhost:5173/".into(),
                "https://app.example.com".into(),
            ],
            ..Network::default()
        };
        network.normalize().unwrap();
        assert_eq!(
            network.browser_origins,
            ["https://app.example.com", "http://localhost:5173"]
        );
        for rejected in [
            "https://app.example.com/login",
            "http://app.example.com",
            "app.example.com",
        ] {
            let mut network = Network {
                browser_origins: vec![rejected.into()],
                ..Network::default()
            };
            assert!(network.normalize().is_err(), "{rejected}");
        }
    }
    #[test]
    fn existing_network_settings_load_with_the_bridge_disabled() {
        let old = serde_json::json!({
            "host": "::1",
            "port": 47841,
            "require_api_key": true,
            "browser_origins": ["https://example.com"]
        });
        let mut network: Network = serde_json::from_value(old).unwrap();
        assert_eq!(network.host, "::1");
        assert_eq!(network.port, 47841);
        assert!(network.require_api_key);
        assert_eq!(network.browser_origins, ["https://example.com"]);
        assert!(network.container_bridge_host.is_none());
        assert!(!Network::default().require_api_key);
        network.container_bridge_host = Some("fd00::1".parse().unwrap());
        let encoded = serde_json::to_value(&network).unwrap();
        assert_eq!(encoded["container_bridge_host"], "fd00::1");
        assert_eq!(serde_json::from_value::<Network>(encoded).unwrap(), network);
    }
}
