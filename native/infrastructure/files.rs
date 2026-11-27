//! Store temporary attachments under the local Router profile.

use crate::application::{
    contracts::{Attachment, Content, Message, Part},
    failure::{Failure, Outcome},
    files::{
        DEFAULT_EXPIRY_SECONDS, FileObject, MAX_EXPIRY_SECONDS, MAX_FILE_BYTES,
        MAX_REQUEST_FILE_BYTES, MAX_REQUEST_FILES, Upload, file_key_required, file_not_found,
        file_too_large, file_uuid,
    },
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File},
    io::{self, BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Clone)]
pub(super) struct FileStore(PathBuf);

#[derive(Deserialize, Serialize)]
struct StoredFile {
    owner: Uuid,
    filename: String,
    bytes: usize,
    created_at: u64,
    expires_at: u64,
}

impl FileStore {
    pub(super) fn new(profile: &Path, shutdown: CancellationToken) -> Self {
        let store = Self(profile.join("files"));
        let cleanup = store.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));
            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    _ = interval.tick() => {
                        let cleanup = cleanup.clone();
                        let _ = tokio::task::spawn_blocking(move || cleanup.sweep(now())).await;
                    }
                }
            }
        });
        store
    }

    pub(super) fn upload(&self, owner: Uuid, upload: Upload) -> Outcome<FileObject> {
        if upload.filename.trim().is_empty()
            || upload.filename.chars().count() > 255
            || upload.filename.chars().any(char::is_control)
            || upload.filename.contains(['/', '\\'])
            || upload.bytes.is_empty()
            || !(DEFAULT_EXPIRY_SECONDS..=MAX_EXPIRY_SECONDS)
                .contains(&upload.expires_after_seconds)
        {
            return Err(Failure::invalid());
        }
        if upload.bytes.len() > MAX_FILE_BYTES {
            return Err(file_too_large());
        }
        super::documents::file_type(&upload.filename)?;
        let mut dir = fs::DirBuilder::new();
        dir.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            dir.mode(0o700);
        }
        dir.create(&self.0).map_err(|_| storage_unavailable())?;
        let id = Uuid::now_v7();
        let created_at = now();
        let record = StoredFile {
            owner,
            filename: upload.filename,
            bytes: upload.bytes.len(),
            created_at,
            expires_at: created_at + upload.expires_after_seconds,
        };
        let mut temp =
            tempfile::NamedTempFile::new_in(&self.0).map_err(|_| storage_unavailable())?;
        serde_json::to_writer(&mut temp, &record).map_err(|_| storage_unavailable())?;
        temp.write_all(b"\n").map_err(|_| storage_unavailable())?;
        temp.write_all(&upload.bytes)
            .map_err(|_| storage_unavailable())?;
        temp.as_file()
            .sync_all()
            .map_err(|_| storage_unavailable())?;
        temp.persist_noclobber(self.path(id))
            .map_err(|_| storage_unavailable())?;
        Ok(FileObject {
            id: format!("file-{id}"),
            object: "file",
            bytes: record.bytes,
            created_at: record.created_at,
            filename: record.filename,
            purpose: "user_data",
            expires_at: record.expires_at,
            status: "processed",
        })
    }

    pub(super) fn delete(&self, owner: Uuid, id: Uuid) -> Outcome<()> {
        let (_, reader) = self.open_owned(owner, id)?;
        drop(reader);
        fs::remove_file(self.path(id)).map_err(file_io)
    }

    pub(super) fn resolve(
        &self,
        owner: Option<Uuid>,
        mut messages: Vec<Message>,
    ) -> Outcome<Vec<Message>> {
        let mut file_count = 0;
        let mut bytes_total = 0;
        for message in &mut messages {
            let Content::Parts(parts) = &mut message.content else {
                continue;
            };
            for part in parts {
                let Part::File { file } = part else {
                    continue;
                };
                file_count += 1;
                if file_count > MAX_REQUEST_FILES {
                    return Err(file_too_large());
                }
                let file_id = match file {
                    Attachment::Reference { file_id } => file_id,
                    Attachment::Inline { file_data, .. } => {
                        if let Some((_, encoded)) = file_data.split_once(";base64,") {
                            // Reserve space before rehydrating references. Extraction still
                            // validates the actual inline MIME and decodes the original.
                            let padding = encoded
                                .bytes()
                                .rev()
                                .take_while(|byte| *byte == b'=')
                                .take(2)
                                .count();
                            bytes_total += (encoded.len() / 4 * 3).saturating_sub(padding);
                            if bytes_total > MAX_REQUEST_FILE_BYTES {
                                return Err(file_too_large());
                            }
                        }
                        continue;
                    }
                };
                let owner = owner.ok_or_else(file_key_required)?;
                let (record, reader) = self.open_owned(owner, file_uuid(file_id)?)?;
                bytes_total += record.bytes;
                if bytes_total > MAX_REQUEST_FILE_BYTES {
                    return Err(file_too_large());
                }
                let mut bytes = Vec::with_capacity(record.bytes);
                reader
                    .take((record.bytes + 1) as u64)
                    .read_to_end(&mut bytes)
                    .map_err(file_io)?;
                if bytes.len() != record.bytes {
                    return Err(storage_unavailable());
                }
                let (_, mime) = super::documents::file_type(&record.filename)?;
                *file = Attachment::Inline {
                    filename: record.filename,
                    file_data: format!("data:{mime};base64,{}", STANDARD.encode(bytes)),
                };
            }
        }
        Ok(messages)
    }

    fn path(&self, id: Uuid) -> PathBuf {
        self.0.join(format!("{id}.upload"))
    }

    fn open_owned(&self, owner: Uuid, id: Uuid) -> Outcome<(StoredFile, BufReader<File>)> {
        let mut reader = BufReader::new(File::open(self.path(id)).map_err(file_io)?);
        let record = read_metadata(&mut reader).map_err(file_io)?;
        if record.owner != owner || record.expires_at <= now() {
            return Err(file_not_found());
        }
        if record.bytes == 0 || record.bytes > MAX_FILE_BYTES {
            return Err(storage_unavailable());
        }
        Ok((record, reader))
    }

    fn sweep(&self, timestamp: u64) {
        let Ok(entries) = fs::read_dir(&self.0) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path
                .extension()
                .is_some_and(|extension| extension == "upload")
            {
                if let Ok(file) = File::open(&path)
                    && read_metadata(&mut BufReader::new(file))
                        .is_ok_and(|record| record.expires_at <= timestamp)
                {
                    let _ = fs::remove_file(path);
                }
            } else if entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with(".tmp"))
                && entry
                    .metadata()
                    .and_then(|info| info.modified())
                    .is_ok_and(|modified| {
                        modified
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs()
                            .saturating_add(DEFAULT_EXPIRY_SECONDS)
                            <= timestamp
                    })
            {
                let _ = fs::remove_file(path);
            }
        }
    }
}

fn read_metadata(reader: &mut BufReader<File>) -> io::Result<StoredFile> {
    // File contents follow the bounded, single-line JSON metadata without base64 inflation.
    let mut header = Vec::new();
    reader.take(8192).read_until(b'\n', &mut header)?;
    if !header.ends_with(b"\n") {
        return Err(io::Error::from(io::ErrorKind::InvalidData));
    }
    serde_json::from_slice(&header).map_err(io::Error::other)
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn file_io(error: io::Error) -> Failure {
    if error.kind() == io::ErrorKind::NotFound {
        file_not_found()
    } else {
        storage_unavailable()
    }
}

fn storage_unavailable() -> Failure {
    Failure::new(
        500,
        "FILE_STORAGE_UNAVAILABLE",
        "Router file storage is unavailable.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::contracts::Role;
    use crate::domain::Model;

    #[test]
    fn retained_file_references_keep_their_content_and_expire() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let first = FileStore(dir.path().join("files"));
        let reopened = FileStore(dir.path().join("files"));
        let owner = Uuid::now_v7();
        let foreign = Uuid::now_v7();
        let created = first
            .upload(
                owner,
                Upload {
                    filename: "note.txt".to_owned(),
                    bytes: b"Read this from the retained file.".to_vec(),
                    expires_after_seconds: 3600,
                },
            )
            .unwrap();
        let request = || {
            let mut message = Message::text(Role::User, String::new());
            message.content = Content::Parts(vec![
                Part::Text {
                    text: "What is in it?".into(),
                },
                Part::File {
                    file: Attachment::Reference {
                        file_id: created.id.clone(),
                    },
                },
            ]);
            vec![message]
        };
        for client in [None, Some(foreign)] {
            let error = reopened.resolve(client, request()).err().unwrap();
            assert_eq!(error.status, if client.is_some() { 404 } else { 401 });
        }
        let model = Model {
            id: "test-model".into(),
            name: "Test model".into(),
            reasoning_efforts: vec![],
            default_reasoning_effort: None,
            context_window: 128_000,
            max_output_tokens: 4_000,
            tools: false,
            structured_output: false,
            input_modalities: vec!["text".into()],
        };
        let prepare = || {
            let hydrated = reopened.resolve(Some(owner), request()).unwrap();
            super::super::documents::prepare(hydrated, &model).unwrap()
        };
        let first_result = serde_json::to_value(prepare())?;
        assert_eq!(first_result, serde_json::to_value(prepare())?);
        assert_eq!(
            first_result[0]["content"][1]["text"],
            "Read this from the retained file."
        );
        assert_eq!(first_result[1]["content"][0]["text"], "What is in it?");
        reopened.sweep(created.expires_at);
        assert_eq!(
            reopened
                .resolve(Some(owner), request())
                .err()
                .unwrap()
                .status,
            404
        );
        Ok(())
    }
}
