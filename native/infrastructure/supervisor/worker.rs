//! Manage the private API processes and stream HTTP to the selected generation.
use super::socket_transfer;
use crate::infrastructure::profile::{Profile, random_token};
use anyhow::{Context, Result, ensure};
use axum::{
    Router,
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header},
    response::{IntoResponse, Response},
};
use hyper_util::{
    client::legacy::{Client, connect::HttpConnector},
    rt::TokioExecutor,
};
use serde::{Deserialize, Serialize};
use std::{
    net::{Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex, RwLock},
    time::{Duration, Instant},
};
use subtle::ConstantTimeEq;
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
    process::{Child, ChildStdin, Command},
    time,
};
use tokio_util::sync::CancellationToken;
pub const WORKER_AUTH: &str = "x-provider-router-worker";
const WORKER_PROTOCOL: u32 = 1;
const START_TIMEOUT: Duration = Duration::from_secs(12);
#[derive(Deserialize, Serialize)]
struct Ready {
    protocol: u32,
    version: String,
    port: u16,
}
#[derive(Deserialize, Serialize)]
struct Start {
    token: String,
    listener: socket_transfer::SocketOffer,
}
pub struct Worker {
    pub executable: PathBuf,
    pub version: String,
    address: SocketAddr,
    token: HeaderValue,
    process: Mutex<Child>,
    _lifetime: ChildStdin,
    _listener: std::net::TcpListener,
    _pin: Option<std::fs::File>,
}
impl Worker {
    pub async fn start(
        profile: &Profile,
        executable: &Path,
        version: &str,
        pin: Option<std::fs::File>,
    ) -> Result<Arc<Self>> {
        let secret = random_token();
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let address = listener.local_addr()?;
        let mut command = Command::new(executable);
        command
            .arg("--data-dir")
            .arg(&profile.dir)
            .arg("__worker")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
        socket_transfer::prepare(&mut command, &[&listener]);
        let mut process = command
            .spawn()
            .context("Could not launch the API worker.")?;
        let mut stdin = process
            .stdin
            .take()
            .context("Worker stdin is unavailable.")?;
        let stdout = process
            .stdout
            .take()
            .context("Worker stdout is unavailable.")?;
        let start = Start {
            token: secret.clone(),
            listener: socket_transfer::offer(&listener, &process)?,
        };
        let mut response = Vec::new();
        time::timeout(START_TIMEOUT, async {
            stdin
                .write_all(&serde_json::to_vec(&start).map_err(std::io::Error::other)?)
                .await?;
            stdin.write_all(b"\n").await?;
            BufReader::new(stdout)
                .take(4096)
                .read_until(b'\n', &mut response)
                .await
        })
        .await
        .context("The API worker did not become ready.")??;
        let ready: Ready = serde_json::from_slice(&response)
            .context("The API worker failed its readiness check.")?;
        ensure!(
            ready.protocol == WORKER_PROTOCOL
                && ready.version == version
                && ready.port == address.port(),
            "The API worker reported an unexpected version or protocol."
        );
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(2))
            .build()?;
        let health: serde_json::Value = client
            .get(format!("http://{address}/health"))
            .header(header::HOST, profile.read()?.network.loopback_authority())
            .header(WORKER_AUTH, &secret)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        ensure!(
            health["version"] == version
                && health["instance_id"] == profile.read()?.instance_id.to_string(),
            "The API worker did not serve the expected profile."
        );
        Ok(Arc::new(Self {
            executable: executable.into(),
            version: version.into(),
            address,
            token: HeaderValue::from_str(&secret)?,
            process: Mutex::new(process),
            _lifetime: stdin,
            _listener: listener,
            _pin: pin,
        }))
    }
    pub fn running(&self) -> bool {
        self.process
            .lock()
            .expect("worker process lock")
            .try_wait()
            .is_ok_and(|status| status.is_none())
    }
}
pub async fn serve(routes: impl FnOnce(CancellationToken) -> Router) -> Result<()> {
    let mut stdin = BufReader::new(tokio::io::stdin());
    let mut message = Vec::new();
    (&mut stdin)
        .take(4096)
        .read_until(b'\n', &mut message)
        .await?;
    let start: Start = serde_json::from_slice(&message)?;
    let token = start.token.into_bytes();
    ensure!(token.len() == 43, "A supervisor token is required.");
    let listener = TcpListener::from_std(socket_transfer::receive(start.listener)?)?;
    let shutdown = CancellationToken::new();
    let routes = routes(shutdown.clone()).layer(axum::middleware::from_fn(
        move |mut request: Request, next: axum::middleware::Next| {
            let authorized: bool = request
                .headers_mut()
                .remove(WORKER_AUTH)
                .is_some_and(|value| value.as_bytes().ct_eq(&token).into());
            async move {
                if authorized {
                    next.run(request).await
                } else {
                    StatusCode::FORBIDDEN.into_response()
                }
            }
        },
    ));
    let ready = Ready {
        protocol: WORKER_PROTOCOL,
        version: env!("CARGO_PKG_VERSION").into(),
        port: listener.local_addr()?.port(),
    };
    let mut stdout = tokio::io::stdout();
    stdout
        .write_all(format!("{}\n", serde_json::to_string(&ready)?).as_bytes())
        .await?;
    stdout.flush().await?;
    let mut byte = [0];
    tokio::select! {
        result = axum::serve(listener, routes).with_graceful_shutdown(shutdown.cancelled_owned()) => result?,
        // The private pipe closes when the supervisor exits, including on an unclean exit.
        _ = stdin.read(&mut byte) => {},
    }
    Ok(())
}
#[derive(Clone)]
pub struct Proxy {
    client: Client<HttpConnector, Body>,
    routing: Arc<RwLock<Routing>>,
    predecessor: Arc<RwLock<Option<Predecessor>>>,
}
#[derive(Clone)]
struct Predecessor {
    port: u16,
    token: HeaderValue,
    _listener: Arc<std::net::TcpListener>,
    until: Instant,
}
struct Routing {
    active: Arc<Worker>,
    retired: Vec<Arc<Worker>>,
}
impl Proxy {
    pub fn new(worker: Arc<Worker>) -> Self {
        let client = Client::builder(TokioExecutor::new()).build_http();
        Self {
            client,
            routing: Arc::new(RwLock::new(Routing {
                active: worker,
                retired: Vec::new(),
            })),
            predecessor: Arc::new(RwLock::new(None)),
        }
    }
    pub fn routes(&self) -> Router {
        Router::new().fallback(forward).with_state(self.clone())
    }
    pub fn active(&self) -> Arc<Worker> {
        self.routing.read().expect("routing lock").active.clone()
    }
    pub fn switch(&self, worker: Arc<Worker>) {
        let mut routing = self.routing.write().expect("routing lock");
        let previous = std::mem::replace(&mut routing.active, worker);
        routing.retired.push(previous);
    }
    pub fn prune(&self) {
        let mut predecessor = self.predecessor.write().expect("predecessor lock");
        if predecessor
            .as_ref()
            .is_some_and(|peer| peer.until <= Instant::now())
        {
            predecessor.take();
        }
        self.routing
            .write()
            .expect("routing lock")
            .retired
            .retain(|worker| Arc::strong_count(worker) > 1);
    }
    pub fn predecessor(&self, listener: std::net::TcpListener, token: &str) -> Result<()> {
        *self.predecessor.write().expect("predecessor lock") = Some(Predecessor {
            port: listener.local_addr()?.port(),
            token: HeaderValue::from_str(token)?,
            _listener: Arc::new(listener),
            until: Instant::now() + Duration::from_secs(660),
        });
        Ok(())
    }
    pub fn live_paths(&self) -> Vec<PathBuf> {
        let routing = self.routing.read().expect("routing lock");
        std::iter::once(&routing.active)
            .chain(&routing.retired)
            .map(|worker| worker.executable.clone())
            .collect()
    }
    async fn send(
        &self,
        address: SocketAddr,
        token: HeaderValue,
        worker: Option<Arc<Worker>>,
        mut request: Request,
    ) -> Result<Response> {
        let authority = address.to_string();
        *request.uri_mut() = Uri::builder()
            .scheme("http")
            .authority(authority)
            .path_and_query(
                request
                    .uri()
                    .path_and_query()
                    .context("Invalid request target.")?
                    .clone(),
            )
            .build()?;
        strip_hop_headers(request.headers_mut());
        request.headers_mut().insert(WORKER_AUTH, token);
        let response = self.client.request(request).await?;
        let (mut parts, body) = response.into_parts();
        strip_hop_headers(&mut parts.headers);
        // The lease keeps this worker alive until the downstream body finishes or is dropped.
        let stream = async_stream::stream! {
            use http_body_util::BodyExt;
            let _worker = worker;
            let mut body = body.into_data_stream();
            while let Some(item) = futures_util::StreamExt::next(&mut body).await {
                yield item;
            }
        };
        Ok(Response::from_parts(parts, Body::from_stream(stream)))
    }
}
async fn forward(State(proxy): State<Proxy>, request: Request) -> Response {
    proxy.forward(request).await
}
impl Proxy {
    pub async fn forward(&self, request: Request) -> Response {
        let path = request.uri().path();
        if request.method() == Method::POST
            && path
                .strip_prefix("/v1/inference/")
                .is_some_and(|tail| tail.ends_with("/cancel"))
        {
            let predecessor = self.predecessor.read().expect("predecessor lock").clone();
            if let Some(peer) = predecessor {
                let address = SocketAddr::from((Ipv4Addr::LOCALHOST, peer.port));
                let response = time::timeout(
                    Duration::from_millis(500),
                    self.send(address, peer.token, None, cancel_copy(&request)),
                )
                .await;
                if let Ok(Ok(response)) = response
                    && response.status() != StatusCode::NOT_FOUND
                {
                    return response;
                }
            }
            let retired = self.routing.read().expect("routing lock").retired.clone();
            for worker in retired.into_iter().filter(|worker| worker.running()) {
                if let Ok(response) = self
                    .send(
                        worker.address,
                        worker.token.clone(),
                        Some(worker),
                        cancel_copy(&request),
                    )
                    .await
                    && response.status() != StatusCode::NOT_FOUND
                {
                    return response;
                }
            }
        }
        let worker = self.active();
        if worker.running()
            && let Ok(response) = self
                .send(worker.address, worker.token.clone(), Some(worker), request)
                .await
        {
            return response;
        }
        (
            StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(serde_json::json!({"error": {
                "code": "WORKER_UNAVAILABLE",
                "message": "The local API worker is restarting.",
                "type": "api_error",
                "param": null
            }})),
        )
            .into_response()
    }
}
fn cancel_copy(request: &Request) -> Request {
    let mut cancel = Request::new(Body::empty());
    *cancel.method_mut() = Method::POST;
    *cancel.uri_mut() = request.uri().clone();
    *cancel.headers_mut() = request.headers().clone();
    cancel.headers_mut().remove(header::CONTENT_LENGTH);
    cancel
}
fn strip_hop_headers(headers: &mut HeaderMap) {
    let connection = headers.remove(header::CONNECTION);
    if let Some(connection) = connection.as_ref().and_then(|value| value.to_str().ok()) {
        for name in connection.split(',') {
            headers.remove(name.trim());
        }
    }
    for name in [
        "keep-alive",
        "proxy-authenticate",
        "proxy-authorization",
        "proxy-connection",
        "te",
        "trailer",
        "transfer-encoding",
        "upgrade",
    ] {
        headers.remove(name);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Bytes,
        routing::{get, post},
    };
    use tokio::sync::Notify;
    #[test]
    #[ignore = "Launched as a child by the streaming cutover test"]
    fn fixture_lifetime() {
        if std::env::var_os("PROVIDER_ROUTER_STREAM_FIXTURE").is_some() {
            std::io::copy(&mut std::io::stdin(), &mut std::io::sink()).unwrap();
        }
    }
    async fn fixture(
        version: &str,
        release: Arc<Notify>,
    ) -> Result<(Arc<Worker>, tokio::task::JoinHandle<()>)> {
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        let public_listener = TcpListener::from_std(listener.try_clone()?)?;
        let executable = std::env::current_exe()?;
        let mut process = Command::new(&executable)
            .args([
                "--exact",
                "infrastructure::supervisor::worker::tests::fixture_lifetime",
                "--ignored",
            ])
            .env("PROVIDER_ROUTER_STREAM_FIXTURE", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        let lifetime = process.stdin.take().context("Fixture stdin is required.")?;
        let worker = Arc::new(Worker {
            executable,
            version: version.into(),
            address,
            token: HeaderValue::from_static("fixture-token"),
            process: Mutex::new(process),
            _lifetime: lifetime,
            _listener: listener,
            _pin: None,
        });
        let version = version.to_owned();
        let cancel = version.clone();
        let routes = Router::new()
            .route(
                "/v1/inference/{id}/cancel",
                post(move || async move { cancel }),
            )
            .route(
                "/stream",
                get(move || async move {
                    let body = async_stream::stream! {
                        yield Ok::<_, std::io::Error>(Bytes::from(format!("data: {version}\n\n")));
                        release.notified().await;
                        yield Ok(Bytes::from_static(b"data: [DONE]\n\n"));
                    };
                    (
                        [(header::CONTENT_TYPE, "text/event-stream")],
                        Body::from_stream(body),
                    )
                }),
            );
        let task = tokio::spawn(async move {
            axum::serve(public_listener, routes).await.unwrap();
        });
        Ok((worker, task))
    }
    #[tokio::test]
    async fn cutover_preserves_an_open_event_stream_and_its_cancellation_route() -> Result<()> {
        let release = Arc::new(Notify::new());
        let (first, old_server) = fixture("old", release.clone()).await?;
        let (second, new_server) = fixture("new", Arc::new(Notify::new())).await?;
        let proxy = Proxy::new(first);
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let endpoint = format!("http://{}", listener.local_addr()?);
        let routes = proxy.routes();
        let server = tokio::spawn(async move {
            axum::serve(listener, routes).await.unwrap();
        });
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(5))
            .build()?;
        let mut old = client.get(format!("{endpoint}/stream")).send().await?;
        assert_eq!(
            old.chunk().await?.context("Old stream is empty.")?,
            "data: old\n\n"
        );
        proxy.switch(second);
        proxy.prune();
        let mut new = client.get(format!("{endpoint}/stream")).send().await?;
        assert_eq!(
            new.chunk().await?.context("New stream is empty.")?,
            "data: new\n\n"
        );
        let cancellation = client
            .post(format!("{endpoint}/v1/inference/test/cancel"))
            .send()
            .await?;
        assert_eq!(cancellation.text().await?, "old");
        release.notify_one();
        assert_eq!(
            old.chunk().await?.context("Old stream was interrupted.")?,
            "data: [DONE]\n\n"
        );
        assert!(old.chunk().await?.is_none());
        server.abort();
        new_server.abort();
        old_server.abort();
        Ok(())
    }
}
