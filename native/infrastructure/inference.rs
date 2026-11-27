//! Wire the inference use case to the local profile, document worker, and providers.

use std::time::Duration;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    application::{
        contracts::{Completion, Message, Request},
        failure::{Failure, Outcome},
        files::{FileObject, Upload},
        inference::{InferenceBackend, REQUEST_TIMEOUT},
    },
    domain::{Connection, Model, State},
};

use super::{
    documents,
    files::FileStore,
    profile::Profile,
    provider::{self, ProviderError},
};

pub struct LocalInference {
    profile: Profile,
    http: reqwest::Client,
    files: FileStore,
}

impl LocalInference {
    pub fn new(profile: Profile, shutdown: CancellationToken) -> Self {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(15))
            .timeout(REQUEST_TIMEOUT)
            .build()
            .expect("HTTP client can be configured");
        let files = FileStore::new(&profile.dir, shutdown);
        Self {
            profile,
            http,
            files,
        }
    }
}

impl InferenceBackend for LocalInference {
    fn settings(&self) -> anyhow::Result<State> {
        self.profile.read()
    }

    async fn prepare_documents(
        &self,
        messages: Vec<Message>,
        model: &Model,
        file_owner: Option<Uuid>,
    ) -> Outcome<Vec<Message>> {
        let files = self.files.clone();
        let messages = tokio::task::spawn_blocking(move || files.resolve(file_owner, messages))
            .await
            .map_err(|_| Failure::document())??;
        documents::extract(messages, model).await
    }

    fn file_references_supported(&self) -> bool {
        true
    }

    async fn upload_file(&self, owner: Uuid, upload: Upload) -> Outcome<FileObject> {
        let files = self.files.clone();
        tokio::task::spawn_blocking(move || files.upload(owner, upload))
            .await
            .map_err(|_| Failure::settings())?
    }

    async fn delete_file(&self, owner: Uuid, id: Uuid) -> Outcome<()> {
        let files = self.files.clone();
        tokio::task::spawn_blocking(move || files.delete(owner, id))
            .await
            .map_err(|_| Failure::settings())?
    }

    async fn complete(&self, connection: &Connection, request: &Request) -> Outcome<Completion> {
        let key = self
            .profile
            .secret(connection.id)
            .map_err(|_| Failure::settings())?;
        let upstream = serde_json::to_value(request).map_err(|_| Failure::invalid())?;
        let value = provider::infer(&self.http, connection, key.as_deref(), &upstream)
            .await
            .map_err(|error| {
                if let Some(error) = error.downcast_ref::<ProviderError>() {
                    Failure::new(error.status, error.code, error.message)
                } else {
                    Failure::provider(
                        "PROVIDER_FAILED",
                        "The provider could not complete this request.",
                    )
                }
            })?;
        serde_json::from_value(value).map_err(|_| {
            Failure::provider(
                "INVALID_COMPLETION",
                "The provider returned an invalid inference response.",
            )
        })
    }
}
