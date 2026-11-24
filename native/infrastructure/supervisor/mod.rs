//! Own the local API process and authenticated control channel.
// RevExt: 212
mod control;
mod handoff;
mod socket_transfer;
mod updates;
mod worker;
// RevExt: 213
pub use control::{SessionLease, attach, status, stop};
pub use handoff::ServeMode;
pub use updates::{forward_cli, installed_release_version};
pub use worker::serve as serve_worker;
// RevExt: 1
use std::{
    fs::{self, File},
    io,
    net::{Ipv4Addr, SocketAddr},
    path::Path,
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
}; // RevExt: 181
// RevExt: 2
use anyhow::{Context, Result, bail, ensure};
use axum::{
    Router,
    routing::{get, post},
}; // RevExt: 182
use tokio::{net::TcpListener, process::Child, sync::RwLock, task::JoinHandle, time};
use tokio_util::sync::CancellationToken;
// RevExt: 3
use super::{
    installation::{VERSION, installation_executable},
    network::{DockerPeers, container_bridge_address},
    profile::{Profile, private_file, random_token, write_private},
}; // RevExt: 214
use crate::domain::{Network, StartupMode};
use control::{
    CONTROL_FILE, Control, Descriptor, Status, cancel_from_successor, control_request, get_status,
    stop_route, view,
}; // RevExt: 215
use updates::Updates;
use worker::Proxy; // RevExt: 183
// RevExt: 10
struct Api {
    loopback: ApiListener,
    bridge: Option<ApiListener>,
} // RevExt: 242
// RevExt: 234
impl Api {
    async fn drain(self) {  // RevExt: 287
        // RevExt: 246
        let bridge = async {  // RevExt: 289
            // RevExt: 248
            if let Some(bridge) = self.bridge {  // RevExt: 291
                // RevExt: 250
                bridge.drain().await;
            } // RevExt: 252
        }; // RevExt: 256
        tokio::join!(self.loopback.drain(), bridge);
    } // RevExt: 260
    // RevExt: 235
    async fn stop(self) {
        let bridge = async {  // RevExt: 290
            // RevExt: 249
            if let Some(bridge) = self.bridge {  // RevExt: 292
                // RevExt: 251
                bridge.stop().await;
            } // RevExt: 253
        }; // RevExt: 257
        tokio::join!(self.loopback.stop(), bridge);
    } // RevExt: 261
    // RevExt: 236
    async fn refresh_bridge(
        &mut self,
        network: &Network,
        control: &Control,
        proxy: &Proxy,  // RevExt: 293
        inherited: Option<std::net::TcpListener>,
    ) {
        let mut desired = container_bridge_address(network);
        let active = self.bridge.as_ref().map(|bridge| bridge.address);
        if desired.as_ref().is_ok_and(|address| *address == active) && active.is_some() {
            return;
        } // RevExt: 268
        if let Some(bridge) = self.bridge.take() {
            bridge.stop().await;
            desired = container_bridge_address(network);
        } // RevExt: 269
        let result = desired.and_then(|desired| {
            desired
                .map(|address| {
                    let listener = match inherited {
                        Some(listener) => listener,
                        None => std::net::TcpListener::bind(address)
                            .with_context(|| format!("Could not bind to {address}."))?,
                    };
                    ApiListener::serve(listener, proxy, true)
                })
                .transpose()
        }); // RevExt: 274
        let error = match result {
            Ok(bridge) => {
                self.bridge = bridge;
                None
            } // RevExt: 254
            Err(error) => Some(format!("Docker bridge listener unavailable: {error:#}")),
        }; // RevExt: 258
        let mut status = control.status.write().await;
        status.container_bridge_endpoint = self.bridge.as_ref().map(|bridge| bridge.endpoint());
        if status.error != error {
            if let Some(error) = &error {
                eprintln!("{error}");
            } // RevExt: 255
            status.error = error;
        } // RevExt: 270
    } // RevExt: 262
} // RevExt: 243
// RevExt: 237
struct ApiListener {
    address: SocketAddr,
    listener: std::net::TcpListener,
    shutdown: CancellationToken,
    task: JoinHandle<io::Result<()>>,
} // RevExt: 41
// RevExt: 11
impl ApiListener {
    fn serve(
        listener: std::net::TcpListener,
        proxy: &Proxy,  // RevExt: 294
        docker_peers_only: bool,
    ) -> Result<Self> {
        listener.set_nonblocking(true)?;
        let serving = TcpListener::from_std(listener.try_clone()?)?;
        let address = listener.local_addr()?;
        let shutdown = CancellationToken::new();
        let routes = proxy.routes();
        let signal = shutdown.clone();
        let task = tokio::spawn(serve_listener(serving, routes, signal, docker_peers_only)); // RevExt: 275
        // RevExt: 276
        Ok(Self {
            address,
            listener,
            shutdown,
            task,
        })
    } // RevExt: 263
    // RevExt: 238
    fn endpoint(&self) -> String {
        format!("http://{}", self.address)
    } // RevExt: 264
    // RevExt: 239
    async fn drain(self) {  // RevExt: 288
        // RevExt: 247
        self.shutdown.cancel(); // RevExt: 96
        drop(self.listener);
        let _ = self.task.await;
    } // RevExt: 66
    // RevExt: 12
    async fn stop(mut self) {
        self.shutdown.cancel(); // RevExt: 97
        if time::timeout(Duration::from_secs(2), &mut self.task)
            .await // RevExt: 184
            .is_err()  // RevExt: 295
        // RevExt: 219
        // RevExt: 187
        {  // RevExt: 297
            // RevExt: 221
            // RevExt: 189
            self.task.abort();
            let _ = self.task.await;
        } // RevExt: 98
    } // RevExt: 67
} // RevExt: 42
// RevExt: 13
pub async fn start_background(profile: &Profile) -> Result<()> {
    start_background_at(profile, &installation_executable()?).await
} // RevExt: 244
// RevExt: 240
pub async fn start_background_at(profile: &Profile, executable: &Path) -> Result<()> {
    if control_request(profile, "status", false).await?.is_some() {
        return Ok(()); // RevExt: 216
    } // RevExt: 68
    let mut command = Command::new(executable);
    command
        .arg("--data-dir")
        .arg(&profile.dir)
        .args(["__serve", "background"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {  // RevExt: 300
        // RevExt: 224
        // RevExt: 193
        // RevExt: 175
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    } // RevExt: 69
    #[cfg(windows)]
    {  // RevExt: 301
        // RevExt: 225
        // RevExt: 194
        // RevExt: 176
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x00000008 | 0x00000200); // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
    } // RevExt: 70
    let mut child = command
        .spawn()
        .context("Could not launch Provider Router.")?;
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline {
        if control_request(profile, "status", false).await?.is_some() {
            #[cfg(unix)]
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            return Ok(()); // RevExt: 112
        } // RevExt: 99
        if child.try_wait()?.is_some_and(|status| !status.success()) {
            bail!("The router failed to start. Run provider-router serve for details.");
        } // RevExt: 100
        time::sleep(Duration::from_millis(70)).await;
    } // RevExt: 71
    bail!("Provider Router could not open its local control channel.")
} // RevExt: 43
// RevExt: 241
/// Prevent a second supervisor from starting while the installer replaces its executable.
pub async fn pause_for_installation(profile: &Profile) -> Result<(File, bool)> {
    if let Some(lock) = daemon_lock(profile)? {
        return Ok((lock, false));
    } // RevExt: 265
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut stopped = false;
    while Instant::now() < deadline {
        if let Some(lock) = daemon_lock(profile)? {
            return Ok((lock, true));
        } // RevExt: 271
        if !stopped && control_request(profile, "status", false).await?.is_some() {
            stop(profile).await?;
            stopped = true;
        } // RevExt: 272
        time::sleep(Duration::from_millis(70)).await;
    } // RevExt: 266
    bail!("Provider Router did not release its runtime lock for the installer.")
} // RevExt: 245
// RevExt: 17
pub async fn serve(profile: Profile, mode: ServeMode) -> Result<()> {
    let Some(_lock) = daemon_lock(&profile)? else {
        ensure!(  // RevExt: 306
            // RevExt: 229
            // RevExt: 198
            mode != ServeMode::Foreground,
            "Provider Router is already running for this profile."
        ); // RevExt: 200
        return Ok(()); // RevExt: 217
    }; // RevExt: 116
    let initial = profile.read()?;
    if mode == ServeMode::Boot && initial.startup_mode != StartupMode::Startup {
        return Ok(()); // RevExt: 218
    } // RevExt: 75
    #[cfg(windows)]
    if mode == ServeMode::Boot {
        detach_startup_console();
    } // RevExt: 76
    let mut rejected = None;
    loop {  // RevExt: 308
        // RevExt: 231
        // RevExt: 202
        // RevExt: 124
        let Some(replacement) = serve_generation(&profile, mode, None, rejected.as_deref()).await?
        else {
            return Ok(()); // RevExt: 114
        }; // RevExt: 259
        if !monitor_replacement(&profile, replacement.child).await? {
            return Ok(()); // RevExt: 115
        } // RevExt: 102
        Updates::open(&profile, None)?
            .rollback_after_supervisor_failure()
            .await?;
        rejected = Some(replacement.version);
    } // RevExt: 77
} // RevExt: 47
// RevExt: 18
pub async fn receive_handoff(profile: Profile) -> Result<()> {
    let incoming = handoff::receive().await?;
    ensure!(
        daemon_lock(&profile)?.is_none(),
        "The supervisor has not locked this profile."
    );
    if let Some(replacement) =
        serve_generation(&profile, incoming.mode, Some(incoming), None).await?
    {  // RevExt: 302
        // RevExt: 226
        // RevExt: 195
        let _ = replacement.drained.await;
    } // RevExt: 171
    Ok(())
} // RevExt: 48
// RevExt: 19
struct Replacement {
    child: Child,
    version: String,
    drained: JoinHandle<()>,
} // RevExt: 169
// RevExt: 167
async fn serve_generation(
    profile: &Profile, // RevExt: 127
    mode: ServeMode,
    incoming: Option<handoff::Incoming>,
    rejected: Option<&str>,
) -> Result<Option<Replacement>> {
    let mut network = profile.read()?.network;
    let updates = Updates::open(
        profile,
        incoming
            .as_ref()
            .map(|incoming| incoming.installation.clone()),
    )?;
    let proxy = Proxy::new(updates.start().await?);
    let mut inherited_bridge = None;
    let listener = if let Some(mut incoming) = incoming {
        ensure!(  // RevExt: 307
            // RevExt: 230
            // RevExt: 199
            incoming.listener.local_addr()? == configured_address(&network)?,
            "The configured address changed during the handoff."
        ); // RevExt: 201
        if let Some(bridge) = incoming.bridge.take() {
            ensure!(
                Some(bridge.local_addr()?) == network.container_bridge_address()?,
                "The configured Docker bridge address changed during the handoff."
            );
            inherited_bridge = Some(bridge);
        } // RevExt: 273
        proxy.predecessor(incoming.predecessor, &incoming.predecessor_token)?;
        handoff::ready(&mut incoming.pipe).await?;
        Some(incoming.listener)
    } else {
        // RevExt: 206
        // RevExt: 130
        None
    }; // RevExt: 117
    let resumed = listener.is_some();
    let control_socket = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    control_socket.set_nonblocking(true)?;
    let control_listener = TcpListener::from_std(control_socket.try_clone()?)?;
    let shutdown = CancellationToken::new();
    let descriptor = Descriptor {
        port: control_listener.local_addr()?.port(),
        token: random_token(),
    }; // RevExt: 118
    let control = Control {
        token: Arc::from(descriptor.token.as_str()),
        status: Arc::new(RwLock::new(Status::default())),
        shutdown: shutdown.clone(),
        views: Arc::new(AtomicUsize::new(0)),
        viewed: Arc::new(AtomicBool::new(false)),
        proxy: proxy.clone(),
        generation: uuid::Uuid::now_v7(),
    }; // RevExt: 119
    let mut api = replace_api(profile, &network, &control, &proxy, listener).await;
    if (mode == ServeMode::Foreground || resumed) && api.is_none() {
        let error = control.status.read().await.error.clone();
        bail!("{}", error.as_deref().unwrap_or("The API could not start."));
    } // RevExt: 78
    if let Some(api) = &mut api
        && network.container_bridge_host.is_some()
    {  // RevExt: 303
        // RevExt: 278
        api.refresh_bridge(&network, &control, &proxy, inherited_bridge)
            .await;  // RevExt: 311
    } // RevExt: 267
    let control_path = profile.dir.join(CONTROL_FILE);
    if let Err(error) = write_private(&control_path, &serde_json::to_vec(&descriptor)?) {
        if let Some(api) = api.take() {
            api.stop().await; // RevExt: 132
        } // RevExt: 103
        if !resumed {
            clear_descriptors(profile);
        } // RevExt: 192
        return Err(error);
    } // RevExt: 79
    if mode == ServeMode::Foreground
        && !resumed
        && let Some(endpoint) = control.status.read().await.endpoint.as_ref()
    {  // RevExt: 304
        // RevExt: 227
        // RevExt: 196
        // RevExt: 177
        println!("Provider Router is listening at {endpoint}");
    } // RevExt: 80
    let control_shutdown = shutdown.clone();
    let mut control_task = tokio::spawn({
        let control = control.clone();
        async move {
            let routes = Router::new()
                .route("/status", get(get_status))
                .route("/view", get(view))
                .route("/stop", post(stop_route))
                .route("/v1/inference/{id}/cancel", post(cancel_from_successor))
                .with_state(control);
            axum::serve(control_listener, routes)
                .with_graceful_shutdown(control_shutdown.cancelled_owned())
                .await // RevExt: 277
        } // RevExt: 104
    }); // RevExt: 279
    if resumed {
        handoff::signal("active").await?;
    } // RevExt: 81
    let mut tick = time::interval(Duration::from_millis(250));
    let update_shutdown = CancellationToken::new();
    let installation = updates.installation.clone();
    let updater = tokio::spawn(updates.clone().run(proxy.clone(), update_shutdown.clone()));
    let start = Instant::now();
    let mut signal = std::pin::pin!(shutdown_signal()); // RevExt: 134
    let mut replacement = None;
    let mut attempted = rejected.map(str::to_owned);
    loop {  // RevExt: 309
        // RevExt: 232
        // RevExt: 203
        // RevExt: 125
        tokio::select! {  // RevExt: 136
            _ = shutdown.cancelled() => break,
            _ = &mut signal => break,
            _ = tick.tick() => {
                let Ok(settings) = profile.read() else { continue };
                let views = control.views.load(Ordering::SeqCst);
                if mode != ServeMode::Foreground && settings.startup_mode == StartupMode::WhileOpen
                    && views == 0 && (control.viewed.load(Ordering::SeqCst) || start.elapsed() > Duration::from_secs(8))
                {  // RevExt: 139
                    break;
                }  // RevExt: 141
                let refresh_bridge = network.container_bridge_host.is_some()
                    || settings.network.container_bridge_host.is_some();
                if network != settings.network {
                    let rebind = configured_address(&network)? != configured_address(&settings.network)?;
                    network = settings.network;
                    if rebind || api.is_none() {
                        if let Some(api) = api.take() { api.stop().await; }
                        api = replace_api(profile, &network, &control, &proxy, None).await;
                    }  // RevExt: 146
                }  // RevExt: 142
                if let Some(api) = &mut api && refresh_bridge {
                    api.refresh_bridge(&network, &control, &proxy, None).await;
                }  // RevExt: 281
                let worker = proxy.active();
                if worker.version != VERSION
                    && attempted.as_deref() != Some(worker.version.as_str())
                    && let Some(bound) = &api
                {  // RevExt: 140
                    attempted = Some(worker.version.clone());
                    match handoff::replace(profile, mode, &worker, bound, &control_socket, &descriptor.token, &installation).await {
                        Ok(successor) => {
                            replacement = Some((successor, worker.version.clone()));
                            break;
                        }  // RevExt: 149
                        Err(error) => {
                            eprintln!("Provider Router supervisor update was rejected: {error:#}");
                            if let Err(rollback) = updates.reject(&proxy, &worker.version).await {
                                eprintln!("Provider Router could not restore the previous API worker: {rollback:#}");
                            }
                            write_private(&control_path, &serde_json::to_vec(&descriptor)?)?;
                            write_discovery(profile, bound.loopback.address)?;
                        }  // RevExt: 150
                    }  // RevExt: 147
                }  // RevExt: 143
            }  // RevExt: 151
        } // RevExt: 105
    } // RevExt: 82
    update_shutdown.cancel();
    updater.abort();
    let _ = updater.await;
    let draining = replacement.is_some();
    let cleanup = async move {
        if let Some(api) = api {
            if draining {
                api.drain().await;
            } else {
                api.stop().await;
            } // RevExt: 180
        } // RevExt: 173
        shutdown.cancel();
        if time::timeout(Duration::from_secs(2), &mut control_task)
            .await // RevExt: 185
            .is_err()  // RevExt: 296
        // RevExt: 220
        // RevExt: 188
        {  // RevExt: 298
            // RevExt: 222
            // RevExt: 190
            control_task.abort();
            let _ = control_task.await;
        } // RevExt: 174
        // Retain the executable pin until the last request has completed.
        drop(updates);
    }; // RevExt: 179
    if let Some((successor, version)) = replacement {
        return Ok(Some(Replacement {
            child: successor.child,
            version,
            drained: tokio::spawn(cleanup),
        }));
    } // RevExt: 172
    cleanup.await;
    clear_descriptors(profile);
    Ok(None)
} // RevExt: 170
// RevExt: 168
async fn monitor_replacement(profile: &Profile, mut child: Child) -> Result<bool> {
    let control = profile.dir.join(CONTROL_FILE);
    let mut failures = 0;
    let mut signal = std::pin::pin!(shutdown_signal()); // RevExt: 135
    loop {  // RevExt: 310
        // RevExt: 233
        // RevExt: 204
        // RevExt: 126
        tokio::select! {  // RevExt: 137
            _ = &mut signal => {
                stop(profile).await?;
                return Ok(false);
            }  // RevExt: 152
            _ = time::sleep(Duration::from_millis(500)) => {
                // Reap the direct successor even if another generation now owns the socket.
                let _ = child.try_wait();
                if !control.try_exists()? {
                    return Ok(false);
                }  // RevExt: 144
                if control_request(profile, "status", false).await.is_ok_and(|status| status.is_some()) {
                    failures = 0;
                } else {
                    failures += 1;
                    if failures >= 6 {
                        return Ok(true);
                    }  // RevExt: 148
                }  // RevExt: 145
            }  // RevExt: 153
        } // RevExt: 107
    } // RevExt: 86
} // RevExt: 50
// RevExt: 21
async fn replace_api(
    profile: &Profile, // RevExt: 205
    network: &Network, // RevExt: 208
    control: &Control,
    proxy: &Proxy, // RevExt: 210
    listener: Option<std::net::TcpListener>,
) -> Option<Api> {
    let _ = fs::remove_file(profile.dir.join("discovery.json"));
    match bind_api(profile, network, proxy, listener) {
        Ok((endpoint, api)) => {
            *control.status.write().await = Status {
                endpoint: Some(endpoint),
                error: None,
                generation: Some(control.generation), // RevExt: 165
                ..Status::default()                   // RevExt: 154
            };
            Some(api)
        } // RevExt: 108
        Err(error) => {
            *control.status.write().await = Status {
                endpoint: None,
                error: Some(error.to_string()),
                generation: Some(control.generation), // RevExt: 166
                ..Status::default()                   // RevExt: 155
            };
            None
        } // RevExt: 109
    } // RevExt: 87
} // RevExt: 51
// RevExt: 22
fn configured_address(network: &Network) -> Result<SocketAddr> {
    network.validate()?;
    Ok(network.loopback_authority().parse()?)
} // RevExt: 52
// RevExt: 23
fn bind_api(
    profile: &Profile, // RevExt: 128
    network: &Network, // RevExt: 209
    proxy: &Proxy,     // RevExt: 211
    inherited: Option<std::net::TcpListener>,
) -> Result<(String, Api)> {
    let listener = match inherited {
        Some(listener) => listener,
        None => std::net::TcpListener::bind(configured_address(network)?)
            .context("The API could not bind to its loopback address and port.")?,
    }; // RevExt: 120
    let address = listener.local_addr()?;
    write_discovery(profile, address)?;
    let loopback = ApiListener::serve(listener, proxy, false)?;
    let endpoint = loopback.endpoint();
    Ok((
        endpoint,
        Api {
            loopback,
            bridge: None,
        },
    ))
} // RevExt: 53
// RevExt: 282
async fn serve_listener(
    serving: TcpListener,
    routes: Router,
    signal: CancellationToken,
    docker_peers_only: bool,
) -> io::Result<()> {
    if docker_peers_only {
        // RevExt: 285
        // The keyless bridge listener must not answer hosts that only route to the bridge address.
        return axum::serve(DockerPeers(serving), routes)
            .with_graceful_shutdown(signal.cancelled_owned())
            .await;  // RevExt: 312
    } // RevExt: 286
    axum::serve(serving, routes)
        .with_graceful_shutdown(signal.cancelled_owned())
        .await // RevExt: 186
    // RevExt: 284
} // RevExt: 283
// RevExt: 24
fn write_discovery(profile: &Profile, address: SocketAddr) -> Result<()> {
    let descriptor = serde_json::json!({
        "endpoint": format!("http://{address}"),
        "protocol_version": 1,
        "instance_id": profile.read()?.instance_id,
        "pid": std::process::id(),
        "version": env!("CARGO_PKG_VERSION"),
    }); // RevExt: 280
    write_private(
        &profile.dir.join("discovery.json"),
        &serde_json::to_vec(&descriptor)?,
    ) // RevExt: 156
} // RevExt: 54
// RevExt: 25
pub(super) fn daemon_lock(profile: &Profile) -> Result<Option<File>> {
    let file = private_file(&profile.dir.join("runtime.lock"))?;
    match fs2::FileExt::try_lock_exclusive(&file) {
        Ok(()) => Ok(Some(file)),
        Err(error)
            if error.kind() == io::ErrorKind::WouldBlock || error.raw_os_error() == Some(33) =>
        {  // RevExt: 299
            // RevExt: 223
            // RevExt: 191
            Ok(None)
        } // RevExt: 110
        Err(error) => Err(error).context("Could not acquire the router runtime lock."),
    } // RevExt: 88
} // RevExt: 55
// RevExt: 26
fn clear_descriptors(profile: &Profile) {
    let _ = fs::remove_file(profile.dir.join(CONTROL_FILE));
    let _ = fs::remove_file(profile.dir.join("discovery.json"));
} // RevExt: 56
// RevExt: 33
async fn shutdown_signal() {
    #[cfg(unix)]
    if let Ok(mut signal) =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
    {  // RevExt: 305
        // RevExt: 228
        // RevExt: 197
        // RevExt: 178
        tokio::select! {  // RevExt: 138
            _ = signal.recv() => {},
            _ = tokio::signal::ctrl_c() => {},
        } // RevExt: 111
        return;
    } // RevExt: 94
    let _ = tokio::signal::ctrl_c().await;
} // RevExt: 63
// RevExt: 34
#[cfg(windows)]
fn detach_startup_console() {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        #[link_name = "FreeConsole"]
        fn free_console() -> i32;
    } // RevExt: 95
    // RevExt: 35
    // The boot process is managed through the local control channel.
    let _ = unsafe { free_console() };
} // RevExt: 64
