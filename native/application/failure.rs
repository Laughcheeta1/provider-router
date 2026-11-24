//! Stable application errors translated by the HTTP adapter.

pub type Outcome<T> = Result<T, Failure>;

#[derive(Debug)]
pub struct Failure {
    pub status: u16,
    pub code: &'static str,
    pub message: &'static str,
}

impl Failure {
    pub fn new(status: u16, code: &'static str, message: &'static str) -> Self {
        Self {
            status,
            code,
            message,
        }
    }
    pub fn bad(code: &'static str, message: &'static str) -> Self {
        Self::new(400, code, message)
    }
    pub fn missing(code: &'static str, message: &'static str) -> Self {
        Self::new(404, code, message)
    }
    pub fn provider(code: &'static str, message: &'static str) -> Self {
        Self::new(502, code, message)
    }
    pub fn invalid() -> Self {
        Self::bad(
            "INVALID_REQUEST",
            "The request does not match the API contract.",
        )
    }
    pub fn settings() -> Self {
        Self::new(
            500,
            "SETTINGS_UNAVAILABLE",
            "Provider Router settings could not be read.",
        )
    }
    pub fn not_found() -> Self {
        Self::missing("NOT_FOUND", "This endpoint is unavailable.")
    }
    pub fn request_not_found() -> Self {
        Self::missing(
            "REQUEST_NOT_FOUND",
            "No active request was found for this client.",
        )
    }
    pub fn cancelled() -> Self {
        Self::new(499, "CANCELLED", "The inference request was cancelled.")
    }
    pub fn router_timeout() -> Self {
        Self::new(
            504,
            "ROUTER_TIMEOUT",
            "The inference request exceeded the Router's deadline.",
        )
    }
    pub fn document() -> Self {
        Self::new(
            422,
            "FILE_UNREADABLE",
            "The attachment could not be extracted within the supported document limits.",
        )
    }
}
