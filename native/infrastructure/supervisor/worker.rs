//! Manage the private API processes and stream HTTP to the selected generation.
// RevExt: 1
use std::{
    net::{Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex, RwLock},
    time::{Duration, Instant},
};  // RevExt: 113
// RevExt: 2
use anyhow::{Context, Result, ensure};
use axum::{
    Router,
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header},
    response::{IntoResponse, Response},
};  // RevExt: 114
use hyper_util::{
    client::legacy::{Client, connect::HttpConnector},
    rt::TokioExecutor,
};  // RevExt: 115
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
    process::{Child, ChildStdin, Command},
    time,
};  // RevExt: 116
use tokio_util::sync::CancellationToken;
// RevExt: 3
use super::socket_transfer;
use crate::infrastructure::profile::{Profile, random_token};  // RevExt: 117
// RevExt: 4
pub const WORKER_AUTH: &str = "x-provider-router-worker";
const WORKER_PROTOCOL: u32 = 1;
const START_TIMEOUT: Duration = Duration::from_secs(12);
// RevExt: 5
#[derive(Deserialize, Serialize)]  // RevExt: 29
struct Ready {
    protocol: u32,
    version: String,
    port: u16,  // RevExt: 31
}  // RevExt: 33
// RevExt: 6
#[derive(Deserialize, Serialize)]  // RevExt: 30
struct Start {
    token: String,
    listener: socket_transfer::SocketOffer,
}  // RevExt: 34
// RevExt: 7
pub struct Worker {
    pub executable: PathBuf,
    pub version: String,
    address: SocketAddr,
    token: HeaderValue,  // RevExt: 47
    process: Mutex<Child>,
    _lifetime: ChildStdin,
    _listener: std::net::TcpListener,
    _pin: Option<std::fs::File>,
}  // RevExt: 35
// RevExt: 8
impl Worker {
    pub async fn start(
        profile: &Profile,
        executable: &Path,
        version: &str,  // RevExt: 118
        pin: Option<std::fs::File>,
    ) -> Result<Arc<Self>> {
        let secret = random_token();
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;  // RevExt: 93
        let address = listener.local_addr()?;  // RevExt: 95
        let mut command = Command::new(executable);
        command
            .arg("--data-dir")
            .arg(&profile.dir)
            .arg("__worker")
            .stdin(Stdio::piped())  // RevExt: 97
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
            .take()  // RevExt: 120
            .context("Worker stdin is unavailable.")?;
        let stdout = process
            .stdout
            .take()  // RevExt: 121
            .context("Worker stdout is unavailable.")?;
        let start = Start {
            token: secret.clone(),
            listener: socket_transfer::offer(&listener, &process)?,
        };  // RevExt: 122
        let mut response = Vec::new();
        time::timeout(START_TIMEOUT, async {
            stdin
                .write_all(&serde_json::to_vec(&start).map_err(std::io::Error::other)?)
                .await?;
            stdin.write_all(b"\n").await?;
            BufReader::new(stdout)
                .take(4096)
                .read_until(b'\n', &mut response)
                .await  // RevExt: 124
        })
        .await
        .context("The API worker did not become ready.")??;
        let ready: Ready = serde_json::from_slice(&response)
            .context("The API worker failed its readiness check.")?;
        ensure!(  // RevExt: 172
  // RevExt: 126
            ready.protocol == WORKER_PROTOCOL
                && ready.version == version
                && ready.port == address.port(),
            "The API worker reported an unexpected version or protocol."
        );  // RevExt: 128
        let client = reqwest::Client::builder()  // RevExt: 133
            .no_proxy()  // RevExt: 135
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(2))
            .build()?;  // RevExt: 137
        let health: serde_json::Value = client
            .get(format!("http://{address}/health"))
            .header(header::HOST, profile.read()?.network.loopback_authority())
            .header(WORKER_AUTH, &secret)
            .send()  // RevExt: 140
            .await?
            .error_for_status()?
            .json()
            .await?;  // RevExt: 142
        ensure!(  // RevExt: 173
  // RevExt: 127
            health["version"] == version
                && health["instance_id"] == profile.read()?.instance_id.to_string(),
            "The API worker did not serve the expected profile."
        );  // RevExt: 129
        Ok(Arc::new(Self {
            executable: executable.into(),
            version: version.into(),  // RevExt: 144
            address,  // RevExt: 146
            token: HeaderValue::from_str(&secret)?,
            process: Mutex::new(process),  // RevExt: 99
            _lifetime: stdin,
            _listener: listener,  // RevExt: 101
            _pin: pin,
        }))
    }  // RevExt: 49
// RevExt: 9
    pub fn running(&self) -> bool {
        self.process
            .lock()
            .expect("worker process lock")
            .try_wait()
            .is_ok_and(|status| status.is_none())
    }  // RevExt: 50
}  // RevExt: 36
// RevExt: 11
pub async fn serve(routes: impl FnOnce(CancellationToken) -> Router) -> Result<()> {
    let mut stdin = BufReader::new(tokio::io::stdin());
    let mut message = Vec::new();
    (&mut stdin)
        .take(4096)
        .read_until(b'\n', &mut message)
        .await?;  // RevExt: 151
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
                }  // RevExt: 64
            }  // RevExt: 67
        },
    ));
    let ready = Ready {
        protocol: WORKER_PROTOCOL,
        version: env!("CARGO_PKG_VERSION").into(),
        port: listener.local_addr()?.port(),
    };  // RevExt: 149
    let mut stdout = tokio::io::stdout();
    stdout
        .write_all(format!("{}\n", serde_json::to_string(&ready)?).as_bytes())
        .await?;  // RevExt: 152
    stdout.flush().await?;
    let mut byte = [0];
    tokio::select! {
        result = axum::serve(listener, routes).with_graceful_shutdown(shutdown.cancelled_owned()) => result?,
        // The private pipe closes when the supervisor exits, including on an unclean exit.
        _ = stdin.read(&mut byte) => {},
    }  // RevExt: 51
    Ok(())
}  // RevExt: 38
// RevExt: 12
#[derive(Clone)]  // RevExt: 71
pub struct Proxy {
    client: Client<HttpConnector, Body>,
    routing: Arc<RwLock<Routing>>,
    predecessor: Arc<RwLock<Option<Predecessor>>>,
}  // RevExt: 39
// RevExt: 13
#[derive(Clone)]  // RevExt: 72
struct Predecessor {
    port: u16,  // RevExt: 32
    token: HeaderValue,  // RevExt: 48
    _listener: Arc<std::net::TcpListener>,
    until: Instant,
}  // RevExt: 40
// RevExt: 14
struct Routing {
    active: Arc<Worker>,
    retired: Vec<Arc<Worker>>,
}  // RevExt: 41
// RevExt: 15
impl Proxy {  // RevExt: 174
  // RevExt: 153
  // RevExt: 73
    pub fn new(worker: Arc<Worker>) -> Self {
        let client = Client::builder(TokioExecutor::new()).build_http();
        Self {
            client,
            routing: Arc::new(RwLock::new(Routing {
                active: worker,
                retired: Vec::new(),
            })),
            predecessor: Arc::new(RwLock::new(None)),
        }  // RevExt: 75
    }  // RevExt: 52
// RevExt: 16
    pub fn routes(&self) -> Router {
        Router::new().fallback(forward).with_state(self.clone())
    }  // RevExt: 53
// RevExt: 17
    pub fn active(&self) -> Arc<Worker> {
        self.routing.read().expect("routing lock").active.clone()
    }  // RevExt: 54
// RevExt: 18
    pub fn switch(&self, worker: Arc<Worker>) {
        let mut routing = self.routing.write().expect("routing lock");
        let previous = std::mem::replace(&mut routing.active, worker);
        routing.retired.push(previous);
    }  // RevExt: 55
// RevExt: 19
    pub fn prune(&self) {
        let mut predecessor = self.predecessor.write().expect("predecessor lock");
        if predecessor
            .as_ref()
            .is_some_and(|peer| peer.until <= Instant::now())
        {  // RevExt: 176
  // RevExt: 155
            predecessor.take();
        }  // RevExt: 85
        self.routing
            .write()
            .expect("routing lock")
            .retired
            .retain(|worker| Arc::strong_count(worker) > 1);
    }  // RevExt: 56
// RevExt: 20
    pub fn predecessor(&self, listener: std::net::TcpListener, token: &str) -> Result<()> {
        *self.predecessor.write().expect("predecessor lock") = Some(Predecessor {
            port: listener.local_addr()?.port(),
            token: HeaderValue::from_str(token)?,
            _listener: Arc::new(listener),
            until: Instant::now() + Duration::from_secs(660),
        });  // RevExt: 107
        Ok(())  // RevExt: 111
    }  // RevExt: 84
// RevExt: 83
    pub fn live_paths(&self) -> Vec<PathBuf> {
        let routing = self.routing.read().expect("routing lock");
        std::iter::once(&routing.active)
            .chain(&routing.retired)
            .map(|worker| worker.executable.clone())
            .collect()
    }  // RevExt: 59
// RevExt: 23
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
            )  // RevExt: 158
            .build()?;  // RevExt: 138
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
            }  // RevExt: 68
        };  // RevExt: 123
        Ok(Response::from_parts(parts, Body::from_stream(stream)))
    }  // RevExt: 60
}  // RevExt: 42
// RevExt: 24
async fn forward(State(proxy): State<Proxy>, request: Request) -> Response {
    proxy.forward(request).await
}  // RevExt: 43
// RevExt: 25
impl Proxy {  // RevExt: 175
  // RevExt: 154
  // RevExt: 74
    pub async fn forward(&self, request: Request) -> Response {
        let path = request.uri().path();
        if request.method() == Method::POST
            && path
                .strip_prefix("/v1/inference/")
                .is_some_and(|tail| tail.ends_with("/cancel"))
        {  // RevExt: 177
  // RevExt: 156
  // RevExt: 79
            let predecessor = self.predecessor.read().expect("predecessor lock").clone();
            if let Some(peer) = predecessor {
                let address = SocketAddr::from((Ipv4Addr::LOCALHOST, peer.port));
                let response = time::timeout(
                    Duration::from_millis(500),
                    self.send(address, peer.token, None, cancel_copy(&request)),
                )
                .await;
                if let Ok(Ok(response)) = response
                    && response.status() != StatusCode::NOT_FOUND  // RevExt: 179
  // RevExt: 160
                {  // RevExt: 181
  // RevExt: 162
                    return response;  // RevExt: 81
                }  // RevExt: 65
            }  // RevExt: 69
            let retired = self.routing.read().expect("routing lock").retired.clone();
            for worker in retired.into_iter().filter(|worker| worker.running()) {
                if let Ok(response) = self
                    .send(
                        worker.address,
                        worker.token.clone(),
                        Some(worker),
                        cancel_copy(&request),
                    )  // RevExt: 164
                    .await
                    && response.status() != StatusCode::NOT_FOUND  // RevExt: 180
  // RevExt: 161
                {  // RevExt: 182
  // RevExt: 163
                    return response;  // RevExt: 82
                }  // RevExt: 66
            }  // RevExt: 70
        }  // RevExt: 76
// RevExt: 26
        let worker = self.active();
        if worker.running()
            && let Ok(response) = self
                .send(worker.address, worker.token.clone(), Some(worker), request)
                .await  // RevExt: 171
  // RevExt: 125
        {  // RevExt: 178
  // RevExt: 157
  // RevExt: 80
            return response;
        }  // RevExt: 77
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
    }  // RevExt: 61
}  // RevExt: 44
// RevExt: 27
fn cancel_copy(request: &Request) -> Request {
    let mut cancel = Request::new(Body::empty());
    *cancel.method_mut() = Method::POST;
    *cancel.uri_mut() = request.uri().clone();
    *cancel.headers_mut() = request.headers().clone();
    cancel.headers_mut().remove(header::CONTENT_LENGTH);
    cancel
}  // RevExt: 45
// RevExt: 28
fn strip_hop_headers(headers: &mut HeaderMap) {
    let connection = headers.remove(header::CONNECTION);
    if let Some(connection) = connection.as_ref().and_then(|value| value.to_str().ok()) {
        for name in connection.split(',') {
            headers.remove(name.trim());
        }  // RevExt: 78
    }  // RevExt: 62
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
    }  // RevExt: 63
}  // RevExt: 46
// RevExt: 86
#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Bytes,
        routing::{get, post},
    };  // RevExt: 150
    use tokio::sync::Notify;
// RevExt: 87
    #[test]
    #[ignore = "Launched as a child by the streaming cutover test"]
    fn fixture_lifetime() {
        if std::env::var_os("PROVIDER_ROUTER_STREAM_FIXTURE").is_some() {
            std::io::copy(&mut std::io::stdin(), &mut std::io::sink()).unwrap();
        }  // RevExt: 106
    }  // RevExt: 103
// RevExt: 88
    async fn fixture(
        version: &str,  // RevExt: 119
        release: Arc<Notify>,
    ) -> Result<(Arc<Worker>, tokio::task::JoinHandle<()>)> {
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;  // RevExt: 94
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;  // RevExt: 96
        let public_listener = TcpListener::from_std(listener.try_clone()?)?;
        let executable = std::env::current_exe()?;
        let mut process = Command::new(&executable)
            .args([
                "--exact",
                "infrastructure::supervisor::worker::tests::fixture_lifetime",
                "--ignored",
            ])
            .env("PROVIDER_ROUTER_STREAM_FIXTURE", "1")
            .stdin(Stdio::piped())  // RevExt: 98
            .stdout(Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        let lifetime = process.stdin.take().context("Fixture stdin is required.")?;
        let worker = Arc::new(Worker {
            executable,
            version: version.into(),  // RevExt: 145
            address,  // RevExt: 147
            token: HeaderValue::from_static("fixture-token"),
            process: Mutex::new(process),  // RevExt: 100
            _lifetime: lifetime,
            _listener: listener,  // RevExt: 102
            _pin: None,
        });  // RevExt: 108
        let version = version.to_owned();
        let cancel = version.clone();
        let routes = Router::new()
            .route(  // RevExt: 183
  // RevExt: 166
                "/v1/inference/{id}/cancel",
                post(move || async move { cancel }),
            )  // RevExt: 159
            .route(  // RevExt: 184
  // RevExt: 167
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
                    )  // RevExt: 165
                }),
            );
        let task = tokio::spawn(async move {
            axum::serve(public_listener, routes).await.unwrap();
        });  // RevExt: 109
        Ok((worker, task))
    }  // RevExt: 104
// RevExt: 89
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
        });  // RevExt: 110
        let client = reqwest::Client::builder()  // RevExt: 134
            .no_proxy()  // RevExt: 136
            .timeout(Duration::from_secs(5))
            .build()?;  // RevExt: 139
        let mut old = client.get(format!("{endpoint}/stream")).send().await?;
        assert_eq!(  // RevExt: 185
  // RevExt: 168
            old.chunk().await?.context("Old stream is empty.")?,
            "data: old\n\n"
        );  // RevExt: 130
// RevExt: 90
        proxy.switch(second);
        proxy.prune();
        let mut new = client.get(format!("{endpoint}/stream")).send().await?;
        assert_eq!(  // RevExt: 186
  // RevExt: 169
            new.chunk().await?.context("New stream is empty.")?,
            "data: new\n\n"
        );  // RevExt: 131
        let cancellation = client
            .post(format!("{endpoint}/v1/inference/test/cancel"))
            .send()  // RevExt: 141
            .await?;  // RevExt: 143
        assert_eq!(cancellation.text().await?, "old");
// RevExt: 91
        release.notify_one();
        assert_eq!(  // RevExt: 187
  // RevExt: 170
            old.chunk().await?.context("Old stream was interrupted.")?,
            "data: [DONE]\n\n"
        );  // RevExt: 132
        assert!(old.chunk().await?.is_none());
        server.abort();
        new_server.abort();
        old_server.abort();
        Ok(())  // RevExt: 112
    }  // RevExt: 105
}  // RevExt: 92
