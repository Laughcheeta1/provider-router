//! Router settings, identifiers, and rules shared across entry points.
// RevExt: 149
use std::{
    net::{IpAddr, SocketAddr},
    path::PathBuf,
};
// RevExt: 150
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
// RevExt: 151
// RevExt: 3
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct State {
    pub instance_id: Uuid,
    pub connections: Vec<Connection>,
    pub clients: Vec<Client>,
    pub network: Network,
    pub startup_mode: StartupMode,
} // RevExt: 156
// RevExt: 4
impl Default for State {
    fn default() -> Self {
        Self {
            instance_id: Uuid::now_v7(),
            connections: Vec::new(),
            clients: Vec::new(),
            network: Network::default(),
            startup_mode: StartupMode::Startup,
        } // RevExt: 39
    } // RevExt: 53
} // RevExt: 157
// RevExt: 5
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum StartupMode {
    Startup,
    KeepAfterOpen,
    WhileOpen,
} // RevExt: 158
// RevExt: 7
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Network {
    pub host: String,
    pub port: u16,
    #[serde(default)]
    pub container_bridge_host: Option<IpAddr>,
    pub require_api_key: bool,
    pub browser_origins: Vec<String>,
} // RevExt: 159
// RevExt: 8
impl Default for Network {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".into(),
            port: 47831,
            container_bridge_host: None,
            require_api_key: false,
            browser_origins: Vec::new(),
        } // RevExt: 41
    } // RevExt: 55
} // RevExt: 160
// RevExt: 9
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
            }  // RevExt: 239
        }  // RevExt: 241
        let mut seen = std::collections::HashSet::new();
        self.browser_origins.retain(|origin| seen.insert(origin.clone()));
        self.validate()
    }  // RevExt: 243
// RevExt: 237
    pub fn validate(&self) -> Result<()> {
        ensure!(  // RevExt: 222
            // RevExt: 189
            // RevExt: 171
            matches!(self.host.as_str(), "127.0.0.1" | "::1" | "localhost"),
            "Use a loopback host (127.0.0.1, ::1, or localhost)."
        ); // RevExt: 176
        ensure!(self.port > 0, "The port must be between 1 and 65535.");
        ensure!(  // RevExt: 223
            // RevExt: 190
            // RevExt: 172
            self.browser_origins.len() <= 50,
            "Approve up to 50 origins."
        ); // RevExt: 177
        for origin in &self.browser_origins {
            let url = url::Url::parse(origin).context("Enter an exact website origin.")?;
            let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
            ensure!(  // RevExt: 228
                // RevExt: 194
                // RevExt: 181
                url.origin().ascii_serialization() == *origin
                    && (url.scheme() == "https" || url.scheme() == "http" && loopback),
                "Use an HTTPS website origin or a local HTTP origin, such as https://app.example.com."
            ); // RevExt: 184
        } // RevExt: 42
        Ok(()) // RevExt: 77
    } // RevExt: 56
    // RevExt: 152
    // RevExt: 10
    pub fn loopback_authority(&self) -> String {
        let host = if self.host == "::1" {
            "[::1]"
        } else {
            "127.0.0.1"
        }; // RevExt: 148
        format!("{host}:{}", self.port)
    } // RevExt: 37
    // RevExt: 197
    pub fn container_bridge_address(&self) -> Result<Option<SocketAddr>> {
        let Some(host) = self.container_bridge_host else {
            return Ok(None);
        }; // RevExt: 201
        let private = match host {
            IpAddr::V4(ip) => ip.is_private(),
            IpAddr::V6(ip) => ip.is_unique_local(),
        }; // RevExt: 202
        ensure!(  // RevExt: 224
            // RevExt: 199
            private,
            "Container bridge host {host} must be an RFC 1918 IPv4 or unique-local IPv6 address."
        ); // RevExt: 200
        Ok(Some(SocketAddr::new(host, self.port)))
    } // RevExt: 198
} // RevExt: 161
// RevExt: 153
// RevExt: 10
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Method {
    #[serde(rename = "codex_http")]
    Codex,
    #[serde(rename = "openrouter_http")]
    Openrouter,
} // RevExt: 162
// RevExt: 11
impl Method {
    pub fn provider(&self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Openrouter => "openrouter",
        } // RevExt: 43
    } // RevExt: 57
} // RevExt: 163
// RevExt: 12
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
} // RevExt: 164
// RevExt: 13
/// Codex catalog entries that are internal to Codex and never offered to clients.
pub fn hidden_codex_model(id: &str) -> bool {
    matches!(id, "gpt-reserve" | "codex-auto-review")
}
// RevExt: 245
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
} // RevExt: 165
// RevExt: 14
impl Connection {
    pub fn new(method: Method) -> Self {
        Self {
            id: Uuid::now_v7(),
            name: if method == Method::Codex {
                "Codex"
            } else {
                "OpenRouter"
            }  // RevExt: 240
            .into(),
            method,
            exposed: false,
            models: Vec::new(),
            codex_home: None,
            executable: None,
            wsl_distribution: None,
        } // RevExt: 44
    } // RevExt: 58
    // RevExt: 154
    pub fn normalize(&mut self, wsl_supported: bool) -> Result<()> {
        ensure!(  // RevExt: 225
            // RevExt: 191
            // RevExt: 173
            self.id.get_version_num() == 7,
            "Provider IDs must be UUIDv7."
        ); // RevExt: 178
        self.name = validated_name(&self.name)?;
        ensure!(  // RevExt: 226
            // RevExt: 192
            // RevExt: 174
            self.wsl_distribution.is_none() || (wsl_supported && self.method == Method::Codex),
            "WSL connections require Codex on Windows."
        ); // RevExt: 179
        if let Some(distribution) = &self.wsl_distribution {
            ensure!(  // RevExt: 229
                // RevExt: 195
                // RevExt: 182
                !distribution.is_empty()
                    && distribution.len() <= 128
                    && !distribution.contains(['\r', '\n', '\0']),
                "Choose a valid WSL distribution."
            ); // RevExt: 185
        } // RevExt: 49
        self.models
            .retain(|model| self.method != Method::Codex || !hidden_codex_model(&model.id));
        for model in &self.models {
            ensure!(  // RevExt: 230
                // RevExt: 196
                // RevExt: 183
                !model.id.is_empty()
                    && !model.name.is_empty()
                    && model.context_window > 0
                    && model.max_output_tokens > 0
                    && model
                        .default_reasoning_effort
                        .as_ref()
                        .is_none_or(|effort| model.reasoning_efforts.contains(effort)),
                "A selected model has invalid capabilities."
            ); // RevExt: 186
        } // RevExt: 50
        Ok(()) // RevExt: 187
    } // RevExt: 169
    // RevExt: 155
    pub fn validate_key(&self, key: &str) -> Result<()> {
        ensure!(  // RevExt: 227
            // RevExt: 193
            // RevExt: 175
            self.method == Method::Openrouter
                && !key.trim().is_empty()
                && key.len() <= 1024
                && !key.contains(['\r', '\n', '\0']),
            "Enter a valid OpenRouter API key."
        ); // RevExt: 180
        Ok(()) // RevExt: 188
    } // RevExt: 170
} // RevExt: 166
// RevExt: 15
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Client {
    pub id: Uuid,
    pub name: String,
    pub created_at: String,
    pub token_hash: String,
} // RevExt: 167
// RevExt: 38
pub fn validated_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 100 || name.chars().any(char::is_control) {
        bail!("Enter a name between 1 and 100 characters.");
    } // RevExt: 76
    Ok(name.into())
} // RevExt: 168
// RevExt: 203
#[cfg(test)]
mod tests {
    use super::*;
    // RevExt: 204
    #[test] // RevExt: 212
    fn bridge_addresses_are_rfc_1918_or_unique_local() {
        for host in [  // RevExt: 231
            "10.0.0.1",
            "172.16.0.1",
            "192.168.0.1",
            "fc00::1",
            "fd00::1",
        ] {  // RevExt: 233
            let network = Network {  // RevExt: 235
                // RevExt: 214
                container_bridge_host: Some(host.parse().unwrap()), // RevExt: 216
                ..Network::default()                                // RevExt: 218
            }; // RevExt: 220
            assert_eq!(
                network.container_bridge_address().unwrap(),
                network
                    .container_bridge_host
                    .map(|host| SocketAddr::new(host, network.port))
            ); // RevExt: 211
        } // RevExt: 207
        for host in [  // RevExt: 232
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
        ] {  // RevExt: 234
            let network = Network {  // RevExt: 236
                // RevExt: 215
                container_bridge_host: Some(host.parse().unwrap()), // RevExt: 217
                ..Network::default()                                // RevExt: 219
            }; // RevExt: 221
            assert!(network.container_bridge_address().is_err(), "{host}");
        } // RevExt: 208
    } // RevExt: 209
    // RevExt: 205
    #[test] // RevExt: 213
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
        for rejected in ["https://app.example.com/login", "http://app.example.com", "app.example.com"] {
            let mut network = Network {
                browser_origins: vec![rejected.into()],
                ..Network::default()
            };
            assert!(network.normalize().is_err(), "{rejected}");
        }  // RevExt: 242
    }  // RevExt: 244
// RevExt: 238
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
    } // RevExt: 210
} // RevExt: 206
