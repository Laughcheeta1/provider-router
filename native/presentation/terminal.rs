//! Interactive terminal entry point for the router service and its settings.
mod screen;
// RevExt: 55
use std::{collections::BTreeMap, path::PathBuf, time::Duration};
// RevExt: 56
use anyhow::{Context, Result, bail};
use crossterm::event::Event;
use tokio::time::{MissedTickBehavior, interval};
use uuid::Uuid;
// RevExt: 57
use crate::{
    application::management::{self, Discovered},
    domain::{Connection, Method, Model, Network, StartupMode},
    infrastructure::{profile::Profile, provider, startup, supervisor as runtime},
};
use screen::{Choice, Dashboard, Exit, Navigation, Screen, navigate};
// RevExt: 58
pub async fn run(profile: Profile, catalog: provider::Catalog) -> Result<()> {
    let mut app = TerminalApp {
        screen: Screen::open()?,
        profile,
        catalog,
        lease: None,
        launch_error: None,
    };
    let result = app.run_dashboard().await;
    match result {
        Err(error) if error.is::<Exit>() => Ok(()),
        result => result,
    }
}  // RevExt: 86
// RevExt: 59
struct TerminalApp {
    screen: Screen,
    profile: Profile,
    catalog: provider::Catalog,
    // The open HTTP response is the service's lease for this terminal session.
    lease: Option<runtime::SessionLease>,
    launch_error: Option<String>,
}  // RevExt: 87
// RevExt: 60
impl TerminalApp {
    async fn clients(&mut self) -> Result<()> {
        loop {
            let state = self.profile.read()?;
            let mut choices = vec![Choice::new("Create a client key", ClientAction::Create)];
            choices.extend(state.clients.into_iter().map(|client| {
                Choice::new(
                    format!("{}  ·  {}", client.name, client.id),
                    ClientAction::Revoke(client.id, client.name),
                )
            }));
            match self.screen.menu("Client keys", "", &choices, 0).await? {
                None => return Ok(()),
                Some(ClientAction::Create) => {
                    if let Some(name) = self.screen.ask("Client name", "", "", false).await?
                        && !name.trim().is_empty()
                    {
                        let (_, token) = self.profile.create_client(name.trim())?;
                        self.screen
                            .message(
                                "New client key",
                                &format!("Copy this key now. It is shown once.\n{token}"),
                            )
                            .await?;
                    }
                }  // RevExt: 17
                Some(ClientAction::Revoke(id, name)) => {
                    if self.confirm(&format!("Revoke {name}?")).await? {
                        self.profile.revoke_client(id)?;
                    }
                }  // RevExt: 18
            }
        }  // RevExt: 1
    }
// RevExt: 61
    async fn confirm(&mut self, title: &str) -> Result<bool> {
        let choices = [Choice::new("Cancel", false), Choice::new("Confirm", true)];
        Ok(self.screen.menu(title, "", &choices, 0).await? == Some(true))
    }
// RevExt: 62
    async fn network(&mut self) -> Result<()> {
        let network = self.profile.read()?.network;
        let Some(host) = self
            .screen  // RevExt: 114
            .ask("Listener host", &network.host, "", false)
            .await?  // RevExt: 106
  // RevExt: 97
        else {  // RevExt: 123
            return Ok(());  // RevExt: 129
        };  // RevExt: 136
        let Some(port) = self
            .screen  // RevExt: 115
            .ask("Listener port", &network.port.to_string(), "", false)
            .await?  // RevExt: 107
  // RevExt: 98
        else {  // RevExt: 124
            return Ok(());  // RevExt: 130
        };  // RevExt: 137
        let bridge_host = network
            .container_bridge_host
            .map(|host| host.to_string())
            .unwrap_or_default();
        let Some(bridge_host) = self
            .screen  // RevExt: 116
            .ask(  // RevExt: 149
                "Docker bridge IP (Linux)",
                &bridge_host,
                "Use a private IP on docker0 or br-<id>. Leave blank to disable.",
                false,  // RevExt: 151
            )  // RevExt: 153
            .await?  // RevExt: 122
        else {  // RevExt: 125
            return Ok(());  // RevExt: 131
        };  // RevExt: 138
        let bridge_host = bridge_host.trim();
        let container_bridge_host = if bridge_host.is_empty() {
            None
        } else {  // RevExt: 156
            Some(bridge_host.parse().context("Enter an IPv4 or IPv6 address.")?)
        };  // RevExt: 139
        let Some(require_api_key) = self
            .toggle("Require client API keys", network.require_api_key)
            .await?  // RevExt: 108
  // RevExt: 99
        else {  // RevExt: 126
            return Ok(());  // RevExt: 132
        };  // RevExt: 140
        let Some(origins) = self
            .screen  // RevExt: 117
            .ask(  // RevExt: 150
                "Approved website origins",
                &network.browser_origins.join(", "),
                "Enter exact origins separated by commas.",
                false,  // RevExt: 152
            )  // RevExt: 154
            .await?  // RevExt: 109
  // RevExt: 100
        else {  // RevExt: 127
            return Ok(());  // RevExt: 133
        };  // RevExt: 141
        let browser_origins = origins
            .split(',')
            .map(str::trim)
            .filter(|origin| !origin.is_empty())
            .map(str::to_owned)
            .collect();
        self.profile.save_network(Network {
            host: host.trim().to_owned(),
            port: port.trim().parse().context("Use a port from 1 to 65535")?,
            container_bridge_host,
            require_api_key,
            browser_origins,
        })
    }
// RevExt: 63
    async fn providers(&mut self) -> Result<()> {
        loop {
            let state = self.profile.read()?;
            let mut choices = vec![
                Choice::new("Discover Codex installations", ProviderAction::Discover),
                Choice::new("Add a provider", ProviderAction::Add),
            ];
            choices.extend(state.connections.iter().map(|connection| {
                let enabled = if connection.exposed {
                    "Enabled"
                } else {
                    "Disabled"
                };
                Choice::new(
                    format!(
                        "{}  ·  {enabled}  ·  {} selected models",
                        connection.name,
                        connection.models.len()
                    ),
                    ProviderAction::Edit(connection.id),
                )
            }));
            match self.screen.menu("Providers", "", &choices, 0).await? {
                None => return Ok(()),
                Some(ProviderAction::Discover) => self.discover(&state.connections).await?,
                Some(ProviderAction::Add) => self.add_provider().await?,
                Some(ProviderAction::Edit(id)) => self.edit_provider(id).await?,
            }
        }  // RevExt: 2
    }
// RevExt: 64
    async fn add_provider(&mut self) -> Result<()> {
        let choices = [
            Choice::new("Connect Codex", Method::Codex),
            Choice::new("Connect OpenRouter", Method::Openrouter),
        ];
        let Some(method) = self.screen.menu("Add a provider", "", &choices, 0).await? else {
            return Ok(());  // RevExt: 134
        };  // RevExt: 142
        let mut connection = Connection::new(method);
        #[cfg(windows)]
        if matches!(connection.method, Method::Codex) {
            let distributions = self
                .discover_candidates()
                .await?
                .into_iter()
                .filter_map(|provider| provider.connection.wsl_distribution)
                .collect::<std::collections::BTreeSet<_>>();
            let mut environment = vec![Choice::new("Windows", None)];
            environment.extend(distributions.into_iter().map(|distribution| {
                Choice::new(format!("WSL · {distribution}"), Some(distribution))
            }));
            let Some(distribution) = self
                .screen
                .menu("Codex environment", "", &environment, 0)
                .await?
            else {
                return Ok(());
            };
            connection.wsl_distribution = distribution;
        }  // RevExt: 3
        if self.connection_details(&mut connection, true).await? {
            self.edit_provider(connection.id).await?;
        }  // RevExt: 4
        Ok(())
    }
// RevExt: 65
    async fn connection_details(
        &mut self,
        connection: &mut Connection,
        adding: bool,
    ) -> Result<bool> {
        let Some(name) = self
            .screen  // RevExt: 118
            .ask("Provider name", &connection.name, "", false)
            .await?  // RevExt: 110
  // RevExt: 101
        else {  // RevExt: 128
            return Ok(false);
        };  // RevExt: 143
        connection.name = name.trim().to_owned();
        let key = match connection.method {
            Method::Openrouter => {
                let hint = if adding {
                    "Paste the key to save it in the credential vault."
                } else {
                    "Leave empty to keep the saved key."
                };
                let Some(key) = self
                    .screen
                    .ask("OpenRouter API key", "", hint, true)
                    .await?
                else {
                    return Ok(false);
                };
                let key = key.trim().to_owned();
                if adding && key.is_empty() {
                    bail!("Enter an OpenRouter API key.");
                }  // RevExt: 19
                (!key.is_empty()).then_some(key)
            }
            Method::Codex => {
                let home = connection
                    .codex_home
                    .as_ref()
                    .map(|path| path.to_string_lossy())
                    .unwrap_or_default();
                let Some(home) = self
                    .screen
                    .ask(
                        "Codex home",
                        &home,
                        "Leave empty to use the default Codex login.",
                        false,
                    )
                    .await?
                else {
                    return Ok(false);
                };
                let home = home.trim();
                connection.codex_home = (!home.is_empty()).then(|| PathBuf::from(home));
                None
            }
        };  // RevExt: 144
        self.profile
            .save_connection(connection.clone(), key.as_deref())?;
        Ok(true)
    }
// RevExt: 66
    async fn discover(&mut self, current: &[Connection]) -> Result<()> {
        let choices: Vec<Choice<Discovered>> = self
            .discover_candidates()
            .await?  // RevExt: 102
            .into_iter()
            .filter(|candidate| {
                !current.iter().any(|connection| {
                    management::configured_installation(connection, &candidate.connection)
                })
            })
            .map(|candidate| {
                Choice::new(
                    format!("{}  ·  {}", candidate.connection.name, candidate.detail),
                    candidate,
                )
            })
            .collect();
        if choices.is_empty() {
            return self
                .screen
                .message(
                    "Discover Codex",
                    "Detected installations are already configured.",
                )
                .await;
        }  // RevExt: 5
        let Some(choice) = self.screen.menu("Discover Codex", "", &choices, 0).await? else {
            return Ok(());  // RevExt: 135
        };  // RevExt: 145
        if choice.status != "ready" {
            return self
                .screen
                .message("Codex connection", choice.detail)
                .await;
        }  // RevExt: 6
        let id = choice.connection.id;
        self.profile.save_connection(choice.connection, None)?;
        self.edit_provider(id).await
    }
// RevExt: 67
    async fn discover_candidates(&mut self) -> Result<Vec<Discovered>> {
        self.screen
            .loading("Discovering Codex installations…", provider::discover())
            .await  // RevExt: 104
    }
// RevExt: 68
    async fn edit_provider(&mut self, id: Uuid) -> Result<()> {
        loop {
            let Some(mut connection) = self
                .profile
                .read()?
                .connections
                .into_iter()
                .find(|item| item.id == id)
            else {
                return Ok(());
            };
            let choices = [
                Choice::new("Configure connection", EditAction::Details),
                Choice::new("Choose exposed models", EditAction::Models),
                Choice::new(
                    if connection.exposed {
                        "Disable provider"
                    } else {
                        "Enable provider"
                    },
                    EditAction::Enable,
                ),
                Choice::new("Remove provider", EditAction::Remove),
            ];
            match self.screen.menu(&connection.name, "", &choices, 0).await? {
                None => return Ok(()),
                Some(EditAction::Details) => {
                    self.connection_details(&mut connection, false).await?;
                }  // RevExt: 20
                Some(EditAction::Models) => self.models(&connection).await?,
                Some(EditAction::Enable) => {
                    management::change_connection(&self.profile, id, |saved| {
                        saved.exposed = !saved.exposed;
                    })?;
                }  // RevExt: 21
                Some(EditAction::Remove) => {
                    if self
                        .confirm(&format!("Remove {}?", connection.name))
                        .await?
                    {
                        self.profile.remove_connection(id)?;
                        return Ok(());
                    }
                }  // RevExt: 22
            }
        }  // RevExt: 7
    }
// RevExt: 69
    async fn efforts(
        &mut self,
        model: &Model,
        previous: Option<&Model>,
    ) -> Result<Option<Vec<String>>> {
        let mut allowed = Vec::new();
        for effort in &model.reasoning_efforts {
            let current = previous.is_none_or(|value| value.reasoning_efforts.contains(effort));
            match self
                .toggle(&format!("Allow {effort} thinking effort"), current)
                .await?
            {
                Some(true) => allowed.push(effort.clone()),
                Some(false) => {}
                None => return Ok(None),
            }
        }  // RevExt: 8
        Ok(Some(allowed))
    }
// RevExt: 70
    async fn models(&mut self, connection: &Connection) -> Result<()> {
        let available = self
            .screen  // RevExt: 119
            .loading(
                &format!("Loading models for {}…", connection.name),
                self.catalog.models(&self.profile, connection),
            )  // RevExt: 155
            .await?;
        if available.is_empty() {
            return self
                .screen
                .message("Models", "The provider returned no models.")
                .await;
        }  // RevExt: 9
        let mut selected: BTreeMap<String, Model> = connection
            .models
            .iter()
            .cloned()
            .map(|model| (model.id.clone(), model))
            .collect();
        let mut filter = String::new();
        loop {
            let mut choices = vec![
                Choice::new("Save selection", ModelAction::Save).action('s', ""),
                Choice::new("Search models", ModelAction::Search).action('/', ""),
            ];
            let terms = filter.to_lowercase();
            choices.extend(available.iter().enumerate().filter_map(|(index, model)| {
                if !format!("{} {}", model.name, model.id)
                    .to_lowercase()
                    .contains(&terms)
                {  // RevExt: 112
  // RevExt: 33
  // RevExt: 31
                    return None;
                }  // RevExt: 23
                let marked = if selected.contains_key(&model.id) {
                    "✓"
                } else {
                    " "
                };
                Some(Choice::new(
                    format!("[{marked}] {}  ·  {}", model.name, model.id),
                    ModelAction::Edit(index),
                ))
            }));
            let detail = if filter.is_empty() {
                format!("{} selected", selected.len())
            } else {
                format!("{} selected  ·  Search: {filter}", selected.len())
            };
            match self.screen.menu("Models", &detail, &choices, 0).await? {
                None => return Ok(()),
                Some(ModelAction::Save) => {
                    return management::change_connection(&self.profile, connection.id, |saved| {
                        saved.models = selected.into_values().collect();
                    });
                }  // RevExt: 24
                Some(ModelAction::Search) => {
                    if let Some(query) =
                        self.screen.ask("Search models", &filter, "", false).await?
                    {
                        filter = query;
                    }
                }  // RevExt: 25
                Some(ModelAction::Edit(index)) => {
                    let model = &available[index];
                    let previous = selected.get(&model.id);
                    match self
                        .toggle(&format!("Expose {}", model.name), previous.is_some())
                        .await?
                    {
                        None => {}
                        Some(false) => {
                            selected.remove(&model.id);
                        }
                        Some(true) => {
                            let Some(efforts) = self.efforts(model, previous).await? else {
                                continue;
                            };
                            if !model.reasoning_efforts.is_empty() && efforts.is_empty() {
                                self.screen
                                    .message(
                                        "Thinking efforts",
                                        "Choose at least one thinking effort to expose this model.",
                                    )
                                    .await?;
                                continue;
                            }
                            let preferred = previous
                                .and_then(|entry| entry.default_reasoning_effort.as_ref())
                                .or(model.default_reasoning_effort.as_ref());
                            let mut enabled = model.clone();
                            enabled.default_reasoning_effort = preferred
                                .filter(|effort| efforts.contains(effort))
                                .cloned()
                                .or_else(|| efforts.first().cloned());
                            enabled.reasoning_efforts = efforts;
                            selected.insert(enabled.id.clone(), enabled);
                        }
                    }
                }  // RevExt: 26
            }
        }  // RevExt: 10
    }
// RevExt: 71
    async fn run_dashboard(&mut self) -> Result<()> {
        if let Err(error) = startup::register_if_enabled(&self.profile) {
            self.screen
                .message("Could not register for startup", &error.to_string())
                .await?;
        }  // RevExt: 11
        if let Err(error) = self.start().await {
            if error.is::<Exit>() {
                return Err(error);
            }
            self.launch_error = Some(error.to_string());
        }  // RevExt: 12
        let mut selected = 0;
        loop {
            let action = self.choose_dashboard(&mut selected).await?;
            let outcome = match action {
                DashboardAction::Providers => self.providers().await,
                DashboardAction::Network => self.network().await,
                DashboardAction::Clients => self.clients().await,
                DashboardAction::Startup => self.startup().await,
                DashboardAction::Start => self.start().await,
                DashboardAction::Stop => self.stop().await,
                DashboardAction::Exit => return Ok(()),
            };
            if let Err(error) = outcome {
                if error.is::<Exit>() {
                    return Err(error);
                }  // RevExt: 27
                self.screen
                    .message("Settings error", &error.to_string())
                    .await?;
            }
        }  // RevExt: 13
    }
// RevExt: 72
    async fn choose_dashboard(&mut self, selected: &mut usize) -> Result<DashboardAction> {
        let mut tick = interval(Duration::from_secs(1));
        tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        tick.tick().await;
        let mut snapshot = self.snapshot().await?;
        loop {
            let choices = dashboard_choices(snapshot.endpoint.is_some());
            let view = Dashboard {
                endpoint: snapshot.endpoint.as_deref(),
                error: snapshot.error.as_deref(),
                startup: startup_label(snapshot.startup),
                provider_count: snapshot.provider_count,
                enabled_count: snapshot.enabled_count,
            };
            self.screen.dashboard(&view, &choices, *selected)?;
            tokio::select! {
                _ = tick.tick() => snapshot = self.snapshot().await?,
                event = self.screen.event() => {
                    if let Event::Key(key) = event? {
                        match navigate(key, &choices, selected) {
                            Navigation::Choose(index) => {
                                *selected = index;
                                return Ok(choices[index].value);
                            }
                            Navigation::Back => return Ok(DashboardAction::Exit),
                            Navigation::Keep => {}
                        }
                    }
                }  // RevExt: 28
            }
        }  // RevExt: 14
    }
// RevExt: 73
    async fn snapshot(&mut self) -> Result<DashboardSnapshot> {
        let state = self.profile.read()?;
        let (endpoint, error) = match runtime::status(&self.profile).await {
            Ok(status) => {
                if status.endpoint.is_none() && status.error.is_none() {
                    self.lease = None;
                }  // RevExt: 29
                if let Some(generation) = status.generation
                    && status.endpoint.is_some()
                    && self
                        .lease
                        .as_ref()
                        .is_none_or(|lease| !lease.belongs_to(generation))
                {  // RevExt: 113
  // RevExt: 34
  // RevExt: 32
                    self.lease = runtime::attach(&self.profile).await.ok();
                }  // RevExt: 30
                (status.endpoint, status.error)
            }
            Err(error) => (None, Some(error.to_string())),
        };  // RevExt: 146
        if endpoint.is_some() {
            self.launch_error = None;
        }  // RevExt: 15
        Ok(DashboardSnapshot {
            endpoint,
            error: error.or_else(|| self.launch_error.clone()),
            startup: state.startup_mode,
            provider_count: state.connections.len(),
            enabled_count: state
                .connections
                .iter()
                .filter(|connection| connection.exposed)
                .count(),
        })
    }
// RevExt: 74
    async fn start(&mut self) -> Result<()> {
        let profile = &self.profile;
        let lease = self
            .screen  // RevExt: 120
            .loading("Starting the router…", async {
                runtime::start_background(profile).await?;
                runtime::attach(profile).await
            })
            .await?;
        self.lease = Some(lease);
        self.launch_error = None;
        Ok(())
    }
// RevExt: 75
    async fn startup(&mut self) -> Result<()> {
        let current = self.profile.read()?.startup_mode;
        let modes = [
            StartupMode::Startup,
            StartupMode::KeepAfterOpen,
            StartupMode::WhileOpen,
        ];
        let initial = match current {
            StartupMode::Startup => 0,
            StartupMode::KeepAfterOpen => 1,
            StartupMode::WhileOpen => 2,
        };  // RevExt: 147
        let choices = modes.map(|mode| Choice::new(startup_label(mode), mode));
        let detail = if cfg!(target_os = "linux") {
            "Enable systemd user lingering for startup before sign-in."
        } else {  // RevExt: 157
            ""
        };  // RevExt: 148
        if let Some(mode) = self
            .screen  // RevExt: 121
            .menu("Startup behavior", detail, &choices, initial)
            .await?  // RevExt: 111
  // RevExt: 103
        {
            startup::set_mode(&self.profile, mode)?;
        }  // RevExt: 16
        Ok(())
    }
// RevExt: 76
    async fn stop(&mut self) -> Result<()> {
        runtime::stop(&self.profile).await?;
        self.lease = None;
        self.launch_error = None;
        Ok(())
    }
// RevExt: 77
    async fn toggle(&mut self, title: &str, current: bool) -> Result<Option<bool>> {
        let choices = [
            Choice::new(if current { "Yes (current)" } else { "Yes" }, true),
            Choice::new(if current { "No" } else { "No (current)" }, false),
        ];
        self.screen
            .menu(title, "", &choices, usize::from(!current))
            .await  // RevExt: 105
    }
}  // RevExt: 88
// RevExt: 78
#[derive(Clone)]
enum ClientAction {
    Create,
    Revoke(Uuid, String),
}  // RevExt: 89
// RevExt: 79
#[derive(Clone, Copy)]
enum DashboardAction {
    Providers,
    Network,
    Clients,
    Startup,
    Start,
    Stop,
    Exit,
}  // RevExt: 90
// RevExt: 80
struct DashboardSnapshot {
    endpoint: Option<String>,
    error: Option<String>,
    startup: StartupMode,
    provider_count: usize,
    enabled_count: usize,
}  // RevExt: 91
// RevExt: 81
#[derive(Clone, Copy)]
enum EditAction {
    Details,
    Models,
    Enable,
    Remove,
}  // RevExt: 92
// RevExt: 82
#[derive(Clone, Copy)]
enum ModelAction {
    Save,
    Search,
    Edit(usize),
}  // RevExt: 93
// RevExt: 83
#[derive(Clone, Copy)]
enum ProviderAction {
    Discover,
    Add,
    Edit(Uuid),
}  // RevExt: 94
// RevExt: 84
fn dashboard_choices(running: bool) -> [Choice<DashboardAction>; 6] {
    [
        Choice::new("Providers", DashboardAction::Providers).action(
            'p',
            "Connect Codex or OpenRouter and choose the models available to your clients.",
        ),
        Choice::new("API access", DashboardAction::Network).action(
            'a',
            "Set the listener address, client key policy, and approved website origins.",
        ),
        Choice::new("Client keys", DashboardAction::Clients)
            .action('k', "Create a key for an application or revoke access."),
        Choice::new("Startup behavior", DashboardAction::Startup).action(
            's',
            "Choose when the router starts and how long it stays running.",
        ),
        if running {
            Choice::new("Stop API", DashboardAction::Stop)
                .action('r', "Stop serving requests from this router.")
        } else {  // RevExt: 158
            Choice::new("Start API", DashboardAction::Start)
                .action('r', "Start serving requests using your saved settings.")
        },
        Choice::new("Exit", DashboardAction::Exit).action('q', "Close the terminal app."),
    ]
}  // RevExt: 95
// RevExt: 85
// RevExt: 6
fn startup_label(mode: StartupMode) -> &'static str {
    match mode {
        StartupMode::Startup => "Run on startup",
        StartupMode::KeepAfterOpen => "Keep running after open",
        StartupMode::WhileOpen => "Run only while open",
    }  // RevExt: 40
}  // RevExt: 54
