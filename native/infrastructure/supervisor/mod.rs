//! Own the local API process and authenticated control channel.
mod control;
mod handoff;
mod socket_transfer;
mod updates;
mod worker;
use super::{
    installation::{VERSION, installation_executable},
    network::{DockerPeers, container_bridge_address},
    profile::{Profile, private_file, random_token, write_private},
};
use crate::domain::{Network, StartupMode};
use anyhow::{Context, Result, bail, ensure};
use axum::{
    Router,
    routing::{get, post},
};
use control::{
    CONTROL_FILE, Control, Descriptor, Status, cancel_from_successor, control_request, get_status,
    stop_route, view,
};
pub use control::{SessionLease, attach, status, stop};
pub use handoff::ServeMode;
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
};
use tokio::{net::TcpListener, process::Child, sync::RwLock, task::JoinHandle, time};
use tokio_util::sync::CancellationToken;
use updates::Updates;
pub use updates::{forward_cli, installed_release_version};
use worker::Proxy;
pub use worker::serve as serve_worker;
struct Api {
    loopback: ApiListener,
    bridge: Option<ApiListener>,
}
impl Api {
    async fn drain(self) {
        let bridge = async {
            if let Some(bridge) = self.bridge {
                bridge.drain().await;
            }
        };
        tokio::join!(self.loopback.drain(), bridge);
    }
    async fn stop(self) {
        let bridge = async {
            if let Some(bridge) = self.bridge {
                bridge.stop().await;
            }
        };
        tokio::join!(self.loopback.stop(), bridge);
    }
    async fn refresh_bridge(
        &mut self,
        network: &Network,
        control: &Control,
        proxy: &Proxy,
        inherited: Option<std::net::TcpListener>,
    ) {
        let mut desired = container_bridge_address(network);
        let active = self.bridge.as_ref().map(|bridge| bridge.address);
        if desired.as_ref().is_ok_and(|address| *address == active) && active.is_some() {
            return;
        }
        if let Some(bridge) = self.bridge.take() {
            bridge.stop().await;
            desired = container_bridge_address(network);
        }
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
        });
        let error = match result {
            Ok(bridge) => {
                self.bridge = bridge;
                None
            }
            Err(error) => Some(format!("Docker bridge listener unavailable: {error:#}")),
        };
        let mut status = control.status.write().await;
        status.container_bridge_endpoint = self.bridge.as_ref().map(|bridge| bridge.endpoint());
        if status.error != error {
            if let Some(error) = &error {
                eprintln!("{error}");
            }
            status.error = error;
        }
    }
}
struct ApiListener {
    address: SocketAddr,
    listener: std::net::TcpListener,
    shutdown: CancellationToken,
    task: JoinHandle<io::Result<()>>,
}
impl ApiListener {
    fn serve(
        listener: std::net::TcpListener,
        proxy: &Proxy,
        docker_peers_only: bool,
    ) -> Result<Self> {
        listener.set_nonblocking(true)?;
        let serving = TcpListener::from_std(listener.try_clone()?)?;
        let address = listener.local_addr()?;
        let shutdown = CancellationToken::new();
        let routes = proxy.routes();
        let signal = shutdown.clone();
        let task = tokio::spawn(serve_listener(serving, routes, signal, docker_peers_only));
        Ok(Self {
            address,
            listener,
            shutdown,
            task,
        })
    }
    fn endpoint(&self) -> String {
        format!("http://{}", self.address)
    }
    async fn drain(self) {
        self.shutdown.cancel();
        drop(self.listener);
        let _ = self.task.await;
    }
    async fn stop(mut self) {
        self.shutdown.cancel();
        if time::timeout(Duration::from_secs(2), &mut self.task)
            .await
            .is_err()
        {
            self.task.abort();
            let _ = self.task.await;
        }
    }
}
pub async fn start_background(profile: &Profile) -> Result<()> {
    start_background_at(profile, &installation_executable()?).await
}
pub async fn start_background_at(profile: &Profile, executable: &Path) -> Result<()> {
    if control_request(profile, "status", false).await?.is_some() {
        return Ok(());
    }
    let mut command = Command::new(executable);
    command
        .arg("--data-dir")
        .arg(&profile.dir)
        .args(["__serve", "background"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x00000008 | 0x00000200); // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
    }
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
            return Ok(());
        }
        if child.try_wait()?.is_some_and(|status| !status.success()) {
            bail!("The router failed to start. Run provider-router serve for details.");
        }
        time::sleep(Duration::from_millis(70)).await;
    }
    bail!("Provider Router could not open its local control channel.")
}
/// Prevent a second supervisor from starting while the installer replaces its executable.
pub async fn pause_for_installation(profile: &Profile) -> Result<(File, bool)> {
    if let Some(lock) = daemon_lock(profile)? {
        return Ok((lock, false));
    }
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut stopped = false;
    while Instant::now() < deadline {
        if let Some(lock) = daemon_lock(profile)? {
            return Ok((lock, true));
        }
        if !stopped && control_request(profile, "status", false).await?.is_some() {
            stop(profile).await?;
            stopped = true;
        }
        time::sleep(Duration::from_millis(70)).await;
    }
    bail!("Provider Router did not release its runtime lock for the installer.")
}
pub async fn serve(profile: Profile, mode: ServeMode) -> Result<()> {
    let Some(_lock) = daemon_lock(&profile)? else {
        ensure!(
            mode != ServeMode::Foreground,
            "Provider Router is already running for this profile."
        );
        return Ok(());
    };
    let initial = profile.read()?;
    if mode == ServeMode::Boot && initial.startup_mode != StartupMode::Startup {
        return Ok(());
    }
    #[cfg(windows)]
    if mode == ServeMode::Boot {
        detach_startup_console();
    }
    let mut rejected = None;
    loop {
        let Some(replacement) = serve_generation(&profile, mode, None, rejected.as_deref()).await?
        else {
            return Ok(());
        };
        if !monitor_replacement(&profile, replacement.child).await? {
            return Ok(());
        }
        Updates::open(&profile, None)?
            .rollback_after_supervisor_failure()
            .await?;
        rejected = Some(replacement.version);
    }
}
pub async fn receive_handoff(profile: Profile) -> Result<()> {
    let incoming = handoff::receive().await?;
    ensure!(
        daemon_lock(&profile)?.is_none(),
        "The supervisor has not locked this profile."
    );
    if let Some(replacement) =
        serve_generation(&profile, incoming.mode, Some(incoming), None).await?
    {
        let _ = replacement.drained.await;
    }
    Ok(())
}
struct Replacement {
    child: Child,
    version: String,
    drained: JoinHandle<()>,
}
async fn serve_generation(
    profile: &Profile,
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
        ensure!(
            incoming.listener.local_addr()? == configured_address(&network)?,
            "The configured address changed during the handoff."
        );
        if let Some(bridge) = incoming.bridge.take() {
            ensure!(
                Some(bridge.local_addr()?) == network.container_bridge_address()?,
                "The configured Docker bridge address changed during the handoff."
            );
            inherited_bridge = Some(bridge);
        }
        proxy.predecessor(incoming.predecessor, &incoming.predecessor_token)?;
        handoff::ready(&mut incoming.pipe).await?;
        Some(incoming.listener)
    } else {
        None
    };
    let resumed = listener.is_some();
    let control_socket = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    control_socket.set_nonblocking(true)?;
    let control_listener = TcpListener::from_std(control_socket.try_clone()?)?;
    let shutdown = CancellationToken::new();
    let descriptor = Descriptor {
        port: control_listener.local_addr()?.port(),
        token: random_token(),
    };
    let control = Control {
        token: Arc::from(descriptor.token.as_str()),
        status: Arc::new(RwLock::new(Status::default())),
        shutdown: shutdown.clone(),
        views: Arc::new(AtomicUsize::new(0)),
        viewed: Arc::new(AtomicBool::new(false)),
        proxy: proxy.clone(),
        generation: uuid::Uuid::now_v7(),
    };
    let mut api = replace_api(profile, &network, &control, &proxy, listener).await;
    if (mode == ServeMode::Foreground || resumed) && api.is_none() {
        let error = control.status.read().await.error.clone();
        bail!("{}", error.as_deref().unwrap_or("The API could not start."));
    }
    if let Some(api) = &mut api
        && network.container_bridge_host.is_some()
    {
        api.refresh_bridge(&network, &control, &proxy, inherited_bridge)
            .await;
    }
    let control_path = profile.dir.join(CONTROL_FILE);
    if let Err(error) = write_private(&control_path, &serde_json::to_vec(&descriptor)?) {
        if let Some(api) = api.take() {
            api.stop().await;
        }
        if !resumed {
            clear_descriptors(profile);
        }
        return Err(error);
    }
    if mode == ServeMode::Foreground
        && !resumed
        && let Some(endpoint) = control.status.read().await.endpoint.as_ref()
    {
        println!("Provider Router is listening at {endpoint}");
    }
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
                .await
        }
    });
    if resumed {
        handoff::signal("active").await?;
    }
    let mut tick = time::interval(Duration::from_millis(250));
    let update_shutdown = CancellationToken::new();
    let installation = updates.installation.clone();
    let updater = tokio::spawn(updates.clone().run(proxy.clone(), update_shutdown.clone()));
    let start = Instant::now();
    let mut signal = std::pin::pin!(shutdown_signal());
    let mut replacement = None;
    let mut attempted = rejected.map(str::to_owned);
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => break,
            _ = &mut signal => break,
            _ = tick.tick() => {
                let Ok(settings) = profile.read() else { continue };
                let views = control.views.load(Ordering::SeqCst);
                if mode != ServeMode::Foreground && settings.startup_mode == StartupMode::WhileOpen
                    && views == 0 && (control.viewed.load(Ordering::SeqCst) || start.elapsed() > Duration::from_secs(8))
                {
                    break;
                }
                let refresh_bridge = network.container_bridge_host.is_some()
                    || settings.network.container_bridge_host.is_some();
                if network != settings.network {
                    let rebind = configured_address(&network)? != configured_address(&settings.network)?;
                    network = settings.network;
                    if rebind || api.is_none() {
                        if let Some(api) = api.take() { api.stop().await; }
                        api = replace_api(profile, &network, &control, &proxy, None).await;
                    }
                }
                if let Some(api) = &mut api && refresh_bridge {
                    api.refresh_bridge(&network, &control, &proxy, None).await;
                }
                let worker = proxy.active();
                if worker.version != VERSION
                    && attempted.as_deref() != Some(worker.version.as_str())
                    && let Some(bound) = &api
                {
                    attempted = Some(worker.version.clone());
                    match handoff::replace(profile, mode, &worker, bound, &control_socket, &descriptor.token, &installation).await {
                        Ok(successor) => {
                            replacement = Some((successor, worker.version.clone()));
                            break;
                        }
                        Err(error) => {
                            eprintln!("Provider Router supervisor update was rejected: {error:#}");
                            if let Err(rollback) = updates.reject(&proxy, &worker.version).await {
                                eprintln!("Provider Router could not restore the previous API worker: {rollback:#}");
                            }
                            write_private(&control_path, &serde_json::to_vec(&descriptor)?)?;
                            write_discovery(profile, bound.loopback.address)?;
                        }
                    }
                }
            }
        }
    }
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
            }
        }
        shutdown.cancel();
        if time::timeout(Duration::from_secs(2), &mut control_task)
            .await
            .is_err()
        {
            control_task.abort();
            let _ = control_task.await;
        }
        // Retain the executable pin until the last request has completed.
        drop(updates);
    };
    if let Some((successor, version)) = replacement {
        return Ok(Some(Replacement {
            child: successor.child,
            version,
            drained: tokio::spawn(cleanup),
        }));
    }
    cleanup.await;
    clear_descriptors(profile);
    Ok(None)
}
async fn monitor_replacement(profile: &Profile, mut child: Child) -> Result<bool> {
    let control = profile.dir.join(CONTROL_FILE);
    let mut failures = 0;
    let mut signal = std::pin::pin!(shutdown_signal());
    loop {
        tokio::select! {
            _ = &mut signal => {
                stop(profile).await?;
                return Ok(false);
            }
            _ = time::sleep(Duration::from_millis(500)) => {
                // Reap the direct successor even if another generation now owns the socket.
                let _ = child.try_wait();
                if !control.try_exists()? {
                    return Ok(false);
                }
                if control_request(profile, "status", false).await.is_ok_and(|status| status.is_some()) {
                    failures = 0;
                } else {
                    failures += 1;
                    if failures >= 6 {
                        return Ok(true);
                    }
                }
            }
        }
    }
}
async fn replace_api(
    profile: &Profile,
    network: &Network,
    control: &Control,
    proxy: &Proxy,
    listener: Option<std::net::TcpListener>,
) -> Option<Api> {
    let _ = fs::remove_file(profile.dir.join("discovery.json"));
    match bind_api(profile, network, proxy, listener) {
        Ok((endpoint, api)) => {
            *control.status.write().await = Status {
                endpoint: Some(endpoint),
                error: None,
                generation: Some(control.generation),
                ..Status::default()
            };
            Some(api)
        }
        Err(error) => {
            *control.status.write().await = Status {
                endpoint: None,
                error: Some(error.to_string()),
                generation: Some(control.generation),
                ..Status::default()
            };
            None
        }
    }
}
fn configured_address(network: &Network) -> Result<SocketAddr> {
    network.validate()?;
    Ok(network.loopback_authority().parse()?)
}
fn bind_api(
    profile: &Profile,
    network: &Network,
    proxy: &Proxy,
    inherited: Option<std::net::TcpListener>,
) -> Result<(String, Api)> {
    let listener = match inherited {
        Some(listener) => listener,
        None => std::net::TcpListener::bind(configured_address(network)?)
            .context("The API could not bind to its loopback address and port.")?,
    };
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
}
async fn serve_listener(
    serving: TcpListener,
    routes: Router,
    signal: CancellationToken,
    docker_peers_only: bool,
) -> io::Result<()> {
    if docker_peers_only {
        // The keyless bridge listener must not answer hosts that only route to the bridge address.
        return axum::serve(DockerPeers(serving), routes)
            .with_graceful_shutdown(signal.cancelled_owned())
            .await;
    }
    axum::serve(serving, routes)
        .with_graceful_shutdown(signal.cancelled_owned())
        .await
}
fn write_discovery(profile: &Profile, address: SocketAddr) -> Result<()> {
    let descriptor = serde_json::json!({
        "endpoint": format!("http://{address}"),
        "protocol_version": 1,
        "instance_id": profile.read()?.instance_id,
        "pid": std::process::id(),
        "version": env!("CARGO_PKG_VERSION"),
    });
    write_private(
        &profile.dir.join("discovery.json"),
        &serde_json::to_vec(&descriptor)?,
    )
}
pub(super) fn daemon_lock(profile: &Profile) -> Result<Option<File>> {
    let file = private_file(&profile.dir.join("runtime.lock"))?;
    match fs2::FileExt::try_lock_exclusive(&file) {
        Ok(()) => Ok(Some(file)),
        Err(error)
            if error.kind() == io::ErrorKind::WouldBlock || error.raw_os_error() == Some(33) =>
        {
            Ok(None)
        }
        Err(error) => Err(error).context("Could not acquire the router runtime lock."),
    }
}
fn clear_descriptors(profile: &Profile) {
    let _ = fs::remove_file(profile.dir.join(CONTROL_FILE));
    let _ = fs::remove_file(profile.dir.join("discovery.json"));
}
async fn shutdown_signal() {
    #[cfg(unix)]
    if let Ok(mut signal) =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
    {
        tokio::select! {
            _ = signal.recv() => {},
            _ = tokio::signal::ctrl_c() => {},
        }
        return;
    }
    let _ = tokio::signal::ctrl_c().await;
}
#[cfg(windows)]
fn detach_startup_console() {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        #[link_name = "FreeConsole"]
        fn free_console() -> i32;
    }
    // The boot process is managed through the local control channel.
    let _ = unsafe { free_console() };
}
