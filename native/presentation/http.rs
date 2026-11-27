//! Local HTTP transport, browser policy, and protocol adapters.
mod openai;
use crate::{
    application::{
        failure::{Failure, Outcome},
        inference::{self, InferenceBackend, InferenceService, exposed},
    },
    domain::State,
};
use axum::{
    Json, Router,
    body::to_bytes,
    extract::Request,
    extract::State as ExtractState,
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
const BODY_LIMIT: usize = 36 * 1024 * 1024;
const REQUEST_BODY_TIMEOUT: Duration = Duration::from_secs(30);
pub fn router<B: InferenceBackend>(backend: B, shutdown: CancellationToken) -> Router {
    Router::new()
        .fallback(dispatch::<B>)
        .with_state(Arc::new(Api(InferenceService::new(backend, shutdown))))
}
struct Api<B>(InferenceService<B>);
async fn dispatch<B: InferenceBackend>(
    ExtractState(api): ExtractState<Arc<Api<B>>>,
    request: Request,
) -> Response {
    let protocol = api.request(request).await;
    let (mut response, origin, id) = match protocol {
        Ok(result) => result,
        Err((failure, origin, id)) => (failure_response(failure), origin, id),
    };
    let headers = response.headers_mut();
    headers
        .entry(header::CACHE_CONTROL)
        .or_insert(HeaderValue::from_static("no-store"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    if let Some(origin) = origin {
        headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
        headers.insert(header::VARY, HeaderValue::from_static("Origin"));
        headers.insert(
            header::ACCESS_CONTROL_EXPOSE_HEADERS,
            HeaderValue::from_static("x-request-id"),
        );
    }
    if let Some(id) = id {
        headers.insert(
            "x-request-id",
            HeaderValue::from_str(&id.to_string()).expect("UUID is a valid header"),
        );
    }
    response
}
type ProtocolResult = Result<
    (Response, Option<HeaderValue>, Option<Uuid>),
    (Failure, Option<HeaderValue>, Option<Uuid>),
>;
impl<B: InferenceBackend> Api<B> {
    async fn request(&self, request: Request) -> ProtocolResult {
        let headers = request.headers();
        let state = self.0.snapshot().map_err(|error| (error, None, None))?;
        let bridge = state.network.container_bridge_address().ok().flatten();
        let allowed = [
            Some(state.network.loopback_authority()),
            Some(format!("localhost:{}", state.network.port)),
            bridge.map(|address| address.to_string()),
            bridge.map(|address| format!("host.docker.internal:{}", address.port())),
        ];
        if headers.get(header::HOST).is_some_and(|host| {
            !allowed
                .iter()
                .flatten()
                .any(|value| host.as_bytes() == value.as_bytes())
        }) {
            return Err((
                Failure::new(
                    403,
                    "HOST_REJECTED",
                    "Use the configured local Router endpoint.",
                ),
                None,
                None,
            ));
        }
        let origin = headers.get(header::ORIGIN).cloned();
        if origin.as_ref().is_some_and(|origin| {
            !state
                .network
                .browser_origins
                .iter()
                .any(|allowed| origin.as_bytes() == allowed.as_bytes())
        }) {
            return Err((
                Failure::new(
                    403,
                    "BROWSER_ORIGIN_REJECTED",
                    "Approve this website origin in Provider Router API access settings.",
                ),
                None,
                None,
            ));
        }
        let mut id = None;
        match self.handle(request, &state, &mut id).await {
            Ok(response) => Ok((response, origin, id)),
            Err(failure) => Err((failure, origin, id)),
        }
    }
    async fn handle(
        &self,
        request: Request,
        state: &State,
        request_id: &mut Option<Uuid>,
    ) -> Outcome<Response> {
        let path = request.uri().path();
        let headers = request.headers();
        let health = || {
            json!({"name":"provider-router","protocol_version":1,"instance_id":state.instance_id,
            "version":env!("CARGO_PKG_VERSION"),"authentication_required":state.network.require_api_key})
        };
        if request.method() == Method::OPTIONS {
            return preflight(headers);
        }
        if request.method() == Method::GET && path == "/health" {
            return Ok(Json(health()).into_response());
        }
        let token = headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "));
        let principal = inference::authenticate(state, token)?;
        if request.method() == Method::GET {
            return match path {
                "/v1/connectivity" => {
                    let mut info = health();
                    let connections = exposed(state).count();
                    let models: usize = exposed(state)
                        .map(|connection| connection.models.len())
                        .sum();
                    info["ready"] = json!(models > 0);
                    info["providers"] = json!(connections);
                    info["models"] = json!(models);
                    Ok(Json(info).into_response())
                }
                "/v1/models" => Ok(Json(models(state)).into_response()),
                "/v1/providers" => {
                    let providers: Vec<_> = exposed(state).map(|connection| json!({"id":connection.id,"name":connection.name,
                        "provider":connection.method.provider(),"method":connection.method,"models":connection.models})).collect();
                    Ok(Json(json!({"protocol_version":1,"providers":providers})).into_response())
                }
                _ => Err(Failure::not_found()),
            };
        }
        if request.method() != Method::POST {
            return Err(Failure::not_found());
        }
        if let Some(id) = path
            .strip_prefix("/v1/inference/")
            .and_then(|value| value.strip_suffix("/cancel"))
        {
            let id = Uuid::parse_str(id).map_err(|_| Failure::request_not_found())?;
            self.0.cancel(id, principal)?;
            return Ok(Json(json!({"cancelled":true})).into_response());
        }
        let surface = match path {
            "/v1/chat/completions" | "/v1/attachment-checks" => Some(openai::Surface::Chat),
            "/v1/responses" => Some(openai::Surface::Responses),
            "/v1/inference" => None,
            _ => return Err(Failure::not_found()),
        };
        let check_attachments = path == "/v1/attachment-checks";
        let correlation = headers
            .get("x-request-id")
            .and_then(|header| header.to_str().ok())
            .and_then(|header| Uuid::parse_str(header).ok())
            .filter(|id| id.get_version_num() == 7);
        if !headers
            .get(header::CONTENT_TYPE)
            .and_then(|header| header.to_str().ok())
            .is_some_and(|mime| {
                mime.split(';')
                    .next()
                    .is_some_and(|value| value.trim().eq_ignore_ascii_case("application/json"))
            })
        {
            return Err(Failure::new(
                415,
                "CONTENT_TYPE_REQUIRED",
                "Use application/json for inference requests.",
            ));
        }
        let bytes = tokio::time::timeout(
            REQUEST_BODY_TIMEOUT,
            to_bytes(request.into_body(), BODY_LIMIT),
        )
        .await
        .map_err(|_| {
            Failure::new(
                408,
                "REQUEST_TIMEOUT",
                "The request body took too long to read.",
            )
        })?
        .map_err(|error| {
            use std::error::Error;
            if error
                .source()
                .is_some_and(|source| source.is::<http_body_util::LengthLimitError>())
            {
                Failure::new(
                    413,
                    "REQUEST_TOO_LARGE",
                    "The request exceeds the supported size.",
                )
            } else {
                Failure::bad("INVALID_JSON", "The request body could not be read.")
            }
        })?;
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| {
            Failure::bad("INVALID_JSON", "The request body must contain valid JSON.")
        })?;
        let (inference, openai) = match surface {
            Some(surface) => {
                let openai = openai::decode(
                    surface,
                    value,
                    &state.connections,
                    correlation.unwrap_or_else(Uuid::now_v7),
                )?;
                (openai.request.clone(), Some(openai))
            }
            None => (
                serde_json::from_value(value).map_err(|_| Failure::invalid())?,
                None,
            ),
        };
        *request_id = Some(inference.request_id);
        let completion = self
            .0
            .infer(state, principal, inference, check_attachments)
            .await?;
        Ok(match openai {
            Some(openai) => openai::response(&openai, &completion),
            None => Json(completion).into_response(),
        })
    }
}
fn models(state: &State) -> Value {
    let rows: Vec<_> = exposed(state).flat_map(|connection| connection.models.iter().map(|model| {
        let provider = connection.method.provider();
        json!({"id":format!("{}/{}", connection.id, model.id),"object":"model","created":0,
            "owned_by":if provider == "codex" {"openai"} else {provider},
            "provider_router":{"connection_id":connection.id,"connection_name":connection.name,"provider":provider,
                "model":model.id,"method":connection.method,"name":model.name,"reasoning_efforts":model.reasoning_efforts,
                "default_reasoning_effort":model.default_reasoning_effort,"context_window":model.context_window,
                "max_output_tokens":model.max_output_tokens,"tools":model.tools,"structured_output":model.structured_output,
                "input_modalities":model.input_modalities}})
    })).collect();
    json!({"object":"list","data":rows})
}
fn preflight(headers: &HeaderMap) -> Outcome<Response> {
    let method = headers
        .get(header::ACCESS_CONTROL_REQUEST_METHOD)
        .and_then(|value| value.to_str().ok());
    let request_headers = headers
        .get(header::ACCESS_CONTROL_REQUEST_HEADERS)
        .map(|value| value.to_str().unwrap_or("invalid"));
    if !headers.contains_key(header::ORIGIN)
        || !matches!(method, Some("GET" | "POST"))
        || request_headers.is_some_and(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|header| !header.is_empty())
                .any(|name| {
                    !["authorization", "content-type", "x-request-id"]
                        .iter()
                        .any(|allowed| name.eq_ignore_ascii_case(allowed))
                })
        })
    {
        return Err(Failure::new(
            403,
            "CORS_PREFLIGHT_REJECTED",
            "This browser request method or header is not supported.",
        ));
    }
    Ok((
        StatusCode::NO_CONTENT,
        [
            (header::ACCESS_CONTROL_ALLOW_METHODS, "GET, POST"),
            (
                header::ACCESS_CONTROL_ALLOW_HEADERS,
                "Authorization, Content-Type, X-Request-ID",
            ),
        ],
    )
        .into_response())
}
fn failure_response(failure: Failure) -> Response {
    let status = StatusCode::from_u16(failure.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let kind = match failure.status {
        401 => "authentication_error",
        429 => "rate_limit_error",
        500.. => "api_error",
        _ => "invalid_request_error",
    };
    (
            status,
            Json(
                json!({"error":{"code":failure.code,"message":failure.message,"type":kind,"param":null}}),
            ),
        )
            .into_response()
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        application::contracts::{Completion, Message, Request as Inference},
        domain::{Connection, Method as ProviderMethod, Model},
    };
    use axum::body::Body;
    use tokio::sync::Notify;
    struct PendingBackend {
        state: State,
        started: Arc<Notify>,
    }
    impl InferenceBackend for PendingBackend {
        fn settings(&self) -> anyhow::Result<State> {
            Ok(self.state.clone())
        }
        async fn prepare_documents(
            &self,
            messages: Vec<Message>,
            _: &Model,
        ) -> Outcome<Vec<Message>> {
            Ok(messages)
        }
        async fn complete(&self, _: &Connection, _: &Inference) -> Outcome<Completion> {
            self.started.notify_one();
            std::future::pending().await
        }
    }
    struct WireBackend(State);

    impl InferenceBackend for WireBackend {
        fn settings(&self) -> anyhow::Result<State> {
            Ok(self.0.clone())
        }
        async fn prepare_documents(
            &self,
            messages: Vec<Message>,
            _: &Model,
        ) -> Outcome<Vec<Message>> {
            Ok(messages)
        }
        async fn complete(&self, _: &Connection, request: &Inference) -> Outcome<Completion> {
            Ok(Completion {
                request_id: request.request_id,
                content: r#"{"answer":42}"#.into(),
                tool_calls: vec![],
                usage: crate::application::contracts::Usage {
                    input_tokens: 2,
                    output_tokens: 1,
                    cached_input_tokens: None,
                    cache_write_tokens: None,
                },
                provider: "codex".into(),
                method: "codex_http".into(),
                model: request.model.clone(),
                reasoning_effort: request.reasoning_effort.clone(),
                continuation: None,
            })
        }
    }

    #[tokio::test]
    async fn public_request_decoders_share_wire_parity_fixtures() -> anyhow::Result<()> {
        let mut connection = Connection::new(ProviderMethod::Codex);
        connection.exposed = true;
        connection.models.push(Model {
            id: "test-model".into(),
            name: "Test model".into(),
            reasoning_efforts: vec!["low".into()],
            default_reasoning_effort: Some("low".into()),
            context_window: 32_000,
            max_output_tokens: 4_000,
            tools: true,
            structured_output: true,
            input_modalities: vec!["text".into()],
        });
        let id = connection.id;
        let api = Arc::new(Api(InferenceService::new(
            WireBackend(State {
                connections: vec![connection],
                ..State::default()
            }),
            CancellationToken::new(),
        )));
        let fixtures: Value =
            serde_json::from_str(include_str!("../../tests/fixtures/router-wire-parity.json"))?;
        for fixture in fixtures["requests"].as_array().unwrap() {
            let surface = fixture["surface"].as_str().unwrap();
            let source = fixture["source"]
                .as_str()
                .unwrap()
                .replace("$REQUEST_ID", &Uuid::now_v7().to_string())
                .replace("$CONNECTION_ID", &id.to_string())
                .replace("$SIMPLE_CONNECTION_ID", &id.simple().to_string());
            let bytes = match source.split_once("$INVALID_UTF8") {
                Some((before, after)) => [before.as_bytes(), &[0xff], after.as_bytes()].concat(),
                None => source.into_bytes(),
            };
            let request = axum::http::Request::post(format!("/v1/{surface}"))
                .header(
                    header::CONTENT_TYPE,
                    fixture["contentType"]
                        .as_str()
                        .unwrap_or("application/json"),
                )
                .body(Body::from(bytes))?;
            let response = dispatch(ExtractState(api.clone()), request).await;
            let successful = response.status().is_success();
            let body: Value = serde_json::from_slice(&to_bytes(response.into_body(), 4096).await?)?;
            assert_eq!(
                body["error"]["code"], fixture["code"],
                "{surface}: {}",
                fixture["label"]
            );
            assert_eq!(
                successful,
                fixture["code"].is_null(),
                "{surface}: {}",
                fixture["label"]
            );
            if fixture["tools"].is_array() {
                assert_eq!(body["tools"], fixture["tools"], "{}", fixture["label"]);
            }
        }
        Ok(())
    }

    #[tokio::test(start_paused = true)]
    async fn deadline_and_client_cancel_have_distinct_http_errors() -> anyhow::Result<()> {
        let mut connection = Connection::new(ProviderMethod::Codex);
        connection.exposed = true;
        connection.models.push(Model {
            id: "test-model".into(),
            name: "Test model".into(),
            reasoning_efforts: vec![],
            default_reasoning_effort: None,
            context_window: 128_000,
            max_output_tokens: 4_000,
            tools: false,
            structured_output: false,
            input_modalities: vec!["text".into()],
        });
        let started = Arc::new(Notify::new());
        let backend = PendingBackend {
            state: State {
                connections: vec![connection],
                ..State::default()
            },
            started: started.clone(),
        };
        let api = Arc::new(Api(InferenceService::new(
            backend,
            CancellationToken::new(),
        )));
        let infer = |id: Uuid| {
            axum::http::Request::post("/v1/chat/completions")
                .header(header::CONTENT_TYPE, "application/json")
                .header("x-request-id", id.to_string())
                .body(Body::from(
                    json!({"model":"test-model","messages":[{"role":"user","content":"Hello"}]})
                        .to_string(),
                ))
        };
        let id = Uuid::now_v7();
        let cancel =
            axum::http::Request::post(format!("/v1/inference/{id}/cancel")).body(Body::empty())?;
        let (cancelled, accepted) =
            tokio::join!(dispatch(ExtractState(api.clone()), infer(id)?), async {
                tokio::time::timeout(Duration::from_secs(1), async {
                    started.notified().await;
                    dispatch(ExtractState(api.clone()), cancel).await
                })
                .await
            });
        assert_eq!(accepted?.status(), StatusCode::OK);
        let timed_out = dispatch(ExtractState(api), infer(Uuid::now_v7())?).await;
        for (response, status, code) in [
            (cancelled, 499, "CANCELLED"),
            (timed_out, 504, "ROUTER_TIMEOUT"),
        ] {
            assert_eq!(response.status(), status);
            let body: Value = serde_json::from_slice(&to_bytes(response.into_body(), 4096).await?)?;
            assert_eq!(body["error"]["code"], code);
        }
        Ok(())
    }
}
