//! Interactive terminal entry point for the router service and its settings.
mod screen;
use crate::{
    application::management::{self, Discovered},
    domain::{Connection, Method, Model, StartupMode},
    infrastructure::{profile::Profile, provider, startup, supervisor as runtime},
};
use anyhow::{Result, anyhow, bail};
use crossterm::event::Event;
use screen::{
    Check, Checklist, Choice, Dashboard, Entry, EntryKind, Exit, FormPick, Navigation, Pick,
    Screen, navigate,
};
use std::{collections::BTreeMap, path::PathBuf, time::Duration};
use tokio::time::{MissedTickBehavior, interval};
use uuid::Uuid;
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
}
struct TerminalApp {
    screen: Screen,
    profile: Profile,
    catalog: provider::Catalog,
    // The open HTTP response is the service's lease for this terminal session.
    lease: Option<runtime::SessionLease>,
    launch_error: Option<String>,
}
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
                }
                Some(ClientAction::Revoke(id, name)) => {
                    if self.confirm(&format!("Revoke {name}?")).await? {
                        self.profile.revoke_client(id)?;
                    }
                }
            }
        }
    }
    async fn confirm(&mut self, title: &str) -> Result<bool> {
        let choices = [Choice::new("Cancel", false), Choice::new("Confirm", true)];
        Ok(self.screen.menu(title, "", &choices, 0).await? == Some(true))
    }
    /// API access as one form; each change is saved on its own.
    async fn network(&mut self) -> Result<()> {
        let mut cursor = 0;
        loop {
            let network = self.profile.read()?.network;
            let bridge = network
                .container_bridge_host
                .map(|host| host.to_string())
                .unwrap_or_default();
            let origins = match network.browser_origins.len() {
                0 => "None".to_owned(),
                1 => "1 approved".to_owned(),
                count => format!("{count} approved"),
            };
            let entries = [
                Entry {
                    label: "Listener host".into(),
                    kind: text(network.host.clone(), ""),
                    hint: "The address the API listens on. Keep 127.0.0.1 to accept only this computer.",
                },
                Entry {
                    label: "Listener port".into(),
                    kind: text(network.port.to_string(), ""),
                    hint: "A port from 1 to 65535.",
                },
                Entry {
                    label: "Docker bridge IP".into(),
                    kind: text(bridge, "Disabled"),
                    hint: "Linux only. A private IP on docker0 or br-<id> lets containers reach the API. Leave empty to disable.",
                },
                Entry {
                    label: "Require client keys".into(),
                    kind: EntryKind::Toggle(network.require_api_key),
                    hint: "Clients must send a key created under Client keys.",
                },
                Entry {
                    label: "Website origins".into(),
                    kind: EntryKind::Open(origins),
                    hint: "Browser origins allowed to call the API.",
                },
            ];
            let mut changed = network.clone();
            let applied = match self
                .screen
                .form("API access", &entries, &mut cursor)
                .await?
            {
                FormPick::Back => return Ok(()),
                FormPick::Open(_) => {
                    self.origins().await?;
                    continue;
                }
                FormPick::Toggle(index) if entries[index].label == "Require client keys" => {
                    changed.require_api_key = !changed.require_api_key;
                    Ok(())
                }
                FormPick::Toggle(_) => continue,
                FormPick::Text(index, value) => {
                    let value = value.trim();
                    match entries[index].label.as_str() {
                        "Listener host" => {
                            changed.host = value.to_owned();
                            Ok(())
                        }
                        "Listener port" => value
                            .parse()
                            .map(|port| changed.port = port)
                            .map_err(|_| anyhow!("Use a port from 1 to 65535.")),
                        _ if value.is_empty() => {
                            changed.container_bridge_host = None;
                            Ok(())
                        }
                        _ => value
                            .parse()
                            .map(|host| changed.container_bridge_host = Some(host))
                            .map_err(|_| anyhow!("Enter an IPv4 or IPv6 address.")),
                    }
                }
            };
            if let Err(error) = applied.and_then(|()| self.profile.save_network(changed)) {
                self.screen
                    .message("Could not save", &error.to_string())
                    .await?;
            }
        }
    }
    /// Approved browser origins as an editable list; each change is saved on its own.
    async fn origins(&mut self) -> Result<()> {
        let mut cursor = 0;
        loop {
            let network = self.profile.read()?.network;
            let mut entries: Vec<Entry> = network
                .browser_origins
                .iter()
                .enumerate()
                .map(|(index, origin)| Entry {
                    label: format!("Origin {}", index + 1),
                    kind: text(origin.clone(), ""),
                    hint: "Edit the exact origin. Clear it to remove it.",
                })
                .collect();
            entries.push(Entry {
                label: "Add origin".into(),
                kind: text(String::new(), "Type a new origin"),
                hint: "For example https://app.example.com or http://localhost:5173.",
            });
            let (index, value) = match self
                .screen
                .form("Website origins", &entries, &mut cursor)
                .await?
            {
                FormPick::Back => return Ok(()),
                FormPick::Text(index, value) => (index, value.trim().to_owned()),
                FormPick::Toggle(_) | FormPick::Open(_) => continue,
            };
            let mut changed = network.clone();
            match (index < changed.browser_origins.len(), value.is_empty()) {
                (true, true) => {
                    changed.browser_origins.remove(index);
                }
                (true, false) => changed.browser_origins[index] = value,
                (false, true) => continue,
                (false, false) => changed.browser_origins.push(value),
            }
            if let Err(error) = self.profile.save_network(changed) {
                self.screen
                    .message("Could not save", &error.to_string())
                    .await?;
            }
        }
    }
    async fn providers(&mut self) -> Result<()> {
        loop {
            let state = self.profile.read()?;
            let mut choices = vec![Choice::new("Add a provider", ProviderAction::Add)];
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
                Some(ProviderAction::Add) => self.add_provider(&state.connections).await?,
                Some(ProviderAction::Edit(id)) => self.edit_provider(id).await?,
            }
        }
    }
    async fn add_provider(&mut self, current: &[Connection]) -> Result<()> {
        let choices = [
            Choice::new("Connect Codex", Method::Codex),
            Choice::new("Connect OpenRouter", Method::Openrouter),
        ];
        let Some(method) = self.screen.menu("Add a provider", "", &choices, 0).await? else {
            return Ok(());
        };
        if matches!(method, Method::Codex) {
            return self.add_codex(current).await;
        }
        let Some(key) = self
            .screen
            .ask(
                "OpenRouter API key",
                "",
                "Paste the key to save it in the credential vault.",
                true,
            )
            .await?
        else {
            return Ok(());
        };
        if key.trim().is_empty() {
            bail!("Enter an OpenRouter API key.");
        }
        let connection = Connection::new(Method::Openrouter);
        let id = connection.id;
        self.profile.save_connection(connection, Some(key.trim()))?;
        self.edit_provider(id).await
    }
    /// Offer the detected Codex installations that are not configured yet, or a manual setup.
    async fn add_codex(&mut self, current: &[Connection]) -> Result<()> {
        let candidates = self.discover_candidates().await?;
        let mut choices: Vec<Choice<Option<Discovered>>> = candidates
            .iter()
            .filter(|candidate| {
                !current.iter().any(|connection| {
                    management::configured_installation(connection, &candidate.connection)
                })
            })
            .map(|candidate| {
                Choice::new(
                    format!("{}  ·  {}", candidate.connection.name, candidate.detail),
                    Some(candidate.clone()),
                )
            })
            .collect();
        let detail = if choices.is_empty() {
            "No new Codex installations were detected."
        } else {
            "Choose a detected installation or set one up manually."
        };
        choices.push(Choice::new("Set up manually", None));
        let Some(choice) = self
            .screen
            .menu("Connect Codex", detail, &choices, 0)
            .await?
        else {
            return Ok(());
        };
        if let Some(found) = choice {
            if found.status != "ready" {
                return self.screen.message("Codex connection", found.detail).await;
            }
            let id = found.connection.id;
            self.profile.save_connection(found.connection, None)?;
            return self.edit_provider(id).await;
        }
        #[cfg_attr(not(windows), allow(unused_mut))]
        let mut connection = Connection::new(Method::Codex);
        #[cfg(windows)]
        {
            let distributions = candidates
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
        }
        let id = connection.id;
        self.profile.save_connection(connection, None)?;
        self.edit_provider(id).await
    }
    async fn discover_candidates(&mut self) -> Result<Vec<Discovered>> {
        self.screen
            .loading("Discovering Codex installations…", provider::discover())
            .await
    }
    /// A provider's settings as one form; each change is saved on its own.
    async fn edit_provider(&mut self, id: Uuid) -> Result<()> {
        let mut cursor = 0;
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
            let credential = match connection.method {
                Method::Openrouter => Entry {
                    label: "API key".into(),
                    kind: EntryKind::Text {
                        value: String::new(),
                        placeholder: "Saved in the credential vault",
                        secret: true,
                    },
                    hint: "Paste a new OpenRouter key to replace the saved one.",
                },
                Method::Codex => Entry {
                    label: "Codex home".into(),
                    kind: text(
                        connection
                            .codex_home
                            .as_ref()
                            .map(|path| path.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                        "Default Codex login",
                    ),
                    hint: "The Codex home folder to sign in with. Leave empty to use the default Codex login.",
                },
            };
            let entries = [
                Entry {
                    label: "Name".into(),
                    kind: text(connection.name.clone(), ""),
                    hint: "How this provider appears in lists.",
                },
                credential,
                Entry {
                    label: "Enabled".into(),
                    kind: EntryKind::Toggle(connection.exposed),
                    hint: "Offer this provider's exposed models to clients.",
                },
                Entry {
                    label: "Models".into(),
                    kind: EntryKind::Open(format!("{} exposed", connection.models.len())),
                    hint: "Choose the models and thinking efforts clients may use.",
                },
                Entry {
                    label: "Remove provider".into(),
                    kind: EntryKind::Action,
                    hint: "Delete this provider and its saved credential.",
                },
            ];
            let title = connection.name.clone();
            let saved = match self.screen.form(&title, &entries, &mut cursor).await? {
                FormPick::Back => return Ok(()),
                FormPick::Toggle(index) if entries[index].label == "Enabled" => {
                    connection.exposed = !connection.exposed;
                    self.profile.save_connection(connection, None)
                }
                FormPick::Toggle(_) => continue,
                FormPick::Open(index) if entries[index].label == "Models" => {
                    self.models(&connection).await?;
                    Ok(())
                }
                FormPick::Open(_) => {
                    if self.confirm(&format!("Remove {title}?")).await? {
                        self.profile.remove_connection(id)?;
                        return Ok(());
                    }
                    Ok(())
                }
                FormPick::Text(index, value) => {
                    let value = value.trim();
                    match entries[index].label.as_str() {
                        "Name" => {
                            connection.name = value.to_owned();
                            self.profile.save_connection(connection, None)
                        }
                        "API key" if value.is_empty() => Ok(()),
                        "API key" => self.profile.save_connection(connection, Some(value)),
                        _ => {
                            connection.codex_home =
                                (!value.is_empty()).then(|| PathBuf::from(value));
                            self.profile.save_connection(connection, None)
                        }
                    }
                }
            };
            if let Err(error) = saved {
                self.screen
                    .message("Could not save", &error.to_string())
                    .await?;
            }
        }
    }
    async fn models(&mut self, connection: &Connection) -> Result<()> {
        let available = self
            .screen
            .loading(
                &format!("Loading models for {}…", connection.name),
                self.catalog.models(&self.profile, connection),
            )
            .await?;
        if available.is_empty() {
            return self
                .screen
                .message("Models", "The provider returned no models.")
                .await;
        }
        let mut selected: BTreeMap<String, Model> = connection
            .models
            .iter()
            .cloned()
            .map(|model| (model.id.clone(), model))
            .collect();
        let title = format!("Models  ·  {}", connection.name);
        let (mut cursor, mut filter) = (0, String::new());
        loop {
            // Each exposed model lists its catalog efforts beneath it.
            let mut rows = Vec::new();
            let mut items = Vec::new();
            for (index, model) in available.iter().enumerate() {
                let exposed = selected.get(&model.id);
                rows.push(ModelRow::Model(index));
                items.push(Check {
                    label: model_label(model),
                    note: String::new(),
                    checked: exposed.is_some(),
                    depth: 0,
                });
                let Some(exposed) = exposed else {
                    continue;
                };
                for effort in &model.reasoning_efforts {
                    let default = exposed.default_reasoning_effort.as_ref() == Some(effort);
                    rows.push(ModelRow::Effort(index, effort));
                    items.push(Check {
                        label: effort.clone(),
                        note: if default {
                            "default".to_owned()
                        } else {
                            String::new()
                        },
                        checked: exposed.reasoning_efforts.contains(effort),
                        depth: 1,
                    });
                }
            }
            let detail = format!(
                "{} of {} exposed. Changes save as you toggle. Thinking efforts appear beneath each chosen model.",
                selected.len(),
                available.len()
            );
            let list = Checklist {
                title: &title,
                detail: &detail,
                items: &items,
                searchable: true,
            };
            let Pick::Toggle(row) = self
                .screen
                .checklist(&list, &mut cursor, &mut filter)
                .await?
            else {
                return Ok(());
            };
            match rows[row] {
                ModelRow::Model(index) => {
                    let model = &available[index];
                    if selected.remove(&model.id).is_none() {
                        selected.insert(model.id.clone(), model.clone());
                    }
                }
                ModelRow::Effort(index, effort) => {
                    let model = &available[index];
                    let previous = selected.remove(&model.id);
                    if let Some(exposed) = toggle_effort(model, previous.as_ref(), effort) {
                        selected.insert(exposed.id.clone(), exposed);
                    }
                }
            }
            management::change_connection(&self.profile, connection.id, |saved| {
                saved.models = selected.values().cloned().collect();
            })?;
        }
    }
    async fn run_dashboard(&mut self) -> Result<()> {
        if let Err(error) = startup::register_if_enabled(&self.profile) {
            self.screen
                .message("Could not register for startup", &error.to_string())
                .await?;
        }
        if let Err(error) = self.start().await {
            if error.is::<Exit>() {
                return Err(error);
            }
            self.launch_error = Some(error.to_string());
        }
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
                }
                self.screen
                    .message("Settings error", &error.to_string())
                    .await?;
            }
        }
    }
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
                    let event = event?;
                    if let Some(index) = self.screen.clicked(&event) {
                        *selected = index;
                        return Ok(choices[index].value);
                    }
                    if let Event::Key(key) = event {
                        match navigate(key, &choices, selected) {
                            Navigation::Choose(index) => {
                                *selected = index;
                                return Ok(choices[index].value);
                            }
                            Navigation::Back => return Ok(DashboardAction::Exit),
                            Navigation::Keep => {}
                        }
                    }
                }
            }
        }
    }
    async fn snapshot(&mut self) -> Result<DashboardSnapshot> {
        let state = self.profile.read()?;
        let (endpoint, error) = match runtime::status(&self.profile).await {
            Ok(status) => {
                if status.endpoint.is_none() && status.error.is_none() {
                    self.lease = None;
                }
                if let Some(generation) = status.generation
                    && status.endpoint.is_some()
                    && self
                        .lease
                        .as_ref()
                        .is_none_or(|lease| !lease.belongs_to(generation))
                {
                    self.lease = runtime::attach(&self.profile).await.ok();
                }
                (status.endpoint, status.error)
            }
            Err(error) => (None, Some(error.to_string())),
        };
        if endpoint.is_some() {
            self.launch_error = None;
        }
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
    async fn start(&mut self) -> Result<()> {
        let profile = &self.profile;
        let lease = self
            .screen
            .loading("Starting the router…", async {
                runtime::start_background(profile).await?;
                runtime::attach(profile).await
            })
            .await?;
        self.lease = Some(lease);
        self.launch_error = None;
        Ok(())
    }
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
        };
        let choices = modes.map(|mode| Choice::new(startup_label(mode), mode));
        let detail = if cfg!(target_os = "linux") {
            "Enable systemd user lingering for startup before sign-in."
        } else {
            ""
        };
        if let Some(mode) = self
            .screen
            .menu("Startup behavior", detail, &choices, initial)
            .await?
        {
            startup::set_mode(&self.profile, mode)?;
        }
        Ok(())
    }
    async fn stop(&mut self) -> Result<()> {
        runtime::stop(&self.profile).await?;
        self.lease = None;
        self.launch_error = None;
        Ok(())
    }
}
#[derive(Clone)]
enum ClientAction {
    Create,
    Revoke(Uuid, String),
}
#[derive(Clone, Copy)]
enum DashboardAction {
    Providers,
    Network,
    Clients,
    Startup,
    Start,
    Stop,
    Exit,
}
struct DashboardSnapshot {
    endpoint: Option<String>,
    error: Option<String>,
    startup: StartupMode,
    provider_count: usize,
    enabled_count: usize,
}
/// A text entry showing `placeholder` while empty.
fn text(value: String, placeholder: &'static str) -> EntryKind {
    EntryKind::Text {
        value,
        placeholder,
        secret: false,
    }
}
/// What a row of the model checklist stands for.
#[derive(Clone, Copy)]
enum ModelRow<'a> {
    Model(usize),
    Effort(usize, &'a String),
}
/// The catalog model limited to the chosen efforts, keeping the preferred default when allowed.
fn expose(model: &Model, previous: Option<&Model>, efforts: Vec<String>) -> Model {
    let preferred = previous
        .and_then(|entry| entry.default_reasoning_effort.as_ref())
        .or(model.default_reasoning_effort.as_ref());
    let mut exposed = model.clone();
    exposed.default_reasoning_effort = preferred
        .filter(|effort| efforts.contains(effort))
        .cloned()
        .or_else(|| efforts.first().cloned());
    exposed.reasoning_efforts = efforts;
    exposed
}
/// Flip one thinking effort of a model. Clearing its last effort stops exposing the model.
fn toggle_effort(model: &Model, previous: Option<&Model>, effort: &str) -> Option<Model> {
    let exposed =
        |value: &String| previous.is_some_and(|entry| entry.reasoning_efforts.contains(value));
    let efforts: Vec<String> = model
        .reasoning_efforts
        .iter()
        .filter(|value| exposed(value) != (value.as_str() == effort))
        .cloned()
        .collect();
    (!efforts.is_empty()).then(|| expose(model, previous, efforts))
}
/// The model name, followed by its ID only when the name does not already spell it.
fn model_label(model: &Model) -> String {
    let normalize = |value: &str| -> String {
        value
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .map(|ch| ch.to_ascii_lowercase())
            .collect()
    };
    if model.name.is_empty() {
        model.id.clone()
    } else if normalize(&model.name) == normalize(&model.id) {
        model.name.clone()
    } else {
        format!("{}  ·  {}", model.name, model.id)
    }
}
#[derive(Clone, Copy)]
enum ProviderAction {
    Add,
    Edit(Uuid),
}
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
        } else {
            Choice::new("Start API", DashboardAction::Start)
                .action('r', "Start serving requests using your saved settings.")
        },
        Choice::new("Exit", DashboardAction::Exit).action('q', "Close the terminal app."),
    ]
}
fn startup_label(mode: StartupMode) -> &'static str {
    match mode {
        StartupMode::Startup => "Run on startup",
        StartupMode::KeepAfterOpen => "Keep running after open",
        StartupMode::WhileOpen => "Run only while open",
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn model(name: &str, id: &str, efforts: &[&str], default: Option<&str>) -> Model {
        Model {
            id: id.to_owned(),
            name: name.to_owned(),
            reasoning_efforts: efforts.iter().map(|effort| (*effort).to_owned()).collect(),
            default_reasoning_effort: default.map(str::to_owned),
            context_window: 0,
            max_output_tokens: 0,
            tools: false,
            structured_output: false,
            input_modalities: Vec::new(),
        }
    }
    #[test]
    fn model_label_shows_the_id_only_when_the_name_differs() {
        assert_eq!(
            model_label(&model("GPT-6.1 Terra", "gpt-6.1-terra", &[], None)),
            "GPT-6.1 Terra"
        );
        assert_eq!(
            model_label(&model("GPT-5.6-Sol", "gpt-5.6-sol", &[], None)),
            "GPT-5.6-Sol"
        );
        assert_eq!(
            model_label(&model("Claude", "anthropic/claude", &[], None)),
            "Claude  ·  anthropic/claude"
        );
        assert_eq!(model_label(&model("", "gpt-6-sol", &[], None)), "gpt-6-sol");
    }
    #[test]
    fn toggling_efforts_keeps_catalog_order_and_a_valid_default() {
        let catalog = model("GPT", "gpt", &["low", "medium", "high"], Some("medium"));
        let narrowed = toggle_effort(&catalog, Some(&catalog), "medium").expect("still exposed");
        assert_eq!(narrowed.reasoning_efforts, ["low", "high"]);
        assert_eq!(narrowed.default_reasoning_effort.as_deref(), Some("low"));
        let restored = toggle_effort(&catalog, Some(&narrowed), "medium").expect("still exposed");
        assert_eq!(restored.reasoning_efforts, ["low", "medium", "high"]);
        assert_eq!(restored.default_reasoning_effort.as_deref(), Some("low"));
    }
    #[test]
    fn clearing_the_last_effort_stops_exposing_the_model() {
        let catalog = model("GPT", "gpt", &["low", "high"], Some("low"));
        let single = toggle_effort(&catalog, Some(&catalog), "high").expect("still exposed");
        assert!(toggle_effort(&catalog, Some(&single), "low").is_none());
    }
}
