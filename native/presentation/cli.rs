//! Scriptable settings and lifecycle commands for the native executable.
use crate::{
    application::management,
    domain::{Connection, Method, Network, StartupMode},
    infrastructure::{
        profile::Profile,
        provider, startup,
        supervisor::{self as runtime, ServeMode},
    },
};
use anyhow::{Context, Result, bail, ensure};
use clap::{
    Args, Subcommand, ValueEnum,
    builder::{PossibleValue, PossibleValuesParser, TypedValueParser},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    io::{self, IsTerminal, Read},
    path::PathBuf,
};
use uuid::Uuid;
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
    },
    /// Discover, configure, and expose inference providers.
    Providers {
        #[command(subcommand)]
        command: ProviderCommand,
    },
    /// Manage revocable credentials for API clients.
    Clients {
        #[command(subcommand)]
        command: ClientCommand,
    },
    /// Configure the API listener and approved browser origins.
    Network {
        #[command(subcommand)]
        command: NetworkCommand,
    },
    /// Apply a JSON settings command supplied on stdin.
    Settings {
        #[command(subcommand)]
        command: SettingsCommand,
    },
    #[command(name = "__serve", hide = true)]
    InternalServe { mode: ServeMode },
    #[command(name = "__worker", hide = true)]
    InternalWorker,
    #[command(name = "__handoff", hide = true)]
    InternalHandoff,
    #[command(name = "__install", hide = true)]
    InternalInstall {
        #[arg(long)]
        destination: PathBuf,
    },
    #[command(name = "__uninstall", hide = true)]
    InternalUninstall,
}
#[derive(Subcommand)]
pub enum ProviderCommand {
    List,
    Discover,
    /// Use the native Codex installation or a named WSL distribution.
    Use {
        installation: String,
    },
    Add {
        #[arg(value_parser = method_parser())]
        method: Method,
        #[arg(long)]
        wsl: Option<String>,
        #[command(flatten)]
        fields: ProviderFields,
    },
    Edit {
        id: String,
        #[command(flatten)]
        fields: ProviderFields,
    },
    Enable {
        id: String,
    },
    Disable {
        id: String,
    },
    Remove {
        id: String,
    },
    Models {
        id: String,
    },
    Select {
        id: String,
        model: String,
        #[arg(long, value_delimiter = ',')]
        efforts: Option<Vec<String>>,
    },
    Deselect {
        id: String,
        model: String,
    },
}
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
}
#[derive(Subcommand)]
pub enum ClientCommand {
    List,
    Create { name: String },
    Revoke { id: String },
}
#[derive(Subcommand)]
pub enum NetworkCommand {
    Show,
    Set {
        /// Loopback host for applications on this computer.
        #[arg(long)]
        host: Option<String>,
        #[arg(long)]
        port: Option<u16>,
        /// Private IP assigned to the Linux host's Docker bridge.
        #[arg(long, conflicts_with = "clear_container_bridge_host")]
        container_bridge_host: Option<std::net::IpAddr>,
        /// Disable access through the Docker bridge.
        #[arg(long)]
        clear_container_bridge_host: bool,
        #[arg(long, conflicts_with = "no_require_key")]
        require_key: bool,
        #[arg(long)]
        no_require_key: bool,
        #[arg(long = "origin", conflicts_with = "clear_origins")]
        origins: Vec<String>,
        #[arg(long)]
        clear_origins: bool,
    },
}
#[derive(Subcommand)]
pub enum SettingsCommand {
    Apply,
}
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Apply {
    Status,
    Snapshot,
    SaveConnection {
        connection: Connection,
        api_key: Option<String>,
    },
    RemoveConnection {
        id: Uuid,
    },
    RefreshModels {
        id: Uuid,
    },
    CreateClient {
        name: String,
    },
    RevokeClient {
        id: Uuid,
    },
    SaveNetwork {
        network: Network,
    },
    SaveStartup {
        mode: StartupMode,
    },
}
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
        }
        Command::Startup { mode } => {
            if let Some(mode) = mode {
                startup::set_mode(&profile, mode)?;
            }
            json!({"mode": profile.read()?.startup_mode})
        }
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
            } => json!(
                profile.update_network(container_bridge_host.is_some(), |network| {
                    if let Some(host) = host {
                        network.host = host;
                    }
                    if let Some(port) = port {
                        network.port = port;
                    }
                    if clear_container_bridge_host || container_bridge_host.is_some() {
                        network.container_bridge_host = container_bridge_host;
                    }
                    if require_key || no_require_key {
                        network.require_api_key = require_key;
                    }
                    if clear_origins || !origins.is_empty() {
                        network.browser_origins = origins;
                    }
                })?
            ),
        },
        Command::Settings {
            command: SettingsCommand::Apply,
        } => {
            let request = serde_json::from_str(&read_stdin()?)
                .map_err(|_| anyhow::anyhow!("Supply a valid JSON settings command on stdin."))?;
            apply(&profile, request).await?
        }
    };
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}
async fn providers(profile: &Profile, command: ProviderCommand) -> Result<Value> {
    match command {
        ProviderCommand::List => Ok(json!(profile.read()?.connections)),
        ProviderCommand::Discover => Ok(json!(provider::discover().await?)),
        ProviderCommand::Use { installation } => {
            let candidates = provider::discover().await?;
            let candidate = candidates
                .into_iter()
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
                .into_iter()
                .find(|item| management::configured_installation(item, &connection))
            {
                return Ok(json!(existing));
            }
            profile.save_connection(connection.clone(), None)?;
            Ok(json!(connection))
        }
        ProviderCommand::Add {
            method,
            wsl,
            fields,
        } => {
            let mut connection = Connection::new(method);
            connection.wsl_distribution = wsl;
            change_connection(profile, connection, fields, true)
        }
        ProviderCommand::Edit { id, fields } => change_connection(
            profile,
            management::connection(profile, v7(&id)?)?,
            fields,
            false,
        ),
        ProviderCommand::Enable { id } => expose(profile, v7(&id)?, true),
        ProviderCommand::Disable { id } => expose(profile, v7(&id)?, false),
        ProviderCommand::Remove { id } => {
            profile.remove_connection(v7(&id)?)?;
            Ok(Value::Null)
        }
        ProviderCommand::Models { id } => models(profile, v7(&id)?).await,
        ProviderCommand::Select { id, model, efforts } => {
            let id = v7(&id)?;
            let connection = management::connection(profile, id)?;
            let mut selection = provider::Catalog::new()?
                .models(profile, &connection)
                .await?
                .into_iter()
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
            management::change_connection(profile, id, |connection| {
                connection.models.retain(|model| model.id != selection.id);
                connection.models.push(selection);
                json!(connection.models)
            })
        }
        ProviderCommand::Deselect { id, model } => {
            let id = v7(&id)?;
            management::change_connection(profile, id, |connection| {
                connection.models.retain(|selected| selected.id != model);
                json!(connection.models)
            })
        }
    }
}
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
    }
    if let Some(name) = fields.name {
        connection.name = name.trim().into();
    }
    if let Some(home) = fields.home {
        connection.codex_home = (!home.as_os_str().is_empty()).then_some(home);
    }
    if let Some(executable) = fields.executable {
        connection.executable = (!executable.as_os_str().is_empty()).then_some(executable);
    }
    let key = fields.key_stdin.then(read_stdin).transpose()?;
    profile.save_connection(connection.clone(), key.as_deref().map(str::trim))?;
    Ok(json!(connection))
}
fn expose(profile: &Profile, id: Uuid, exposed: bool) -> Result<Value> {
    management::change_connection(profile, id, |connection| {
        connection.exposed = exposed;
        json!(connection)
    })
}
async fn models(profile: &Profile, id: Uuid) -> Result<Value> {
    let connection = management::connection(profile, id)?;
    Ok(json!(
        provider::Catalog::new()?
            .models(profile, &connection)
            .await?
    ))
}
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
}
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
}
fn create_client(profile: &Profile, name: &str) -> Result<Value> {
    let (id, token) = profile.create_client(name)?;
    Ok(json!({"id":id,"token":token}))
}
async fn apply(profile: &Profile, action: Apply) -> Result<Value> {
    match action {
        Apply::Status => return status(profile).await,
        Apply::Snapshot => {
            let mut value = status(profile).await?;
            value["detected_providers"] = json!(provider::discover().await?);
            value["platform"] = json!(std::env::consts::OS);
            return Ok(value);
        }
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
    }
    Ok(Value::Null)
}
fn v7(value: &str) -> Result<Uuid> {
    let id = Uuid::parse_str(value).context("Enter a UUIDv7 identifier.")?;
    ensure!(id.get_version_num() == 7, "Enter a UUIDv7 identifier.");
    Ok(id)
}
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
    }
    Ok(input)
}
fn startup_parser() -> impl TypedValueParser<Value = StartupMode> {
    PossibleValuesParser::new(["startup", "keep-after-open", "while-open"]).map(|value| match value
        .as_str()
    {
        "startup" => StartupMode::Startup,
        "keep-after-open" => StartupMode::KeepAfterOpen,
        "while-open" => StartupMode::WhileOpen,
        _ => unreachable!("Clap validates startup modes"),
    })
}
fn method_parser() -> impl TypedValueParser<Value = Method> {
    PossibleValuesParser::new(["codex", "openrouter"]).map(|value| match value.as_str() {
        "codex" => Method::Codex,
        "openrouter" => Method::Openrouter,
        _ => unreachable!("Clap validates provider methods"),
    })
}
impl ValueEnum for ServeMode {
    fn value_variants<'a>() -> &'a [Self] {
        &[Self::Foreground, Self::Background, Self::Boot]
    }
    fn to_possible_value(&self) -> Option<PossibleValue> {
        Some(PossibleValue::new(match self {
            Self::Foreground => "foreground",
            Self::Background => "background",
            Self::Boot => "boot",
        }))
    }
}
