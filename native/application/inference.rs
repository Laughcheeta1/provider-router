//! Inference authorization, admission, cancellation, and dispatch.
use super::{
    contracts::{self, Completion, Message, Request as Inference},
    failure::{Failure, Outcome},
    files::{FILE_NOT_FOUND, FileObject, Upload, file_key_required},
};
use crate::domain::{Connection, Method, Model, State};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    future::Future,
    sync::{Arc, Mutex},
    time::Duration,
};
use subtle::ConstantTimeEq;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(600);
pub trait InferenceBackend: Send + Sync + 'static {
    fn settings(&self) -> anyhow::Result<State>;
    fn prepare_documents(
        &self,
        messages: Vec<Message>,
        model: &Model,
        file_owner: Option<Uuid>,
    ) -> impl Future<Output = Outcome<Vec<Message>>> + Send;
    fn file_references_supported(&self) -> bool {
        false
    }
    fn upload_file(&self, _: Uuid, _: Upload) -> impl Future<Output = Outcome<FileObject>> + Send {
        async { Err(Failure::not_found()) }
    }
    fn delete_file(&self, _: Uuid, _: Uuid) -> impl Future<Output = Outcome<()>> + Send {
        async { Err(Failure::not_found()) }
    }
    fn complete(
        &self,
        connection: &Connection,
        request: &Inference,
    ) -> impl Future<Output = Outcome<Completion>> + Send;
}
pub struct InferenceService<B> {
    backend: B,
    registry: Arc<Mutex<Registry>>,
    document_slots: Arc<Semaphore>,
    shutdown: CancellationToken,
}
impl<B: InferenceBackend> InferenceService<B> {
    pub fn new(backend: B, shutdown: CancellationToken) -> Self {
        Self {
            backend,
            registry: Arc::new(Mutex::new(Registry::default())),
            document_slots: Arc::new(Semaphore::new(2)),
            shutdown,
        }
    }
    pub fn snapshot(&self) -> Outcome<State> {
        self.backend.settings().map_err(|_| Failure::settings())
    }
    pub fn can_reference_files(&self, principal: Principal) -> bool {
        principal.file_owner().is_some() && self.backend.file_references_supported()
    }
    pub async fn upload_file(&self, principal: Principal, file: Upload) -> Outcome<FileObject> {
        let owner = principal.file_owner().ok_or_else(file_key_required)?;
        let uploaded = self.backend.upload_file(owner, file).await?;
        if !self.still_authorized(principal) {
            let id = super::files::file_uuid(&uploaded.id)?;
            let _ = self.backend.delete_file(owner, id).await;
            return Err(file_key_required());
        }
        Ok(uploaded)
    }
    pub async fn delete_file(&self, principal: Principal, id: Uuid) -> Outcome<()> {
        let owner = principal.file_owner().ok_or_else(file_key_required)?;
        self.backend.delete_file(owner, id).await
    }
    pub fn cancel(&self, id: Uuid, principal: Principal) -> Outcome<()> {
        let registry = self.registry.lock().map_err(|_| Failure::settings())?;
        let pending = registry
            .active
            .get(&id)
            .filter(|pending| pending.client == principal.owner)
            .ok_or_else(Failure::request_not_found)?;
        pending.cancel.cancel();
        Ok(())
    }
    fn begin(&self, id: Uuid, client: Uuid) -> Outcome<Lease> {
        let mut registry = self.registry.lock().map_err(|_| Failure::settings())?;
        if registry.active.contains_key(&id) || registry.recent.contains(&id) {
            return Err(Failure::new(
                409,
                "REQUEST_REPLAYED",
                "This request identifier has already been used.",
            ));
        }
        if registry.active.len() >= 16
            || registry
                .active
                .values()
                .filter(|pending| pending.client == client)
                .count()
                >= 4
        {
            return Err(Failure::new(
                429,
                "CLIENT_BUSY",
                "Too many inference requests are active.",
            ));
        }
        let cancel = self.shutdown.child_token();
        registry.active.insert(
            id,
            Pending {
                client,
                cancel: cancel.clone(),
            },
        );
        Ok(Lease {
            registry: self.registry.clone(),
            id,
            cancel,
            retryable: false,
        })
    }
    pub async fn infer(
        &self,
        state: &State,
        principal: Principal,
        mut request: Inference,
        check: bool,
    ) -> Outcome<Completion> {
        if request.request_id.get_version_num() != 7 || request.connection_id.get_version_num() != 7
        {
            return Err(Failure::invalid());
        }
        let connection = exposed(state)
            .find(|connection| connection.id == request.connection_id)
            .ok_or_else(|| {
                Failure::missing(
                    "CONNECTION_UNAVAILABLE",
                    "The requested connection is not exposed.",
                )
            })?;
        let model = connection
            .models
            .iter()
            .find(|model| model.id == request.model)
            .ok_or_else(|| {
                Failure::bad(
                    "MODEL_UNAVAILABLE",
                    "The selected model is not exposed by this connection.",
                )
            })?;
        if request.compaction == Some(true) && (check || connection.method != Method::Codex) {
            return Err(Failure::bad(
                "COMPACTION_UNSUPPORTED",
                "Explicit compaction requires a Codex subscription connection.",
            ));
        }
        request.validate(model)?;
        if request.reasoning_effort.is_none() {
            request.reasoning_effort = model
                .default_reasoning_effort
                .clone()
                .or_else(|| model.reasoning_efforts.first().cloned());
        }
        let mut lease = self.begin(request.request_id, principal.owner)?;
        let mut dispatched = false;
        let actual = async {
            if request.messages.iter().any(contracts::has_file) {
                let _permit = self
                    .document_slots
                    .clone()
                    .try_acquire_owned()
                    .map_err(|_| {
                        Failure::new(
                            429,
                            "DOCUMENT_BUSY",
                            "Too many documents are being processed.",
                        )
                    })?;
                let messages = std::mem::take(&mut request.messages);
                request.messages = tokio::time::timeout(
                    Duration::from_secs(30),
                    self.backend
                        .prepare_documents(messages, model, principal.file_owner()),
                )
                .await
                .map_err(|_| Failure::document())??;
            }
            if check {
                return Ok(Completion {
                    request_id: request.request_id,
                    content: "Attachments fit the request.".into(),
                    tool_calls: vec![],
                    usage: contracts::Usage {
                        input_tokens: 0,
                        output_tokens: 0,
                        cached_input_tokens: None,
                        cache_write_tokens: None,
                    },
                    provider: connection.method.provider().into(),
                    method: method_name(connection),
                    model: request.model,
                    reasoning_effort: None,
                    continuation: None,
                });
            }
            dispatched = true;
            let result = self.backend.complete(connection, &request).await?;
            request.evaluate(&result, connection)?;
            Ok(result)
        };
        let result = tokio::select! {
            _ = lease.cancel.cancelled() => Err(Failure::cancelled()),
            _ = self.revoked(principal) => Err(Failure::cancelled()),
            result = tokio::time::timeout(REQUEST_TIMEOUT, actual) => result.unwrap_or_else(|_| Err(Failure::router_timeout())),
        };
        // A corrected file reference may reuse this ID because no model call was attempted.
        lease.retryable = !dispatched
            && result
                .as_ref()
                .is_err_and(|error| error.code == FILE_NOT_FOUND);
        if result.is_ok() && !self.still_authorized(principal) {
            return Err(Failure::new(
                401,
                "CLIENT_UNAUTHORIZED",
                "The client key was revoked.",
            ));
        }
        result
    }
    fn still_authorized(&self, principal: Principal) -> bool {
        let Ok(state) = self.backend.settings() else {
            return false;
        };
        principal
            .token_hash
            .map_or(!state.network.require_api_key, |hash| {
                state.clients.iter().any(|client| {
                    client.id == principal.owner && hash_matches(&client.token_hash, &hash)
                })
            })
    }
    async fn revoked(&self, principal: Principal) {
        let mut interval = tokio::time::interval(Duration::from_millis(500));
        loop {
            interval.tick().await;
            if !self.still_authorized(principal) {
                return;
            }
        }
    }
}
fn method_name(connection: &Connection) -> String {
    serde_json::to_value(connection.method)
        .unwrap_or(Value::Null)
        .as_str()
        .unwrap_or_default()
        .to_owned()
}
pub fn exposed(state: &State) -> impl Iterator<Item = &Connection> {
    state
        .connections
        .iter()
        .filter(|connection| connection.exposed)
}
#[derive(Default)]
struct Registry {
    active: HashMap<Uuid, Pending>,
    recent: HashSet<Uuid>,
    order: VecDeque<Uuid>,
}
struct Pending {
    client: Uuid,
    cancel: CancellationToken,
}
struct Lease {
    registry: Arc<Mutex<Registry>>,
    id: Uuid,
    cancel: CancellationToken,
    retryable: bool,
}
impl Drop for Lease {
    fn drop(&mut self) {
        if let Ok(mut registry) = self.registry.lock() {
            registry.active.remove(&self.id);
            if !self.retryable || self.cancel.is_cancelled() {
                registry.recent.insert(self.id);
                registry.order.push_back(self.id);
                if registry.order.len() > 4096
                    && let Some(oldest) = registry.order.pop_front()
                {
                    registry.recent.remove(&oldest);
                }
            }
        }
        self.cancel.cancel();
    }
}
#[derive(Clone, Copy)]
pub struct Principal {
    owner: Uuid,
    token_hash: Option<[u8; 32]>,
}
impl Principal {
    pub fn file_owner(self) -> Option<Uuid> {
        self.token_hash.map(|_| self.owner)
    }
}
pub fn authenticate(state: &State, token: Option<&str>) -> Outcome<Principal> {
    if let Some(token) = token.filter(|value| {
        value.len() == 43
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    }) {
        let hash = Sha256::digest(token.as_bytes()).into();
        if let Some(client) = state
            .clients
            .iter()
            .find(|client| hash_matches(&client.token_hash, &hash))
        {
            return Ok(Principal {
                owner: client.id,
                token_hash: Some(hash),
            });
        }
    }
    if state.network.require_api_key {
        return Err(Failure::new(
            401,
            "CLIENT_UNAUTHORIZED",
            "A valid Provider Router client key is required.",
        ));
    }
    Ok(Principal {
        owner: state.instance_id,
        token_hash: None,
    })
}
fn hash_matches(expected: &str, actual: &[u8; 32]) -> bool {
    let Some(hash) = hex::decode(expected)
        .ok()
        .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
    else {
        return false;
    };
    bool::from(hash.ct_eq(actual))
}
