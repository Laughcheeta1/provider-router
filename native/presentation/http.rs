//! Local HTTP transport, browser policy, and protocol adapters.
// RevExt: 3
mod openai;
// RevExt: 4
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
// RevExt: 5
const BODY_LIMIT: usize = 36 * 1024 * 1024;
const REQUEST_BODY_TIMEOUT: Duration = Duration::from_secs(30);
// RevExt: 6
pub fn router<B: InferenceBackend>(backend: B, shutdown: CancellationToken) -> Router {
    Router::new()
        .fallback(dispatch::<B>)
        .with_state(Arc::new(Api(InferenceService::new(backend, shutdown))))
}  // RevExt: 18
// RevExt: 7
struct Api<B>(InferenceService<B>);
// RevExt: 8
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
        );  // RevExt: 24
    }  // RevExt: 26
    if let Some(id) = id {
        headers.insert(
            "x-request-id",
            HeaderValue::from_str(&id.to_string()).expect("UUID is a valid header"),
        );  // RevExt: 25
    }  // RevExt: 27
    response
}  // RevExt: 19
// RevExt: 9
type ProtocolResult = Result<
    (Response, Option<HeaderValue>, Option<Uuid>),
    (Failure, Option<HeaderValue>, Option<Uuid>),
>;
// RevExt: 10
impl<B: InferenceBackend> Api<B> {
    async fn request(&self, request: Request) -> ProtocolResult {
        let headers = request.headers();
        let state = self.0.snapshot().map_err(|error| (error, None, None))?;
        let allowed = [
            state.network.loopback_authority(),
            format!("localhost:{}", state.network.port),
        ];
        if headers.get(header::HOST).is_some_and(|host| {
            !allowed
                .iter()  // RevExt: 1
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
            ));  // RevExt: 32
        }  // RevExt: 35
        let origin = headers.get(header::ORIGIN).cloned();
        if origin.as_ref().is_some_and(|origin| {
            !state
                .network
                .browser_origins
                .iter()  // RevExt: 2
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
            ));  // RevExt: 33
        }  // RevExt: 36
        let mut id = None;
        match self.handle(request, &state, &mut id).await {
            Ok(response) => Ok((response, origin, id)),
            Err(failure) => Err((failure, origin, id)),
        }  // RevExt: 37
    }  // RevExt: 28
// RevExt: 11
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
        };  // RevExt: 44
        if request.method() == Method::OPTIONS {
            return preflight(headers);
        }  // RevExt: 38
        if request.method() == Method::GET && path == "/health" {
            return Ok(Json(health()).into_response());
        }  // RevExt: 39
// RevExt: 12
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
        }  // RevExt: 40
        if request.method() != Method::POST {
            return Err(Failure::not_found());
        }  // RevExt: 41
        if let Some(id) = path
            .strip_prefix("/v1/inference/")
            .and_then(|value| value.strip_suffix("/cancel"))
        {  // RevExt: 56
  // RevExt: 48
            let id = Uuid::parse_str(id).map_err(|_| Failure::request_not_found())?;
            self.0.cancel(id, principal)?;
            return Ok(Json(json!({"cancelled":true})).into_response());
        }  // RevExt: 42
// RevExt: 13
        let surface = match path {
            "/v1/chat/completions" | "/v1/attachment-checks" => Some(openai::Surface::Chat),
            "/v1/responses" => Some(openai::Surface::Responses),
            "/v1/inference" => None,
            _ => return Err(Failure::not_found()),
        };  // RevExt: 45
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
        {  // RevExt: 57
  // RevExt: 49
            return Err(Failure::new(
                415,
                "CONTENT_TYPE_REQUIRED",
                "Use application/json for inference requests.",
            ));  // RevExt: 34
        }  // RevExt: 43
        let bytes = tokio::time::timeout(
            REQUEST_BODY_TIMEOUT,
            to_bytes(request.into_body(), BODY_LIMIT),
        )  // RevExt: 50
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
            }  // RevExt: 52
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
            }  // RevExt: 53
            None => (
                serde_json::from_value(value).map_err(|_| Failure::invalid())?,
                None,
            ),
        };  // RevExt: 46
        *request_id = Some(inference.request_id);
        let completion = self
            .0
            .infer(state, principal, inference, check_attachments)
            .await?;
        Ok(match openai {
            Some(openai) => openai::response(&openai, &completion),
            None => Json(completion).into_response(),
        })  // RevExt: 54
    }  // RevExt: 29
// RevExt: 14
}  // RevExt: 20
// RevExt: 15
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
}  // RevExt: 21
// RevExt: 16
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
        })  // RevExt: 58
  // RevExt: 55
    {
        return Err(Failure::new(
            403,
            "CORS_PREFLIGHT_REJECTED",
            "This browser request method or header is not supported.",
        ));
    }  // RevExt: 30
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
}  // RevExt: 22
// RevExt: 17
fn failure_response(failure: Failure) -> Response {
    let status = StatusCode::from_u16(failure.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let kind = match failure.status {
        401 => "authentication_error",
        429 => "rate_limit_error",
        500.. => "api_error",
        _ => "invalid_request_error",
    };  // RevExt: 47
    (
            status,
            Json(
                json!({"error":{"code":failure.code,"message":failure.message,"type":kind,"param":null}}),
            ),
        )  // RevExt: 51
            .into_response()
}  // RevExt: 31
