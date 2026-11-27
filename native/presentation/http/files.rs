//! Decode the OpenAI multipart file upload contract.

use crate::application::{
    failure::{Failure, Outcome},
    files::{DEFAULT_EXPIRY_SECONDS, MAX_FILE_BYTES, Upload, file_too_large},
};
use axum::extract::{FromRequest, Multipart, Request, multipart::MultipartError};
use std::time::Duration;

pub(super) async fn upload(request: Request, timeout: Duration) -> Outcome<Upload> {
    tokio::time::timeout(timeout, decode(request))
        .await
        .map_err(|_| {
            Failure::new(
                408,
                "REQUEST_TIMEOUT",
                "The file upload took too long to read.",
            )
        })?
}

async fn decode(request: Request) -> Outcome<Upload> {
    let mut multipart = Multipart::from_request(request, &()).await.map_err(|_| {
        Failure::bad(
            "INVALID_UPLOAD",
            "Use multipart/form-data to upload a file.",
        )
    })?;
    let mut file = None;
    let mut purpose = false;
    let mut anchor = false;
    let mut expires_after = None;
    while let Some(field) = multipart.next_field().await.map_err(multipart_failure)? {
        match field.name() {
            Some("file") if file.is_none() => {
                let filename = field.file_name().ok_or_else(Failure::invalid)?.to_owned();
                let bytes = field.bytes().await.map_err(multipart_failure)?;
                if bytes.len() > MAX_FILE_BYTES {
                    return Err(file_too_large());
                }
                file = Some((filename, bytes.to_vec()));
            }
            Some("purpose") if !purpose => {
                if field.text().await.map_err(multipart_failure)? != "user_data" {
                    return Err(Failure::bad(
                        "FILE_PURPOSE_UNSUPPORTED",
                        "Router accepts files with purpose user_data.",
                    ));
                }
                purpose = true;
            }
            Some("expires_after[anchor]") if !anchor => {
                if field.text().await.map_err(multipart_failure)? != "created_at" {
                    return Err(Failure::invalid());
                }
                anchor = true;
            }
            Some("expires_after[seconds]") if expires_after.is_none() => {
                expires_after = Some(
                    field
                        .text()
                        .await
                        .map_err(multipart_failure)?
                        .parse::<u64>()
                        .map_err(|_| Failure::invalid())?,
                );
            }
            _ => return Err(Failure::invalid()),
        }
    }
    let (filename, bytes) = file.ok_or_else(Failure::invalid)?;
    if !purpose || anchor != expires_after.is_some() {
        return Err(Failure::invalid());
    }
    Ok(Upload {
        filename,
        bytes,
        expires_after_seconds: expires_after.unwrap_or(DEFAULT_EXPIRY_SECONDS),
    })
}

fn multipart_failure(error: MultipartError) -> Failure {
    if error.status().as_u16() == 413 {
        file_too_large()
    } else {
        Failure::bad("INVALID_UPLOAD", "The multipart upload could not be read.")
    }
}
