//! Interactive terminal entry point for the router service and its settings.
mod screen;
// RevExt: 55
use std::{collections::BTreeMap, path::PathBuf, time::Duration};
// RevExt: 56
use anyhow::{Result, anyhow, bail};
use crossterm::event::Event;
use tokio::time::{MissedTickBehavior, interval};
use uuid::Uuid;
// RevExt: 57
use crate::{
    application::management::{self, Discovered},
    domain::{Connection, Method, Model, StartupMode},
    infrastructure::{profile::Profile, provider, startup, supervisor as runtime},
};  // RevExt: 277
use screen::{
    Check, Checklist, Choice, Dashboard, Entry, EntryKind, Exit, FormPick, Navigation, Pick, Screen,
    navigate,
};  // RevExt: 278
// RevExt: 58
pub async fn run(profile: Profile, catalog: provider::Catalog) -> Result<()> {
    let mut app = TerminalApp {
        screen: Screen::open()?,
        profile,
        catalog,
        lease: None,
        launch_error: None,
    };  // RevExt: 222
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
        loop {  // RevExt: 282
            let state = self.profile.read()?;
            let mut choices = vec![Choice::new("Create a client key", ClientAction::Create)];
            choices.extend(state.clients.into_iter().map(|client| {
                Choice::new(  // RevExt: 290
                    format!("{}  ·  {}", client.name, client.id),
                    ClientAction::Revoke(client.id, client.name),
                )  // RevExt: 159
            }));  // RevExt: 163
            match self.screen.menu("Client keys", "", &choices, 0).await? {
                None => return Ok(()),  // RevExt: 167
                Some(ClientAction::Create) => {
                    if let Some(name) = self.screen.ask("Client name", "", "", false).await?
                        && !name.trim().is_empty()
                    {  // RevExt: 171
                        let (_, token) = self.profile.create_client(name.trim())?;
                        self.screen
                            .message(
                                "New client key",
                                &format!("Copy this key now. It is shown once.\n{token}"),
                            )
                            .await?;
                    }  // RevExt: 227
                }  // RevExt: 17
                Some(ClientAction::Revoke(id, name)) => {
                    if self.confirm(&format!("Revoke {name}?")).await? {
                        self.profile.revoke_client(id)?;
                    }  // RevExt: 228
                }  // RevExt: 18
            }  // RevExt: 175
        }  // RevExt: 1
    }
// RevExt: 61
    async fn confirm(&mut self, title: &str) -> Result<bool> {
        let choices = [Choice::new("Cancel", false), Choice::new("Confirm", true)];
        Ok(self.screen.menu(title, "", &choices, 0).await? == Some(true))
    }
// RevExt: 62
// RevExt: 114
// RevExt: 106
// RevExt: 97
// RevExt: 123
// RevExt: 129
// RevExt: 136
// RevExt: 115
// RevExt: 107
// RevExt: 98
// RevExt: 124
// RevExt: 130
// RevExt: 137
// RevExt: 116
// RevExt: 149
// RevExt: 151
// RevExt: 153
// RevExt: 122
// RevExt: 125
// RevExt: 131
// RevExt: 138
// RevExt: 156
// RevExt: 139
// RevExt: 108
// RevExt: 99
// RevExt: 126
// RevExt: 132
// RevExt: 140
// RevExt: 117
// RevExt: 150
// RevExt: 152
// RevExt: 154
// RevExt: 109
// RevExt: 100
// RevExt: 127
// RevExt: 133
// RevExt: 141
    /// API access as one form; each change is saved on its own.
    async fn network(&mut self) -> Result<()> {
        let mut cursor = 0;  // RevExt: 310
        loop {  // RevExt: 283
            let network = self.profile.read()?.network;  // RevExt: 313
            let bridge = network
                .container_bridge_host
                .map(|host| host.to_string())
                .unwrap_or_default();
            let origins = match network.browser_origins.len() {
                0 => "None".to_owned(),
                1 => "1 approved".to_owned(),
                count => format!("{count} approved"),
            };  // RevExt: 315
            let entries = [  // RevExt: 320
                Entry {  // RevExt: 322
                    label: "Listener host".into(),
                    kind: text(network.host.clone(), ""),
                    hint: "The address the API listens on. Keep 127.0.0.1 to accept only this computer.",
                },  // RevExt: 331
                Entry {  // RevExt: 323
                    label: "Listener port".into(),
                    kind: text(network.port.to_string(), ""),
                    hint: "A port from 1 to 65535.",
                },  // RevExt: 332
                Entry {  // RevExt: 324
                    label: "Docker bridge IP".into(),
                    kind: text(bridge, "Disabled"),
                    hint: "Linux only. A private IP on docker0 or br-<id> lets containers reach the API. Leave empty to disable.",
                },  // RevExt: 333
                Entry {  // RevExt: 325
                    label: "Require client keys".into(),
                    kind: EntryKind::Toggle(network.require_api_key),
                    hint: "Clients must send a key created under Client keys.",
                },  // RevExt: 334
                Entry {  // RevExt: 326
                    label: "Website origins".into(),
                    kind: EntryKind::Open(origins),
                    hint: "Browser origins allowed to call the API.",
                },  // RevExt: 335
            ];  // RevExt: 342
            let mut changed = network.clone();  // RevExt: 344
            let applied = match self.screen.form("API access", &entries, &mut cursor).await? {
                FormPick::Back => return Ok(()),  // RevExt: 346
                FormPick::Open(_) => {  // RevExt: 349
                    self.origins().await?;
                    continue;  // RevExt: 351
                }  // RevExt: 295
                FormPick::Toggle(index) if entries[index].label == "Require client keys" => {  // RevExt: 353
                    changed.require_api_key = !changed.require_api_key;
                    Ok(())  // RevExt: 355
                }  // RevExt: 296
                FormPick::Toggle(_) => continue,  // RevExt: 401
                FormPick::Text(index, value) => {  // RevExt: 358
                    let value = value.trim();  // RevExt: 360
                    match entries[index].label.as_str() {  // RevExt: 362
                        "Listener host" => {
                            changed.host = value.to_owned();
                            Ok(())  // RevExt: 364
                        }  // RevExt: 366
                        "Listener port" => value
                            .parse()  // RevExt: 371
                            .map(|port| changed.port = port)
                            .map_err(|_| anyhow!("Use a port from 1 to 65535.")),
                        _ if value.is_empty() => {
                            changed.container_bridge_host = None;
                            Ok(())  // RevExt: 365
                        }  // RevExt: 367
                        _ => value
                            .parse()  // RevExt: 372
                            .map(|host| changed.container_bridge_host = Some(host))
                            .map_err(|_| anyhow!("Enter an IPv4 or IPv6 address.")),
                    }  // RevExt: 293
                }  // RevExt: 297
            };  // RevExt: 316
            if let Err(error) = applied.and_then(|()| self.profile.save_network(changed)) {
                self.screen.message("Could not save", &error.to_string()).await?;  // RevExt: 373
            }  // RevExt: 303
        }  // RevExt: 307
    }
// RevExt: 269
    /// Approved browser origins as an editable list; each change is saved on its own.
    async fn origins(&mut self) -> Result<()> {
        let mut cursor = 0;  // RevExt: 311
        loop {  // RevExt: 284
            let network = self.profile.read()?.network;  // RevExt: 314
            let mut entries: Vec<Entry> = network
                .browser_origins
                .iter()  // RevExt: 376
                .enumerate()
                .map(|(index, origin)| Entry {
                    label: format!("Origin {}", index + 1),
                    kind: text(origin.clone(), ""),
                    hint: "Edit the exact origin. Clear it to remove it.",
                })  // RevExt: 378
                .collect();
            entries.push(Entry {
                label: "Add origin".into(),
                kind: text(String::new(), "Type a new origin"),
                hint: "For example https://app.example.com or http://localhost:5173.",
            });
            let (index, value) = match self.screen.form("Website origins", &entries, &mut cursor).await? {
                FormPick::Back => return Ok(()),  // RevExt: 347
                FormPick::Text(index, value) => (index, value.trim().to_owned()),
                FormPick::Toggle(_) | FormPick::Open(_) => continue,
            };  // RevExt: 317
            let mut changed = network.clone();  // RevExt: 345
            match (index < changed.browser_origins.len(), value.is_empty()) {
                (true, true) => {
                    changed.browser_origins.remove(index);
                }  // RevExt: 298
                (true, false) => changed.browser_origins[index] = value,
                (false, true) => continue,
                (false, false) => changed.browser_origins.push(value),
            }  // RevExt: 304
            if let Err(error) = self.profile.save_network(changed) {
                self.screen.message("Could not save", &error.to_string()).await?;  // RevExt: 374
            }  // RevExt: 305
        }  // RevExt: 308
    }
// RevExt: 63
    async fn providers(&mut self) -> Result<()> {
        loop {  // RevExt: 285
            let state = self.profile.read()?;
            let mut choices = vec![Choice::new("Add a provider", ProviderAction::Add)];
            // RevExt: 186
            // RevExt: 188
            choices.extend(state.connections.iter().map(|connection| {
                let enabled = if connection.exposed {
                    "Enabled"
                } else {  // RevExt: 191
                    "Disabled"
                };  // RevExt: 380
                Choice::new(  // RevExt: 291
                    format!(
                        "{}  ·  {enabled}  ·  {} selected models",
                        connection.name,
                        connection.models.len()
                    ),  // RevExt: 382
                    ProviderAction::Edit(connection.id),
                )  // RevExt: 160
            }));  // RevExt: 164
            match self.screen.menu("Providers", "", &choices, 0).await? {
                None => return Ok(()),  // RevExt: 168
                Some(ProviderAction::Add) => self.add_provider(&state.connections).await?,
                Some(ProviderAction::Edit(id)) => self.edit_provider(id).await?,
            }  // RevExt: 176
        }  // RevExt: 2
    }
// RevExt: 64
// RevExt: 247
// RevExt: 249
// RevExt: 241
// RevExt: 251
// RevExt: 248
// RevExt: 250
// RevExt: 4
// RevExt: 201
// RevExt: 65
// RevExt: 205
// RevExt: 118
// RevExt: 110
// RevExt: 101
// RevExt: 128
// RevExt: 143
// RevExt: 192
// RevExt: 177
// RevExt: 178
// RevExt: 144
    async fn add_provider(&mut self, current: &[Connection]) -> Result<()> {
        let choices = [
            Choice::new("Connect Codex", Method::Codex),
            Choice::new("Connect OpenRouter", Method::Openrouter),
        ];  // RevExt: 384
        let Some(method) = self.screen.menu("Add a provider", "", &choices, 0).await? else {
            return Ok(());  // RevExt: 134
        };  // RevExt: 142
        if matches!(method, Method::Codex) {
            return self.add_codex(current).await;
        }  // RevExt: 240
        let Some(key) = self
            .screen  // RevExt: 388
            .ask("OpenRouter API key", "", "Paste the key to save it in the credential vault.", true)
            .await?  // RevExt: 389
        else {
            return Ok(());  // RevExt: 386
        };  // RevExt: 387
        if key.trim().is_empty() {
            bail!("Enter an OpenRouter API key.");
        }  // RevExt: 19
        let connection = Connection::new(Method::Openrouter);
        let id = connection.id;  // RevExt: 390
        self.profile.save_connection(connection, Some(key.trim()))?;
        self.edit_provider(id).await  // RevExt: 392
    }
// RevExt: 219
    /// Offer the detected Codex installations that are not configured yet, or a manual setup.
    async fn add_codex(&mut self, current: &[Connection]) -> Result<()> {
        let candidates = self.discover_candidates().await?;
        let mut choices: Vec<Choice<Option<Discovered>>> = candidates
            .iter()  // RevExt: 252
            .filter(|candidate| {
                !current.iter().any(|connection| {
                    management::configured_installation(connection, &candidate.connection)
                })  // RevExt: 379
            })
            .map(|candidate| {
                Choice::new(  // RevExt: 292
                    format!("{}  ·  {}", candidate.connection.name, candidate.detail),
                    Some(candidate.clone()),
                )  // RevExt: 226
            })
            .collect();  // RevExt: 394
        let detail = if choices.is_empty() {
            "No new Codex installations were detected."
        } else {  // RevExt: 246
            "Choose a detected installation or set one up manually."
        };  // RevExt: 244
        choices.push(Choice::new("Set up manually", None));
        let Some(choice) = self.screen.menu("Connect Codex", detail, &choices, 0).await? else {
            return Ok(());  // RevExt: 243
        };  // RevExt: 245
        if let Some(found) = choice {
            if found.status != "ready" {
                return self.screen.message("Codex connection", found.detail).await;
            }  // RevExt: 237
            let id = found.connection.id;
            self.profile.save_connection(found.connection, None)?;
            return self.edit_provider(id).await;
        }  // RevExt: 242
        #[cfg_attr(not(windows), allow(unused_mut))]
        let mut connection = Connection::new(Method::Codex);
        #[cfg(windows)]
        {  // RevExt: 254
            let distributions = candidates
                .into_iter()
                .filter_map(|provider| provider.connection.wsl_distribution)
                .collect::<std::collections::BTreeSet<_>>();
    // RevExt: 194
            let mut environment = vec![Choice::new("Windows", None)];
            environment.extend(distributions.into_iter().map(|distribution| {
                Choice::new(format!("WSL · {distribution}"), Some(distribution))
            }));  // RevExt: 165
            let Some(distribution) = self
                .screen  // RevExt: 197
                .menu("Codex environment", "", &environment, 0)
                .await?  // RevExt: 195
            else {
                return Ok(());  // RevExt: 257
            };  // RevExt: 260
            connection.wsl_distribution = distribution;
        }  // RevExt: 3
        let id = connection.id;  // RevExt: 391
        self.profile.save_connection(connection, None)?;
        self.edit_provider(id).await  // RevExt: 393
    }
// RevExt: 66
// RevExt: 102
// RevExt: 161
// RevExt: 207
// RevExt: 198
// RevExt: 162
// RevExt: 210
// RevExt: 5
// RevExt: 135
// RevExt: 145
// RevExt: 208
// RevExt: 199
// RevExt: 211
// RevExt: 6
// RevExt: 67
    async fn discover_candidates(&mut self) -> Result<Vec<Discovered>> {
        self.screen
            .loading("Discovering Codex installations…", provider::discover())
            .await  // RevExt: 104
    }
// RevExt: 68
// RevExt: 189
// RevExt: 169
// RevExt: 20
// RevExt: 21
// RevExt: 213
// RevExt: 172
// RevExt: 22
// RevExt: 179
    /// A provider's settings as one form; each change is saved on its own.
    async fn edit_provider(&mut self, id: Uuid) -> Result<()> {
        let mut cursor = 0;  // RevExt: 312
        loop {  // RevExt: 286
            let Some(mut connection) = self
                .profile
                .read()?
                .connections
                .into_iter()
                .find(|item| item.id == id)
            else {
                return Ok(());  // RevExt: 258
            };  // RevExt: 261
            let credential = match connection.method {
                Method::Openrouter => Entry {
                    label: "API key".into(),
                    kind: EntryKind::Text {
                        value: String::new(),
                        placeholder: "Saved in the credential vault",
                        secret: true,
                    },
                    hint: "Paste a new OpenRouter key to replace the saved one.",
                },  // RevExt: 336
                Method::Codex => Entry {
                    label: "Codex home".into(),
                    kind: text(
                        connection
                            .codex_home
                            .as_ref()
                            .map(|path| path.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                        "Default Codex login",
                    ),  // RevExt: 383
                    hint: "The Codex home folder to sign in with. Leave empty to use the default Codex login.",
                },  // RevExt: 337
            };  // RevExt: 318
            let entries = [  // RevExt: 321
                Entry {  // RevExt: 327
                    label: "Name".into(),
                    kind: text(connection.name.clone(), ""),
                    hint: "How this provider appears in lists.",
                },  // RevExt: 338
                credential,
                Entry {  // RevExt: 328
                    label: "Enabled".into(),
                    kind: EntryKind::Toggle(connection.exposed),
                    hint: "Offer this provider's exposed models to clients.",
                },  // RevExt: 339
                Entry {  // RevExt: 329
                    label: "Models".into(),
                    kind: EntryKind::Open(format!("{} exposed", connection.models.len())),
                    hint: "Choose the models and thinking efforts clients may use.",
                },  // RevExt: 340
                Entry {  // RevExt: 330
                    label: "Remove provider".into(),
                    kind: EntryKind::Action,
                    hint: "Delete this provider and its saved credential.",
                },  // RevExt: 341
            ];  // RevExt: 343
            let title = connection.name.clone();
            let saved = match self.screen.form(&title, &entries, &mut cursor).await? {
                FormPick::Back => return Ok(()),  // RevExt: 348
                FormPick::Toggle(index) if entries[index].label == "Enabled" => {  // RevExt: 354
                    connection.exposed = !connection.exposed;
                    self.profile.save_connection(connection, None)
                }  // RevExt: 299
                FormPick::Toggle(_) => continue,  // RevExt: 402
                FormPick::Open(index) if entries[index].label == "Models" => {
                    self.models(&connection).await?;
                    Ok(())  // RevExt: 356
                }  // RevExt: 300
                FormPick::Open(_) => {  // RevExt: 350
                    if self.confirm(&format!("Remove {title}?")).await? {
                        self.profile.remove_connection(id)?;
                        return Ok(());
                    }  // RevExt: 229
                    Ok(())  // RevExt: 357
                }  // RevExt: 301
                FormPick::Text(index, value) => {  // RevExt: 359
                    let value = value.trim();  // RevExt: 361
                    match entries[index].label.as_str() {  // RevExt: 363
                        "Name" => {
                            connection.name = value.to_owned();
                            self.profile.save_connection(connection, None)  // RevExt: 396
                        }  // RevExt: 368
                        "API key" if value.is_empty() => Ok(()),
                        "API key" => self.profile.save_connection(connection, Some(value)),
                        _ => {
                            connection.codex_home = (!value.is_empty()).then(|| PathBuf::from(value));
                            self.profile.save_connection(connection, None)  // RevExt: 397
                        }  // RevExt: 369
                    }  // RevExt: 294
                }  // RevExt: 302
            };  // RevExt: 319
            if let Err(error) = saved {
                self.screen.message("Could not save", &error.to_string()).await?;  // RevExt: 375
            }  // RevExt: 306
        }  // RevExt: 7
    }
// RevExt: 69
// RevExt: 206
// RevExt: 196
// RevExt: 180
// RevExt: 8
// RevExt: 70
// RevExt: 209
// RevExt: 200
// RevExt: 212
// RevExt: 187
// RevExt: 190
// RevExt: 112
// RevExt: 33
// RevExt: 31
// RevExt: 23
// RevExt: 193
// RevExt: 166
// RevExt: 170
// RevExt: 24
// RevExt: 173
// RevExt: 25
// RevExt: 214
// RevExt: 174
// RevExt: 215
// RevExt: 216
// RevExt: 217
// RevExt: 26
// RevExt: 181
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
                .screen  // RevExt: 256
                .message("Models", "The provider returned no models.")
                .await;
        }  // RevExt: 9
        let mut selected: BTreeMap<String, Model> = connection
            .models
            .iter()  // RevExt: 253
            .cloned()
            .map(|model| (model.id.clone(), model))
            .collect();  // RevExt: 395
        let title = format!("Models  ·  {}", connection.name);
        let (mut cursor, mut filter) = (0, String::new());
        loop {  // RevExt: 287
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
                    continue;  // RevExt: 352
                };  // RevExt: 381
                for effort in &model.reasoning_efforts {
                    let default = exposed.default_reasoning_effort.as_ref() == Some(effort);
                    rows.push(ModelRow::Effort(index, effort));
                    items.push(Check {
                        label: effort.clone(),
                        note: if default { "default".to_owned() } else { String::new() },
                        checked: exposed.reasoning_efforts.contains(effort),
                        depth: 1,
                    });
                }  // RevExt: 234
            }  // RevExt: 238
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
            };  // RevExt: 262
            let Pick::Toggle(row) = self.screen.checklist(&list, &mut cursor, &mut filter).await? else {
                return Ok(());  // RevExt: 259
            };  // RevExt: 263
            match rows[row] {
                ModelRow::Model(index) => {
                    let model = &available[index];  // RevExt: 266
                    if selected.remove(&model.id).is_none() {
                        selected.insert(model.id.clone(), model.clone());
                    }  // RevExt: 230
                }  // RevExt: 235
                ModelRow::Effort(index, effort) => {
                    let model = &available[index];  // RevExt: 267
                    let previous = selected.remove(&model.id);
                    // RevExt: 268
                    if let Some(exposed) = toggle_effort(model, previous.as_ref(), effort) {
                        selected.insert(exposed.id.clone(), exposed);
                    }  // RevExt: 231
                }  // RevExt: 236
            }  // RevExt: 239
            management::change_connection(&self.profile, connection.id, |saved| {
                saved.models = selected.values().cloned().collect();
            })?;
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
            }  // RevExt: 182
            self.launch_error = Some(error.to_string());
        }  // RevExt: 12
        let mut selected = 0;
        loop {  // RevExt: 288
            let action = self.choose_dashboard(&mut selected).await?;
            let outcome = match action {
                DashboardAction::Providers => self.providers().await,
                DashboardAction::Network => self.network().await,
                DashboardAction::Clients => self.clients().await,
                DashboardAction::Startup => self.startup().await,
                DashboardAction::Start => self.start().await,
                DashboardAction::Stop => self.stop().await,
                DashboardAction::Exit => return Ok(()),
            };  // RevExt: 264
            if let Err(error) = outcome {
                if error.is::<Exit>() {
                    return Err(error);
                }  // RevExt: 27
                self.screen
                    .message("Settings error", &error.to_string())
                    .await?;
            }  // RevExt: 183
        }  // RevExt: 13
    }
// RevExt: 72
    async fn choose_dashboard(&mut self, selected: &mut usize) -> Result<DashboardAction> {
        let mut tick = interval(Duration::from_secs(1));
        tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        tick.tick().await;
        let mut snapshot = self.snapshot().await?;
        loop {  // RevExt: 289
            let choices = dashboard_choices(snapshot.endpoint.is_some());
            let view = Dashboard {
                endpoint: snapshot.endpoint.as_deref(),
                error: snapshot.error.as_deref(),
                startup: startup_label(snapshot.startup),
                provider_count: snapshot.provider_count,
                enabled_count: snapshot.enabled_count,
            };  // RevExt: 265
            self.screen.dashboard(&view, &choices, *selected)?;
            tokio::select! {
                _ = tick.tick() => snapshot = self.snapshot().await?,
                event = self.screen.event() => {
                    let event = event?;
                    if let Some(index) = self.screen.clicked(&event) {
                        *selected = index;
                        return Ok(choices[index].value);
                    }  // RevExt: 232
                    if let Event::Key(key) = event {
                        match navigate(key, &choices, selected) {
                            Navigation::Choose(index) => {
                                *selected = index;
                                return Ok(choices[index].value);
                            }  // RevExt: 218
                            Navigation::Back => return Ok(DashboardAction::Exit),
                            Navigation::Keep => {}
                        }  // RevExt: 370
                    }  // RevExt: 233
                }  // RevExt: 28
            }  // RevExt: 184
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
            }  // RevExt: 185
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
                .iter()  // RevExt: 377
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
        Ok(())  // RevExt: 202
    }
// RevExt: 75
    async fn startup(&mut self) -> Result<()> {
        let current = self.profile.read()?.startup_mode;
        let modes = [
            StartupMode::Startup,
            StartupMode::KeepAfterOpen,
            StartupMode::WhileOpen,
        ];  // RevExt: 385
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
        {  // RevExt: 255
            startup::set_mode(&self.profile, mode)?;
        }  // RevExt: 16
        Ok(())  // RevExt: 203
    }
// RevExt: 76
    async fn stop(&mut self) -> Result<()> {
        runtime::stop(&self.profile).await?;
        self.lease = None;
        self.launch_error = None;
        Ok(())  // RevExt: 204
    }
// RevExt: 77
// RevExt: 105
}  // RevExt: 88
// RevExt: 78
#[derive(Clone)]
enum ClientAction {
    Create,
    Revoke(Uuid, String),
}  // RevExt: 89
// RevExt: 79
#[derive(Clone, Copy)]  // RevExt: 398
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
// RevExt: 92
// RevExt: 82
/// A text entry showing `placeholder` while empty.
fn text(value: String, placeholder: &'static str) -> EntryKind {
    EntryKind::Text {
        value,
        placeholder,
        secret: false,
    }
}  // RevExt: 279
// RevExt: 270
/// What a row of the model checklist stands for.
#[derive(Clone, Copy)]  // RevExt: 399
enum ModelRow<'a> {
    Model(usize),
    Effort(usize, &'a String),
}  // RevExt: 93
// RevExt: 220
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
}  // RevExt: 224
// RevExt: 221
/// Flip one thinking effort of a model. Clearing its last effort stops exposing the model.
fn toggle_effort(model: &Model, previous: Option<&Model>, effort: &str) -> Option<Model> {
    let exposed = |value: &String| previous.is_some_and(|entry| entry.reasoning_efforts.contains(value));
    let efforts: Vec<String> = model
        .reasoning_efforts
        .iter()
        .filter(|value| exposed(value) != (value.as_str() == effort))
        .cloned()
        .collect();
    (!efforts.is_empty()).then(|| expose(model, previous, efforts))
}  // RevExt: 280
// RevExt: 271
/// The model name, followed by its ID only when the name does not already spell it.
fn model_label(model: &Model) -> String {
    let normalize = |value: &str| -> String {
        value
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .map(|ch| ch.to_ascii_lowercase())
            .collect()
    };  // RevExt: 223
    if model.name.is_empty() {
        model.id.clone()
    } else if normalize(&model.name) == normalize(&model.id) {
        model.name.clone()
    } else {
        format!("{}  ·  {}", model.name, model.id)
    }
}  // RevExt: 225
// RevExt: 83
#[derive(Clone, Copy)]  // RevExt: 400
enum ProviderAction {
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
// RevExt: 272
#[cfg(test)]
mod tests {
    use super::*;
// RevExt: 273
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
        }  // RevExt: 309
    }
// RevExt: 274
    #[test]
    fn model_label_shows_the_id_only_when_the_name_differs() {
        assert_eq!(model_label(&model("GPT-6.1 Terra", "gpt-6.1-terra", &[], None)), "GPT-6.1 Terra");
        assert_eq!(model_label(&model("GPT-5.6-Sol", "gpt-5.6-sol", &[], None)), "GPT-5.6-Sol");
        assert_eq!(model_label(&model("Claude", "anthropic/claude", &[], None)), "Claude  ·  anthropic/claude");
        assert_eq!(model_label(&model("", "gpt-6-sol", &[], None)), "gpt-6-sol");
    }
// RevExt: 275
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
// RevExt: 276
    #[test]
    fn clearing_the_last_effort_stops_exposing_the_model() {
        let catalog = model("GPT", "gpt", &["low", "high"], Some("low"));
        let single = toggle_effort(&catalog, Some(&catalog), "high").expect("still exposed");
        assert!(toggle_effort(&catalog, Some(&single), "low").is_none());
    }
}  // RevExt: 281
