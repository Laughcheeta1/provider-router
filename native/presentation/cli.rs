//! Scriptable settings and lifecycle commands for the native executable.
// RevExt: 1
use std::{
    io::{self, IsTerminal, Read},
    path::PathBuf,
};  // RevExt: 116
// RevExt: 2
use anyhow::{Context, Result, bail, ensure};
use clap::{
    Args, Subcommand, ValueEnum,
    builder::{PossibleValue, PossibleValuesParser, TypedValueParser},
};  // RevExt: 117
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;
// RevExt: 3
use crate::{
    application::management,
    domain::{Connection, Method, Network, StartupMode},
    infrastructure::{
        profile::Profile,
        provider, startup,
        supervisor::{self as runtime, ServeMode},
    },  // RevExt: 119
};  // RevExt: 118
// RevExt: 4
#[derive(Subcommand)]
pub enum Command {
    /// Run the API in the foreground.
    Serve,
    /// Show the listener and current settings.
    Status,
    /// Stop the running API.
    Stop,
    /// Read or set the router startup behavior.
    Startup {
        #[arg(value_parser = startup_parser())]
        mode: Option<StartupMode>,
    },  // RevExt: 78
    /// Discover, configure, and expose inference providers.
    Providers {
        #[command(subcommand)]
        command: ProviderCommand,
    },  // RevExt: 79
    /// Manage revocable credentials for API clients.
    Clients {
        #[command(subcommand)]
        command: ClientCommand,
    },  // RevExt: 80
    /// Configure the API listener and approved browser origins.
    Network {
        #[command(subcommand)]
        command: NetworkCommand,
    },  // RevExt: 81
    /// Apply a JSON settings command supplied on stdin.
    Settings {
        #[command(subcommand)]
        command: SettingsCommand,
    },  // RevExt: 82
    #[command(name = "__serve", hide = true)]
    InternalServe { mode: ServeMode },
    #[command(name = "__worker", hide = true)]
    InternalWorker,
    #[command(name = "__handoff", hide = true)]
    InternalHandoff,
    #[command(name = "__install", hide = true)]
    InternalInstall {
        #[arg(long)]  // RevExt: 127
        destination: PathBuf,
    },
    #[command(name = "__uninstall", hide = true)]
    InternalUninstall,
}  // RevExt: 26
// RevExt: 5
#[derive(Subcommand)]
pub enum ProviderCommand {
    List,
    Discover,
    /// Use the native Codex installation or a named WSL distribution.
    Use {
        installation: String,
    },  // RevExt: 83
    Add {
        #[arg(value_parser = method_parser())]
        method: Method,
        #[arg(long)]  // RevExt: 128
        wsl: Option<String>,
        #[command(flatten)]
        fields: ProviderFields,
    },  // RevExt: 84
    Edit {
        id: String,
        #[command(flatten)]
        fields: ProviderFields,
    },  // RevExt: 85
    Enable {
        id: String,
    },  // RevExt: 86
    Disable {
        id: String,
    },  // RevExt: 87
    Remove {
        id: String,
    },  // RevExt: 88
    Models {
        id: String,
    },  // RevExt: 89
    Select {
        id: String,
        model: String,
        #[arg(long, value_delimiter = ',')]
        efforts: Option<Vec<String>>,
    },  // RevExt: 90
    Deselect {
        id: String,
        model: String,
    },  // RevExt: 91
}  // RevExt: 27
// RevExt: 6
#[derive(Args)]
pub struct ProviderFields {
    #[arg(long)]
    name: Option<String>,
    #[arg(long)]
    home: Option<PathBuf>,
    #[arg(long)]
    executable: Option<PathBuf>,
    /// Read the OpenRouter API key from a pipe.
    #[arg(long)]
    key_stdin: bool,
}  // RevExt: 28
// RevExt: 7
#[derive(Subcommand)]
pub enum ClientCommand {
    List,
    Create { name: String },
    Revoke { id: String },
}  // RevExt: 29
// RevExt: 8
#[derive(Subcommand)]
pub enum NetworkCommand {
    Show,
    Set {
        /// Loopback host for applications on this computer.
        #[arg(long)]  // RevExt: 129
        host: Option<String>,
        #[arg(long)]  // RevExt: 130
        port: Option<u16>,
        /// Private IP assigned to the Linux host's Docker bridge.
        #[arg(long, conflicts_with = "clear_container_bridge_host")]
        container_bridge_host: Option<std::net::IpAddr>,
        /// Disable access through the Docker bridge.
        #[arg(long)]  // RevExt: 131
        clear_container_bridge_host: bool,
        #[arg(long, conflicts_with = "no_require_key")]
        require_key: bool,
        #[arg(long)]  // RevExt: 132
        no_require_key: bool,
        #[arg(long = "origin", conflicts_with = "clear_origins")]
        origins: Vec<String>,
        #[arg(long)]  // RevExt: 133
        clear_origins: bool,
    },  // RevExt: 92
}  // RevExt: 30
// RevExt: 9
#[derive(Subcommand)]
pub enum SettingsCommand {
    Apply,
}  // RevExt: 31
// RevExt: 10
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Apply {
    Status,
    Snapshot,
    SaveConnection {
        connection: Connection,
        api_key: Option<String>,
    },  // RevExt: 93
    RemoveConnection {
        id: Uuid,
    },  // RevExt: 94
    RefreshModels {
        id: Uuid,
    },  // RevExt: 95
    CreateClient {
        name: String,
    },  // RevExt: 96
    RevokeClient {
        id: Uuid,
    },  // RevExt: 97
    SaveNetwork {
        network: Network,
    },  // RevExt: 98
    SaveStartup {
        mode: StartupMode,
    },  // RevExt: 99
}  // RevExt: 32
// RevExt: 11
pub async fn execute(profile: Profile, command: Command) -> Result<()> {
    let result = match command {
        Command::Serve => return runtime::serve(profile, ServeMode::Foreground).await,
        Command::InternalServe { mode } => return runtime::serve(profile, mode).await,
        Command::InternalWorker => unreachable!("The executable dispatches workers at startup."),
        Command::InternalHandoff => return runtime::receive_handoff(profile).await,
        Command::InternalInstall { destination } => {
            return crate::infrastructure::installer::install(&profile, &destination).await;
        }
        Command::InternalUninstall => {
            return crate::infrastructure::installer::uninstall(&profile).await;
        }
        Command::Status => status(&profile).await?,
        Command::Stop => {
            runtime::stop(&profile).await?;
            Value::Null
        }  // RevExt: 102
        Command::Startup { mode } => {
            if let Some(mode) = mode {
                startup::set_mode(&profile, mode)?;
            }
            json!({"mode": profile.read()?.startup_mode})
        }  // RevExt: 103
        Command::Providers { command } => providers(&profile, command).await?,
        Command::Clients { command } => match command {
            ClientCommand::List => public_clients(&profile)?,
            ClientCommand::Create { name } => create_client(&profile, &name)?,
            ClientCommand::Revoke { id } => {
                profile.revoke_client(v7(&id)?)?;
                Value::Null
            }
        },
        Command::Network { command } => match command {
            NetworkCommand::Show => json!(profile.read()?.network),
            NetworkCommand::Set {
                host,
                port,
                container_bridge_host,
                clear_container_bridge_host,
                require_key,
                no_require_key,
                origins,
                clear_origins,
            } => json!(profile.update_network(container_bridge_host.is_some(), |network| {
                if let Some(host) = host {
                    network.host = host;
                }  // RevExt: 134
                if let Some(port) = port {
                    network.port = port;
                }  // RevExt: 135
                if clear_container_bridge_host || container_bridge_host.is_some() {
                    network.container_bridge_host = container_bridge_host;
                }  // RevExt: 136
                if require_key || no_require_key {
                    network.require_api_key = require_key;
                }  // RevExt: 137
                if clear_origins || !origins.is_empty() {
                    network.browser_origins = origins;
                }  // RevExt: 138
            })?),
        },
        Command::Settings {
            command: SettingsCommand::Apply,
        } => {
            let request = serde_json::from_str(&read_stdin()?)
                .map_err(|_| anyhow::anyhow!("Supply a valid JSON settings command on stdin."))?;
            apply(&profile, request).await?
        }  // RevExt: 104
    };
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}  // RevExt: 33
// RevExt: 12
async fn providers(profile: &Profile, command: ProviderCommand) -> Result<Value> {
    match command {
        ProviderCommand::List => Ok(json!(profile.read()?.connections)),
        ProviderCommand::Discover => Ok(json!(provider::discover().await?)),
        ProviderCommand::Use { installation } => {
            let candidates = provider::discover().await?;
            let candidate = candidates
                .into_iter()  // RevExt: 120
                .find(|item| {
                    item.status == "ready"
                        && item
                            .connection
                            .wsl_distribution
                            .as_deref()
                            .unwrap_or("native")
                            == installation
                })
                .context("Choose a ready installation from provider-router providers discover.")?;
            let connection = candidate.connection;
            if let Some(existing) = profile
                .read()?
                .connections
                .into_iter()  // RevExt: 121
                .find(|item| management::configured_installation(item, &connection))
            {
                return Ok(json!(existing));
            }
            profile.save_connection(connection.clone(), None)?;
            Ok(json!(connection))
        }  // RevExt: 105
        ProviderCommand::Add {
            method,
            wsl,
            fields,  // RevExt: 123
        } => {
            let mut connection = Connection::new(method);
            connection.wsl_distribution = wsl;
            change_connection(profile, connection, fields, true)
        }  // RevExt: 106
        ProviderCommand::Edit { id, fields } => change_connection(
            profile,
            management::connection(profile, v7(&id)?)?,
            fields,  // RevExt: 124
            false,
        ),  // RevExt: 107
        ProviderCommand::Enable { id } => expose(profile, v7(&id)?, true),
        ProviderCommand::Disable { id } => expose(profile, v7(&id)?, false),
        ProviderCommand::Remove { id } => {
            profile.remove_connection(v7(&id)?)?;
            Ok(Value::Null)
        }  // RevExt: 108
        ProviderCommand::Models { id } => models(profile, v7(&id)?).await,
        ProviderCommand::Select { id, model, efforts } => {
            let id = v7(&id)?;
            let connection = management::connection(profile, id)?;
            let mut selection = provider::Catalog::new()?.models(profile, &connection)
                .await?
                .into_iter()  // RevExt: 122
                .find(|available| available.id == model)
                .context("Run provider-router providers models to list available models.")?;
            if let Some(efforts) = efforts {
                ensure!(
                    !efforts.is_empty()
                        && efforts
                            .iter()
                            .all(|effort| selection.reasoning_efforts.contains(effort)),
                    "Choose thinking efforts listed for the model."
                );
                selection.default_reasoning_effort = selection
                    .default_reasoning_effort
                    .filter(|effort| efforts.contains(effort))
                    .or_else(|| efforts.first().cloned());
                selection.reasoning_efforts = efforts;
            }
            management::change_connection(profile, id, |connection| {  // RevExt: 125
  // RevExt: 46
                connection.models.retain(|model| model.id != selection.id);
                connection.models.push(selection);
                json!(connection.models)  // RevExt: 48
            })
        }  // RevExt: 109
        ProviderCommand::Deselect { id, model } => {
            let id = v7(&id)?;
            management::change_connection(profile, id, |connection| {  // RevExt: 126
  // RevExt: 47
                connection.models.retain(|selected| selected.id != model);
                json!(connection.models)  // RevExt: 49
            })
        }  // RevExt: 110
    }  // RevExt: 50
}  // RevExt: 34
// RevExt: 13
fn change_connection(
    profile: &Profile,
    mut connection: Connection,
    fields: ProviderFields,
    adding: bool,
) -> Result<Value> {
    ensure!(
        !(fields.key_stdin && connection.method != Method::Openrouter),
        "Only OpenRouter uses a supplied API key."
    );
    if connection.method == Method::Openrouter {
        ensure!(
            fields.home.is_none() && fields.executable.is_none(),
            "Only Codex uses a home directory or executable."
        );
        ensure!(
            !adding || fields.key_stdin,
            "Supply the OpenRouter API key with --key-stdin."
        );
    }  // RevExt: 51
    if let Some(name) = fields.name {
        connection.name = name.trim().into();
    }  // RevExt: 52
    if let Some(home) = fields.home {
        connection.codex_home = (!home.as_os_str().is_empty()).then_some(home);
    }  // RevExt: 53
    if let Some(executable) = fields.executable {
        connection.executable = (!executable.as_os_str().is_empty()).then_some(executable);
    }  // RevExt: 54
    let key = fields.key_stdin.then(read_stdin).transpose()?;
    profile.save_connection(connection.clone(), key.as_deref().map(str::trim))?;
    Ok(json!(connection))
}  // RevExt: 35
// RevExt: 14
fn expose(profile: &Profile, id: Uuid, exposed: bool) -> Result<Value> {
    management::change_connection(profile, id, |connection| {
        connection.exposed = exposed;
        json!(connection)
    })  // RevExt: 113
}  // RevExt: 36
// RevExt: 15
async fn models(profile: &Profile, id: Uuid) -> Result<Value> {
    let connection = management::connection(profile, id)?;
    Ok(json!(provider::Catalog::new()?.models(profile, &connection).await?))
}  // RevExt: 37
// RevExt: 16
async fn status(profile: &Profile) -> Result<Value> {
    let state = profile.read()?;
    let runtime = runtime::status(profile).await?;
    Ok(json!({
        "instance_id": state.instance_id,
        "connections": state.connections,
        "clients": public_clients(profile)?,
        "network": state.network,
        "startup_mode": state.startup_mode,
        "endpoint": runtime.endpoint,
        "container_bridge_endpoint": runtime.container_bridge_endpoint,
        "error": runtime.error,
        "worker_version": runtime.worker_version,
        "supervisor_version": runtime.supervisor_version,
    }))
}  // RevExt: 38
// RevExt: 17
fn public_clients(profile: &Profile) -> Result<Value> {
    Ok(json!(
        profile
            .read()?
            .clients
            .into_iter()
            .map(
                |client| json!({"id":client.id, "name":client.name, "created_at":client.created_at})
            )
            .collect::<Vec<_>>()
    ))
}  // RevExt: 39
// RevExt: 18
fn create_client(profile: &Profile, name: &str) -> Result<Value> {
    let (id, token) = profile.create_client(name)?;
    Ok(json!({"id":id,"token":token}))
}  // RevExt: 40
// RevExt: 19
async fn apply(profile: &Profile, action: Apply) -> Result<Value> {
    match action {
        Apply::Status => return status(profile).await,
        Apply::Snapshot => {
            let mut value = status(profile).await?;
            value["detected_providers"] = json!(provider::discover().await?);
            value["platform"] = json!(std::env::consts::OS);
            return Ok(value);
        }  // RevExt: 111
        Apply::SaveConnection {
            connection,
            api_key,
        } => profile.save_connection(connection, api_key.as_deref())?,
        Apply::RemoveConnection { id } => profile.remove_connection(id)?,
        Apply::RefreshModels { id } => return models(profile, id).await,
        Apply::CreateClient { name } => return create_client(profile, &name),
        Apply::RevokeClient { id } => profile.revoke_client(id)?,
        Apply::SaveNetwork { network } => profile.save_network(network)?,
        Apply::SaveStartup { mode } => startup::set_mode(profile, mode)?,
    }  // RevExt: 55
    Ok(Value::Null)
}  // RevExt: 41
// RevExt: 20
fn v7(value: &str) -> Result<Uuid> {
    let id = Uuid::parse_str(value).context("Enter a UUIDv7 identifier.")?;
    ensure!(id.get_version_num() == 7, "Enter a UUIDv7 identifier.");
    Ok(id)
}  // RevExt: 42
// RevExt: 21
fn read_stdin() -> Result<String> {
    ensure!(
        !io::stdin().is_terminal(),
        "Pipe the value to stdin or enter it in the terminal app."
    );
    const LIMIT: u64 = 2 * 1024 * 1024;
    let mut input = String::new();
    io::stdin()
        .lock()
        .take(LIMIT + 1)
        .read_to_string(&mut input)?;
    if input.len() as u64 > LIMIT {
        bail!("The settings command is too large.");
    }  // RevExt: 56
    Ok(input)
}  // RevExt: 43
// RevExt: 22
fn startup_parser() -> impl TypedValueParser<Value = StartupMode> {
    PossibleValuesParser::new(["startup", "keep-after-open", "while-open"]).map(|value| {
        match value.as_str() {
            "startup" => StartupMode::Startup,
            "keep-after-open" => StartupMode::KeepAfterOpen,
            "while-open" => StartupMode::WhileOpen,
            _ => unreachable!("Clap validates startup modes"),
        }  // RevExt: 112
    })  // RevExt: 114
}  // RevExt: 100
// RevExt: 76
fn method_parser() -> impl TypedValueParser<Value = Method> {
    PossibleValuesParser::new(["codex", "openrouter"]).map(|value| match value.as_str() {
        "codex" => Method::Codex,
        "openrouter" => Method::Openrouter,
        _ => unreachable!("Clap validates provider methods"),
    })  // RevExt: 115
}  // RevExt: 101
// RevExt: 77
impl ValueEnum for ServeMode {
    fn value_variants<'a>() -> &'a [Self] {
  // RevExt: 70
        &[Self::Foreground, Self::Background, Self::Boot]
    }  // RevExt: 68
// RevExt: 66
    fn to_possible_value(&self) -> Option<PossibleValue> {
  // RevExt: 71
        Some(PossibleValue::new(match self {
  // RevExt: 73
            Self::Foreground => "foreground",
            Self::Background => "background",
            Self::Boot => "boot",
        }))  // RevExt: 75
    }  // RevExt: 69
}  // RevExt: 67
