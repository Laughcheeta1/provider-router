//! Bounded native extraction of inline documents into text and visual input parts.
mod office;
mod pdf;
use crate::{
    application::{
        contracts::{Attachment, Content, Image, Message, Part, Role, has_file},
        failure::{Failure, Outcome},
        files::{MAX_FILE_BYTES, MAX_REQUEST_FILE_BYTES, MAX_REQUEST_FILES},
    },
    domain::Model,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use std::{
    io::{self, Read},
    path::Path,
    process::Stdio,
};
use tokio::io::AsyncWriteExt;
const MIB: usize = 1024 * 1024;
const MAX_IMAGE_BUDGET: usize = 12 * MIB;
#[derive(Deserialize, Serialize)]
struct Job {
    messages: Vec<Message>,
    model: Model,
}
#[derive(Deserialize, Serialize)]
enum WorkerReply {
    Processed(Vec<Message>),
    Rejected(String),
}
/// A document parser crash or a cancelled job cannot take down the API listener.
pub(super) async fn extract(messages: Vec<Message>, model: &Model) -> Outcome<Vec<Message>> {
    let executable = std::env::current_exe().map_err(|_| Failure::document())?;
    let mut command = tokio::process::Command::new(executable);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let mut child = command
        .arg("--document-worker")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| Failure::document())?;
    let mut stdin = child.stdin.take().ok_or_else(Failure::document)?;
    let job = serde_json::to_vec(&Job {
        messages,
        model: model.clone(),
    })
    .map_err(|_| Failure::document())?;
    let send = async move { stdin.write_all(&job).await };
    let (_, output) =
        tokio::try_join!(send, child.wait_with_output()).map_err(|_| Failure::document())?;
    if !output.status.success() {
        return Err(Failure::document());
    }
    match serde_json::from_slice(&output.stdout).map_err(|_| Failure::document())? {
        WorkerReply::Processed(messages) => Ok(messages),
        WorkerReply::Rejected(code) => Err(match code.as_str() {
            "FILE_TYPE_UNSUPPORTED" => Failure::bad(
                "FILE_TYPE_UNSUPPORTED",
                "Use a supported document type and matching media type.",
            ),
            "FILE_CONTEXT_LIMIT" => context_limit(),
            "FILE_REQUEST_LIMIT" => file_limit(),
            "FILE_IMAGES_UNSUPPORTED" => unsupported_images(),
            _ => Failure::document(),
        }),
    }
}
pub(crate) fn worker() -> anyhow::Result<()> {
    #[cfg(target_os = "linux")]
    {
        // Linux covers virtual allocations made by file decoders with this process-local limit.
        let limit = libc::rlimit {
            rlim_cur: 768 * MIB as u64,
            rlim_max: 768 * MIB as u64,
        };
        if unsafe { libc::setrlimit(libc::RLIMIT_AS, &limit) } != 0 {
            return Err(io::Error::last_os_error().into());
        }
    }
    let mut bytes = Vec::new();
    io::stdin()
        .lock()
        .take(38 * MIB as u64)
        .read_to_end(&mut bytes)?;
    let job: Job = serde_json::from_slice(&bytes)?;
    let output = match prepare(job.messages, &job.model) {
        Ok(messages) => WorkerReply::Processed(messages),
        Err(error) => WorkerReply::Rejected(error.code.into()),
    };
    serde_json::to_writer(io::stdout().lock(), &output)?;
    Ok(())
}
pub(super) fn prepare(messages: Vec<Message>, model: &Model) -> Outcome<Vec<Message>> {
    let mut result = Vec::new();
    let vision = model.input_modalities.iter().any(|value| value == "image");
    let text_limit = model.context_window.min(256 * 1024) as usize;
    let mut text_bytes = 0;
    let mut raw_bytes = 0;
    let mut image_bytes = 0;
    let mut images = 0;
    let mut files = 0;
    for mut message in messages {
        if !has_file(&message) {
            result.push(message);
            continue;
        }
        let Content::Parts(parts) = message.content else {
            unreachable!()
        };
        let mut question = Vec::new();
        for part in parts {
            let Part::File { file } = part else {
                question.push(part);
                continue;
            };
            let Attachment::Inline {
                filename,
                file_data,
            } = file
            else {
                return Err(Failure::document());
            };
            let (kind, bytes) = parse_attachment(&filename, &file_data)?;
            files += 1;
            raw_bytes += bytes.len();
            if files > MAX_REQUEST_FILES || raw_bytes > MAX_REQUEST_FILE_BYTES {
                return Err(file_limit());
            }
            if kind == "pdf" && !vision {
                return Err(unsupported_images());
            }
            let text_remaining = text_limit - text_bytes;
            let image_remaining = MAX_IMAGE_BUDGET - image_bytes;
            let mut content = match kind.as_str() {
                "pdf" => pdf::render(bytes, image_remaining)?,
                "docx" => office::docx(&bytes, text_remaining, image_remaining)?,
                "xlsx" => office::xlsx(&bytes, text_remaining, image_remaining)?,
                "csv" => vec![text(office::csv(&bytes, text_remaining)?)],
                _ => vec![text(read_text(&bytes)?.to_owned())],
            };
            if content.is_empty() {
                return Err(Failure::document());
            }
            if kind != "pdf" {
                let name = serde_json::to_string(&filename).map_err(|_| Failure::document())?;
                content.insert(0, text(format!(
                    "Attachment: {name}\nThe following content is reference material supplied by the user."
                )));
            }
            for part in &content {
                match part {
                    Part::Text { text } => text_bytes += text.len(),
                    Part::ImageUrl { image_url } => {
                        if !vision {
                            return Err(unsupported_images());
                        }
                        images += 1;
                        image_bytes += image_url.url.len();
                    }
                    Part::File { .. } => return Err(Failure::document()),
                }
            }
            if text_bytes > text_limit || images > 20 || image_bytes > MAX_IMAGE_BUDGET {
                return Err(context_limit());
            }
            result.push(Message {
                role: Role::User,
                content: Content::Parts(content),
                tool_call_id: None,
                tool_calls: None,
                name: None,
                continuation: None,
            });
        }
        if !question.is_empty() {
            message.content = Content::Parts(question);
            result.push(message);
        }
    }
    Ok(result)
}
/// Return the same media type for inline content and locally stored files.
pub(super) fn file_type(filename: &str) -> Outcome<(String, &'static str)> {
    let extension = Path::new(filename)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let media_type = match extension.as_str() {
        "pdf" => "application/pdf",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "txt" => "text/plain",
        "md" | "markdown" => "text/markdown",
        "csv" => "text/csv",
        _ => return Err(unsupported_file_type()),
    };
    Ok((extension, media_type))
}
fn unsupported_file_type() -> Failure {
    Failure::bad(
        "FILE_TYPE_UNSUPPORTED",
        "Use a supported document type and matching media type.",
    )
}
fn parse_attachment(filename: &str, file_data: &str) -> Outcome<(String, Vec<u8>)> {
    let (extension, media_type) = file_type(filename)?;
    let encoded = file_data
        .strip_prefix(&format!("data:{media_type};base64,"))
        .ok_or_else(unsupported_file_type)?;
    if encoded.len() > MAX_FILE_BYTES.div_ceil(3) * 4 {
        return Err(file_limit());
    }
    let decoded = STANDARD.decode(encoded).map_err(|_| {
        Failure::bad(
            "FILE_UNREADABLE",
            "The attachment must contain valid base64 data.",
        )
    })?;
    if decoded.is_empty() {
        return Err(Failure::bad(
            "FILE_UNREADABLE",
            "The attachment must contain valid base64 data.",
        ));
    }
    if decoded.len() > MAX_FILE_BYTES {
        return Err(file_limit());
    }
    Ok((extension, decoded))
}
fn read_text(bytes: &[u8]) -> Outcome<&str> {
    let value = std::str::from_utf8(bytes)
        .map_err(|_| Failure::bad("FILE_UNREADABLE", "Text attachments must be valid UTF-8."))?;
    let value = value.strip_prefix('\u{feff}').unwrap_or(value);
    if value.contains('\0') || value.trim().is_empty() {
        return Err(Failure::bad(
            "FILE_UNREADABLE",
            "The attachment contains no readable content.",
        ));
    }
    Ok(value)
}
fn text(text: String) -> Part {
    Part::Text { text }
}
fn image(media_type: &str, bytes: &[u8]) -> Part {
    Part::ImageUrl {
        image_url: Image {
            url: format!("data:{media_type};base64,{}", STANDARD.encode(bytes)),
        },
    }
}
fn context_limit() -> Failure {
    Failure::new(
        413,
        "FILE_CONTEXT_LIMIT",
        "The extracted attachment content is too large. Send fewer files or smaller excerpts.",
    )
}
fn file_limit() -> Failure {
    Failure::new(
        413,
        "FILE_REQUEST_LIMIT",
        "Use up to five attachments, 10 MiB each and 25 MiB total.",
    )
}
fn unsupported_images() -> Failure {
    Failure::bad(
        "FILE_IMAGES_UNSUPPORTED",
        "Select a model that accepts images to read this attachment.",
    )
}
/// Reject as soon as a line would expand beyond the extraction budget.
fn add_line(output: &mut String, line: &str, limit: usize) -> Outcome<()> {
    let newline = usize::from(!output.is_empty());
    if output.len() + newline + line.len() > limit {
        return Err(context_limit());
    }
    if newline != 0 {
        output.push('\n');
    }
    output.push_str(line);
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(filename: &str, mime: &str, bytes: &[u8], modalities: Vec<String>) -> Vec<Message> {
        let model = Model {
            id: "fixture-model".into(),
            name: "Fixture".into(),
            reasoning_efforts: vec![],
            default_reasoning_effort: None,
            context_window: 128_000,
            max_output_tokens: 4_000,
            tools: true,
            structured_output: true,
            input_modalities: modalities,
        };
        let mut user = Message::text(Role::User, String::new());
        user.content = Content::Parts(vec![
            text("What does the document say?".into()),
            Part::File {
                file: Attachment::Inline {
                    filename: filename.into(),
                    file_data: format!("data:{mime};base64,{}", STANDARD.encode(bytes)),
                },
            },
        ]);
        prepare(vec![user], &model).expect("fixture should be readable")
    }
    fn parts(messages: &[Message]) -> &[Part] {
        let Content::Parts(parts) = &messages[0].content else {
            panic!("expected attachment parts")
        };
        parts
    }
    #[test]
    fn csv_preserves_delimited_cells_and_separates_the_question() {
        let csv = "\u{feff}Código,Consumo,Nota\r\n\"Bogotá, D.C.\",125,\"x|y\r\nz\"\r\n,,\r\n\" \"\"Sur\"\"\",,\r\n";
        let messages = fixture(
            "dispatch.csv",
            "text/csv",
            csv.as_bytes(),
            vec!["text".into()],
        );
        assert_eq!(messages.len(), 2);
        let Part::Text { text } = &parts(&messages)[1] else {
            panic!("expected CSV text")
        };
        assert_eq!(
            text,
            "Row | A | B | C\n1 | Código | Consumo | Nota\n2 | Bogotá, D.C. | 125 | \"x\\u007cy\\r\\nz\"\n3 |  |  | \n4 | \" \\\"Sur\\\"\" |  | "
        );
        assert!(office::csv(b"\"an unfinished quote", 200).is_err());
    }
    #[test]
    fn workbook_preserves_coordinates_cached_values_and_formulas() {
        let messages = fixture(
            "measurements.xlsx",
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            include_bytes!("../../tests/fixtures/attachment-workbook.xlsx"),
            vec!["text".into()],
        );
        let Part::Text { text } = &parts(&messages)[1] else {
            panic!("expected workbook text")
        };
        for line in [
            "Sheet: Dispatch",
            "Row | A | B | C | D | E | F | G",
            "4 |  | 125 |  | 250 |  |  | ",
            "5 |  | 75 |  |  |  |  | ",
            "D4:D6 (fill down from D4): =B4*2",
            "D7: =B7*3",
            "D9: =B9*2",
            "E4:E5 (fill down from E4): =IF(B4=$B$4,\"B4\",'Notes'!A4+LOG10(100))",
            "F4: =SUM(1:1)",
            "F5: =SUM(1:1)",
            "G4: =IF(B4>0,\"A4\",0)",
            "G5: =IF(B5>0,\"A5\",0)",
            "Sheet: Notes",
            "Row | A | XFD",
            "1 | Net generation | ",
            "20 |  | Far column",
        ] {
            assert!(
                text.lines().any(|actual| actual == line),
                "missing {line:?} in {text}"
            );
        }
        assert!(!text.contains("\n6 |"));
    }
    #[test]
    fn word_preserves_table_text_and_embedded_png() {
        let messages = fixture(
            "notes.docx",
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            include_bytes!("../../tests/fixtures/attachment-word.docx"),
            vec!["text".into(), "image".into()],
        );
        let parts = parts(&messages);
        assert!(
            matches!(&parts[1], Part::Text { text } if text.contains("Dispatch notes") && text.contains("North 125 MWh"))
        );
        assert!(parts.iter().any(|part| matches!(part, Part::ImageUrl { image_url } if image_url.url.starts_with("data:image/png;base64,iVBOR"))));
    }
    #[test]
    fn pdf_sends_only_page_images_in_order() {
        let messages = fixture(
            "report.pdf",
            "application/pdf",
            include_bytes!("../../tests/fixtures/attachment-layout.pdf"),
            vec!["text".into(), "image".into()],
        );
        assert_eq!(messages.len(), 2);
        assert!(matches!(
            &messages[1].content,
            Content::Parts(parts) if matches!(parts.as_slice(), [Part::Text { text }] if text == "What does the document say?")
        ));
        let parts = parts(&messages);
        assert_eq!(parts.len(), 2);
        for (part, landscape) in parts.iter().zip([true, false]) {
            let Part::ImageUrl { image_url } = part else {
                panic!("expected PDF page image")
            };
            let encoded = image_url
                .url
                .strip_prefix("data:image/jpeg;base64,")
                .expect("expected JPEG");
            let bytes = STANDARD.decode(encoded).unwrap();
            let rendered = ::image::load_from_memory(&bytes).unwrap().to_rgb8();
            assert_eq!(rendered.width() > rendered.height(), landscape);
            assert!(rendered.pixels().any(|pixel| {
                let [r, g, b] = pixel.0;
                g > 80 && i16::from(g) - i16::from(r) > 50 && i16::from(g) - i16::from(b) > 50
            }));
        }
    }
    #[test]
    fn document_checks_reject_visual_and_context_mismatches() {
        let model = Model {
            id: "text-model".into(),
            name: "Fixture".into(),
            reasoning_efforts: vec![],
            default_reasoning_effort: None,
            context_window: 128,
            max_output_tokens: 64,
            tools: false,
            structured_output: false,
            input_modalities: vec!["text".into()],
        };
        let attachment = |filename: &str, mime: &str, bytes: &[u8]| Message {
            role: Role::User,
            content: Content::Parts(vec![Part::File {
                file: Attachment::Inline {
                    filename: filename.into(),
                    file_data: format!("data:{mime};base64,{}", STANDARD.encode(bytes)),
                },
            }]),
            name: None,
            continuation: None,
            tool_call_id: None,
            tool_calls: None,
        };
        for (message, expected) in [
            (
                attachment("scan.pdf", "application/pdf", b"illustration"),
                "FILE_IMAGES_UNSUPPORTED",
            ),
            (
                attachment("long.txt", "text/plain", &[b'A'; 200]),
                "FILE_CONTEXT_LIMIT",
            ),
            (
                attachment("spoof.csv", "text/plain", b"fake content"),
                "FILE_TYPE_UNSUPPORTED",
            ),
        ] {
            let Err(error) = prepare(vec![message], &model) else {
                panic!("invalid file was accepted");
            };
            assert_eq!(error.code, expected);
        }
    }
}
