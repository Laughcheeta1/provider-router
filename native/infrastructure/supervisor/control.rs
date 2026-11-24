//! Authenticated supervisor control channel and terminal session leases.
// RevExt: 208
use std::{
    fs::File,
    io,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
// RevExt: 209
use anyhow::{Context, Result, bail};
use axum::{
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;
use tokio::{sync::RwLock, time};
use tokio_util::sync::CancellationToken;
// RevExt: 210
use crate::infrastructure::{installation::VERSION, profile::Profile};
// RevExt: 211
use super::worker::{Proxy, WORKER_AUTH};
// RevExt: 212
// RevExt: 4
pub(super) const CONTROL_FILE: &str = "control.json";
// RevExt: 6
#[derive(Clone, Default, Debug, Deserialize, Serialize)]
pub struct Status {
    pub endpoint: Option<String>,
    #[serde(default)] // RevExt: 219
    pub container_bridge_endpoint: Option<String>,
    pub error: Option<String>,
    #[serde(default)] // RevExt: 162
    pub generation: Option<uuid::Uuid>,
    #[serde(default)] // RevExt: 163
    pub worker_version: Option<String>,
    #[serde(default)] // RevExt: 164
    pub supervisor_version: Option<String>,
} // RevExt: 37
// RevExt: 7
#[derive(Deserialize, Serialize)]
pub(super) struct Descriptor {
    pub(super) port: u16,
    pub(super) token: String,
} // RevExt: 38
// RevExt: 8
#[derive(Clone)]
pub(super) struct Control {
    pub(super) token: Arc<str>,
    pub(super) status: Arc<RwLock<Status>>,
    pub(super) shutdown: CancellationToken,
    pub(super) views: Arc<AtomicUsize>,
    pub(super) viewed: Arc<AtomicBool>,
    pub(super) proxy: Proxy,
    pub(super) generation: uuid::Uuid,
} // RevExt: 39
// RevExt: 9
struct View(Arc<AtomicUsize>);
impl Drop for View {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    } // RevExt: 65
} // RevExt: 40
// RevExt: 213
pub struct SessionLease(reqwest::Response);
// RevExt: 214
impl SessionLease {
    pub fn belongs_to(&self, generation: uuid::Uuid) -> bool {
        self.0
            .headers()
            .get("x-router-generation")
            .is_some_and(|value| value.as_bytes() == generation.to_string().as_bytes())
    }
} // RevExt: 217
// RevExt: 215
// RevExt: 14
pub async fn attach(profile: &Profile) -> Result<SessionLease> {
    control_request(profile, "view", false)
        .await?
        .map(SessionLease)
        .context("The local router is not running.")
} // RevExt: 44
// RevExt: 15
pub async fn status(profile: &Profile) -> Result<Status> {
    match control_request(profile, "status", false).await? {
        Some(response) => Ok(response.json().await?),
        None => Ok(Status::default()),
    } // RevExt: 72
} // RevExt: 45
// RevExt: 16
pub async fn stop(profile: &Profile) -> Result<()> {
    if control_request(profile, "stop", true).await?.is_none() {
        return Ok(());
    } // RevExt: 73
    let deadline = Instant::now() + Duration::from_secs(4);
    while Instant::now() < deadline {
        if control_request(profile, "status", false).await?.is_none() {
            return Ok(()); // RevExt: 113
        } // RevExt: 101
        time::sleep(Duration::from_millis(70)).await;
    } // RevExt: 74
    bail!("The local router is still shutting down.")
} // RevExt: 46
// RevExt: 27
fn authorized(control: &Control, headers: &HeaderMap) -> bool {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|token| token.as_bytes().ct_eq(control.token.as_bytes()).into())
} // RevExt: 57
// RevExt: 28
pub(super) async fn get_status(State(control): State<Control>, headers: HeaderMap) -> Response {
    if !authorized(&control, &headers) {
        return StatusCode::FORBIDDEN.into_response(); // RevExt: 158
    } // RevExt: 89
    let mut status = control.status.read().await.clone();
    status.worker_version = Some(control.proxy.active().version.clone());
    status.supervisor_version = Some(VERSION.into());
    axum::Json(status).into_response()
} // RevExt: 58
// RevExt: 29
pub(super) async fn stop_route(State(control): State<Control>, headers: HeaderMap) -> Response {
    if !authorized(&control, &headers) {
        return StatusCode::FORBIDDEN.into_response(); // RevExt: 159
    } // RevExt: 90
    control.shutdown.cancel();
    StatusCode::NO_CONTENT.into_response()
} // RevExt: 59
// RevExt: 30
pub(super) async fn cancel_from_successor(
    State(control): State<Control>,
    mut request: Request,
) -> Response {
    let authorized = request
        .headers_mut()
        .remove(WORKER_AUTH)
        .is_some_and(|token| token.as_bytes().ct_eq(control.token.as_bytes()).into());
    if !authorized {
        return StatusCode::FORBIDDEN.into_response(); // RevExt: 160
    } // RevExt: 91
    control.proxy.forward(request).await
} // RevExt: 60
// RevExt: 31
pub(super) async fn view(State(control): State<Control>, headers: HeaderMap) -> Response {
    if !authorized(&control, &headers) {
        return StatusCode::FORBIDDEN.into_response(); // RevExt: 161
    } // RevExt: 92
    control.views.fetch_add(1, Ordering::SeqCst);
    control.viewed.store(true, Ordering::SeqCst);
    let view = View(control.views);
    let only_until_close = async_stream::stream! {
        let _lease = view;
        yield Ok::<_, io::Error>(axum::body::Bytes::from_static(b"ready\n"));
        control.shutdown.cancelled().await;
    }; // RevExt: 121
    (
        [
            (header::CONTENT_TYPE, "application/octet-stream".to_owned()),
            (
                axum::http::HeaderName::from_static("x-router-generation"),
                control.generation.to_string(),
            ),
        ],
        Body::from_stream(only_until_close),
    ) // RevExt: 157
        .into_response()
} // RevExt: 61
// RevExt: 32
pub(super) async fn control_request(
    profile: &Profile, // RevExt: 129
    path: &str,
    post: bool,
) -> Result<Option<reqwest::Response>> {
    let file = match File::open(profile.dir.join(CONTROL_FILE)) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    }; // RevExt: 122
    let descriptor: Descriptor =
        serde_json::from_reader(file).context("The local router control descriptor is invalid.")?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(1))
        .build()?;
    let url = format!("http://127.0.0.1:{}/{path}", descriptor.port);
    let method = if post {
        reqwest::Method::POST
    } else {
        // RevExt: 207
        // RevExt: 131
        reqwest::Method::GET
    }; // RevExt: 123
    let request = client
        .request(method, url)
        .header(header::AUTHORIZATION, descriptor.token);
    match time::timeout(Duration::from_secs(2), request.send()).await {
        Ok(Ok(response)) if response.status().is_success() => Ok(Some(response)),
        Ok(Ok(response)) if response.status() == StatusCode::FORBIDDEN => Ok(None),
        Ok(Err(error)) if error.is_connect() || error.is_timeout() => Ok(None),
        Ok(Err(error)) if path == "status" && closed_socket(&error) => Ok(None),
        Ok(Err(error)) => Err(error).context("Could not contact the local router."),
        Err(_) => Ok(None),
        Ok(Ok(_)) => bail!("The local router rejected the control request."),
    } // RevExt: 93
} // RevExt: 62
// RevExt: 216
fn closed_socket(error: &reqwest::Error) -> bool {
    std::iter::successors(std::error::Error::source(error), |source| source.source())
        .filter_map(|source| source.downcast_ref::<io::Error>())
        .any(|error| {
            matches!(
                error.kind(),
                io::ErrorKind::ConnectionReset
                    | io::ErrorKind::ConnectionAborted
                    | io::ErrorKind::BrokenPipe
            )
        })
} // RevExt: 218
