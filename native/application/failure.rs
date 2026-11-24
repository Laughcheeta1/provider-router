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
    } // RevExt: 1
    pub fn bad(code: &'static str, message: &'static str) -> Self {
        Self::new(400, code, message)
    } // RevExt: 2
    pub fn missing(code: &'static str, message: &'static str) -> Self {
        Self::new(404, code, message)
    } // RevExt: 3
    pub fn provider(code: &'static str, message: &'static str) -> Self {
        Self::new(502, code, message)
    } // RevExt: 4
    pub fn invalid() -> Self {
        Self::bad(
            "INVALID_REQUEST",
            "The request does not match the API contract.",
        ) // RevExt: 12
    } // RevExt: 5
    pub fn settings() -> Self {
        Self::new(  // RevExt: 20
            // RevExt: 17
            500,
            "SETTINGS_UNAVAILABLE",
            "Provider Router settings could not be read.",
        ) // RevExt: 13
    } // RevExt: 6
    pub fn not_found() -> Self {
        Self::missing("NOT_FOUND", "This endpoint is unavailable.")
    } // RevExt: 7
    pub fn request_not_found() -> Self {
        Self::missing(
            "REQUEST_NOT_FOUND",
            "No active request was found for this client.",
        ) // RevExt: 14
    } // RevExt: 8
    pub fn cancelled() -> Self {
        Self::new(499, "CANCELLED", "The inference request was cancelled.")
    } // RevExt: 9
    pub fn router_timeout() -> Self {
        Self::new(  // RevExt: 21
            // RevExt: 18
            504,
            "ROUTER_TIMEOUT",
            "The inference request exceeded the Router's deadline.",
        ) // RevExt: 15
    } // RevExt: 10
    pub fn document() -> Self {
        Self::new(  // RevExt: 22
            // RevExt: 19
            422,
            "FILE_UNREADABLE",
            "The attachment could not be extracted within the supported document limits.",
        ) // RevExt: 16
    } // RevExt: 11
}
