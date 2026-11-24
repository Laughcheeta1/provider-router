//! Two-phase supervisor replacement over private process pipes.
use super::{Api, socket_transfer, updates::Installation, worker::Worker};
use crate::infrastructure::{installation::VERSION, profile::Profile};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{net::TcpListener, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, Stdin},
    process::{Child, Command},
    time,
};
const HANDOFF_TIMEOUT: Duration = Duration::from_secs(30);
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ServeMode {
    Foreground,
    Background,
    Boot,
}
#[derive(Serialize, Deserialize)]
struct Transfer {
    protocol: u32,
    mode: ServeMode,
    public: socket_transfer::SocketOffer,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    bridge: Option<socket_transfer::SocketOffer>,
    control: socket_transfer::SocketOffer,
    control_token: String,
    installation: Installation,
}
pub struct Incoming {
    pub mode: ServeMode,
    pub listener: TcpListener,
    pub bridge: Option<TcpListener>,
    pub predecessor: TcpListener,
    pub predecessor_token: String,
    pub pipe: BufReader<Stdin>,
    pub installation: Installation,
}
pub struct Outgoing {
    pub child: Child,
}
pub async fn receive() -> Result<Incoming> {
    let mut pipe = BufReader::new(tokio::io::stdin());
    let mut message = Vec::new();
    (&mut pipe)
        .take(8192)
        .read_until(b'\n', &mut message)
        .await?;
    let transfer: Transfer =
        serde_json::from_slice(&message).context("A supervisor handoff is required.")?;
    ensure!(
        transfer.protocol == if transfer.bridge.is_some() { 2 } else { 1 },
        "Incompatible supervisor handoff protocol."
    );
    let listener = socket_transfer::receive(transfer.public)?;
    let bridge = transfer.bridge.map(socket_transfer::receive).transpose()?;
    let predecessor = socket_transfer::receive(transfer.control)?;
    ensure!(
        listener.local_addr()?.ip().is_loopback() && predecessor.local_addr()?.ip().is_loopback(),
        "Unexpected inherited listener address."
    );
    Ok(Incoming {
        mode: transfer.mode,
        listener,
        bridge,
        predecessor,
        predecessor_token: transfer.control_token,
        pipe,
        installation: transfer.installation,
    })
}
pub async fn ready(pipe: &mut BufReader<Stdin>) -> Result<()> {
    signal(VERSION).await?;
    let mut commit = [0u8; 1];
    pipe.read_exact(&mut commit).await?;
    ensure!(commit == [1], "The supervisor cancelled the handoff.");
    Ok(())
}
pub async fn signal(message: &str) -> Result<()> {
    let mut stdout = tokio::io::stdout();
    stdout.write_all(message.as_bytes()).await?;
    stdout.write_all(b"\n").await?;
    stdout.flush().await?;
    Ok(())
}
pub(super) async fn replace(
    profile: &Profile,
    mode: ServeMode,
    worker: &Worker,
    api: &Api,
    control: &TcpListener,
    control_token: &str,
    installation: &Installation,
) -> Result<Outgoing> {
    let mut command = Command::new(&worker.executable);
    command
        .arg("--data-dir")
        .arg(&profile.dir)
        .arg("__handoff")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    #[cfg(windows)]
    command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    let listener = &api.loopback.listener;
    let bridge = api.bridge.as_ref().map(|bridge| &bridge.listener);
    let mut listeners = vec![listener, control];
    listeners.extend(bridge);
    socket_transfer::prepare(&mut command, &listeners);
    let mut child = command.spawn()?;
    let result = time::timeout(HANDOFF_TIMEOUT, async {
        let transfer = Transfer {
            protocol: if bridge.is_some() { 2 } else { 1 },
            mode,
            public: socket_transfer::offer(listener, &child)?,
            bridge: bridge
                .map(|listener| socket_transfer::offer(listener, &child))
                .transpose()?,
            control: socket_transfer::offer(control, &child)?,
            control_token: control_token.into(),
            installation: installation.clone(),
        };
        let mut stdin = child
            .stdin
            .take()
            .context("Replacement stdin is unavailable.")?;
        let stdout = child
            .stdout
            .take()
            .context("Replacement stdout is unavailable.")?;
        let mut stdout = BufReader::new(stdout);
        stdin.write_all(&serde_json::to_vec(&transfer)?).await?;
        stdin.write_all(b"\n").await?;
        let mut line = String::new();
        (&mut stdout).take(128).read_line(&mut line).await?;
        ensure!(
            line.trim_end() == worker.version,
            "The replacement supervisor did not become ready."
        );
        stdin.write_all(&[1]).await?;
        line.clear();
        (&mut stdout).take(128).read_line(&mut line).await?;
        ensure!(
            line.trim_end() == "active",
            "The replacement supervisor did not activate."
        );
        Ok::<_, anyhow::Error>(())
    })
    .await;
    match result {
        Ok(Ok(())) => Ok(Outgoing { child }),
        result => {
            let _ = child.kill().await;
            Err(result
                .context("The supervisor handoff timed out.")?
                .unwrap_err())
        }
    }
}
