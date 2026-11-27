//! The upload and opaque file reference contracts used by inference.

use super::failure::{Failure, Outcome};
use serde::Serialize;
use uuid::Uuid;

pub const MAX_FILE_BYTES: usize = 10 * 1024 * 1024;
pub const MAX_REQUEST_FILE_BYTES: usize = 25 * 1024 * 1024;
pub const MAX_REQUEST_FILES: usize = 5;
pub const MAX_UPLOAD_BODY_BYTES: usize = MAX_FILE_BYTES + 64 * 1024;
pub const DEFAULT_EXPIRY_SECONDS: u64 = 3600;
pub const MAX_EXPIRY_SECONDS: u64 = 30 * 24 * 3600;
pub const FILE_NOT_FOUND: &str = "FILE_NOT_FOUND";

pub struct Upload {
    pub filename: String,
    pub bytes: Vec<u8>,
    pub expires_after_seconds: u64,
}

#[derive(Serialize)]
pub struct FileObject {
    pub id: String,
    pub object: &'static str,
    pub bytes: usize,
    pub created_at: u64,
    pub filename: String,
    pub purpose: &'static str,
    pub expires_at: u64,
    pub status: &'static str,
}

pub fn file_uuid(id: &str) -> Outcome<Uuid> {
    let id = id.strip_prefix("file-").ok_or_else(file_not_found)?;
    Uuid::parse_str(id)
        .ok()
        .filter(|uuid| uuid.get_version_num() == 7 && uuid.to_string() == id)
        .ok_or_else(file_not_found)
}

pub fn file_not_found() -> Failure {
    Failure::missing(FILE_NOT_FOUND, "No active file was found for this client.")
}

pub fn file_key_required() -> Failure {
    Failure::new(
        401,
        "CLIENT_UNAUTHORIZED",
        "Stored files require a registered Provider Router client key.",
    )
}

pub fn file_too_large() -> Failure {
    Failure::new(
        413,
        "FILE_REQUEST_LIMIT",
        "An attachment exceeds the supported file size.",
    )
}
