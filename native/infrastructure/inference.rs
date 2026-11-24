//! Wire the inference use case to the local profile, document worker, and providers.

use std::time::Duration;

use crate::{
    application::{
        contracts::{Completion, Message, Request},
        failure::{Failure, Outcome},
        inference::{InferenceBackend, REQUEST_TIMEOUT},
    },
    domain::{Connection, Model, State},
};

use super::{
    documents,
    profile::Profile,
    provider::{self, ProviderError},
};

pub struct LocalInference {
    profile: Profile,
    http: reqwest::Client,
}

impl LocalInference {
    pub fn new(profile: Profile) -> Self {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(15))
            .timeout(REQUEST_TIMEOUT)
            .build()
            .expect("HTTP client can be configured");
        Self { profile, http }
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
    ) -> Outcome<Vec<Message>> {
        documents::extract(messages, model).await
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
