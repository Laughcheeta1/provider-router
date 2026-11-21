//! Extract Word package text/images and stream spreadsheet cells without expanding sparse grids.

use super::{Failure, MIB, Outcome, Part, add_line, context_limit, image, read_text, text};
use calamine::{Data, DataType, Reader, Xlsx, expand_shared_formula};
use quick_xml::{Reader as XmlReader, events::Event};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, Cursor, Read},
};
use zip::ZipArchive;

type OfficeArchive<'a> = ZipArchive<Cursor<&'a [u8]>>;
const MAX_ARCHIVE_BYTES: usize = 64 * MIB;

pub(super) fn csv(bytes: &[u8], limit: usize) -> Outcome<String> {
    let input = read_text(bytes)?;
    validate_csv(input)?;
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .from_reader(input.as_bytes());
    let mut rows = String::new();
    let mut width = 0;
    for (offset, record) in reader.records().enumerate() {
        let record = record.map_err(|_| Failure::document())?;
        width = width.max(record.len());
        add_line(&mut rows, &grid_row(offset + 1, record.iter()), limit)?;
    }
    if rows.is_empty() {
        return Err(Failure::document());
    }
    if width > limit.saturating_div(4) {
        return Err(context_limit());
    }
    let mut output = grid_header((0..width).map(|column| column as u32));
    add_line(&mut output, &rows, limit)?;
    Ok(output)
}

fn validate_csv(text: &str) -> Outcome<()> {
    enum Field {
        Initial,
        Open,
        Closed,
        Unquoted,
    }
    let mut field = Field::Initial;
    for byte in text.bytes() {
        field = match (field, byte) {
            (Field::Open, b'"') => Field::Closed,
            (Field::Open, _) | (Field::Closed | Field::Initial, b'"') => Field::Open,
            (_, b',' | b'\r' | b'\n') => Field::Initial,
            (Field::Closed, _) | (Field::Unquoted, b'"') => return Err(Failure::document()),
            _ => Field::Unquoted,
        };
    }
    if matches!(field, Field::Open) {
        return Err(Failure::document());
    }
    Ok(())
}

pub(super) fn docx(bytes: &[u8], text_limit: usize, image_limit: usize) -> Outcome<Vec<Part>> {
    let mut archive = office_archive(bytes)?;
    let mut document = String::new();
    archive
        .by_name("word/document.xml")
        .map_err(|_| Failure::document())?
        .take(MAX_ARCHIVE_BYTES as u64 + 1)
        .read_to_string(&mut document)
        .map_err(|_| Failure::document())?;
    if document.len() > MAX_ARCHIVE_BYTES {
        return Err(context_limit());
    }
    let parsed = word_text(&document, text_limit)?;
    let mut parts = Vec::new();
    if !parsed.trim().is_empty() {
        parts.push(text(parsed));
    }
    parts.extend(embedded_images(&mut archive, "word/media/", image_limit)?);
    Ok(parts)
}

fn word_text(document: &str, limit: usize) -> Outcome<String> {
    let mut reader = XmlReader::from_str(document);
    let mut output = String::new();
    let mut inside_text = false;
    loop {
        match reader.read_event().map_err(|_| Failure::document())? {
            Event::Start(node) if node.local_name().as_ref() == b"t" => inside_text = true,
            Event::End(node) if node.local_name().as_ref() == b"t" => inside_text = false,
            Event::End(node) if matches!(node.local_name().as_ref(), b"p" | b"tr") => {
                if !output.is_empty() && !output.ends_with('\n') {
                    output.push('\n');
                }
            }
            Event::Empty(node) if matches!(node.local_name().as_ref(), b"br" | b"tab") => {
                output.push(if node.local_name().as_ref() == b"br" {
                    '\n'
                } else {
                    '\t'
                });
            }
            Event::Text(value) if inside_text => {
                output.push_str(&value.decode().map_err(|_| Failure::document())?);
            }
            Event::CData(value) if inside_text => {
                output.push_str(&value.decode().map_err(|_| Failure::document())?);
            }
            Event::GeneralRef(value) if inside_text => {
                if let Some(character) =
                    value.resolve_char_ref().map_err(|_| Failure::document())?
                {
                    output.push(character);
                } else {
                    output.push(
                        match value.decode().map_err(|_| Failure::document())?.as_ref() {
                            "amp" => '&',
                            "lt" => '<',
                            "gt" => '>',
                            "quot" => '"',
                            "apos" => '\'',
                            _ => return Err(Failure::document()),
                        },
                    );
                }
            }
            Event::DocType(_) => return Err(Failure::document()),
            Event::Eof => return Ok(output.trim().to_owned()),
            _ => (),
        }
        if output.len() > limit {
            return Err(context_limit());
        }
    }
}

pub(super) fn xlsx(bytes: &[u8], text_limit: usize, image_limit: usize) -> Outcome<Vec<Part>> {
    let mut archive = office_archive(bytes)?;
    let mut workbook = Xlsx::new(Cursor::new(bytes)).map_err(|_| Failure::document())?;
    let mut output = String::new();
    for sheet in workbook.sheet_names() {
        add_line(
            &mut output,
            &format!("Sheet: {}", inline(&sheet)),
            text_limit,
        )?;
        let mut reader = workbook
            .worksheet_cells_reader(&sheet)
            .map_err(|_| Failure::document())?;
        let mut values: BTreeMap<u32, BTreeMap<u32, String>> = BTreeMap::new();
        let mut formulas: BTreeMap<u32, BTreeMap<u32, String>> = BTreeMap::new();
        let mut used = BTreeSet::new();
        let mut minimum_size = output.len();
        while let Some(record) = reader
            .next_cell_with_formula()
            .map_err(|_| Failure::document())?
        {
            let (row, column) = record.pos;
            if row >= 1_048_576 || column >= 16_384 {
                return Err(Failure::document());
            }
            if !record.value.is_empty() {
                let value = Data::from(record.value).to_string();
                if !value.is_empty() {
                    minimum_size += value.len() + 3;
                    values.entry(row).or_default().insert(column, value);
                    used.insert(column);
                }
            }
            if let Some(formula) = record.formula {
                // A fill-down run may be compacted; still bound the allocation for formulas in memory.
                minimum_size += formula.len() + 3;
                formulas.entry(column).or_default().insert(row, formula);
                used.insert(column);
            }
            if minimum_size > text_limit.saturating_mul(4) {
                return Err(context_limit());
            }
        }
        if let Some(&last) = used.last() {
            let columns: Vec<_> = if last < 32 {
                (0..=last).collect()
            } else {
                used.into_iter().collect()
            };
            add_line(
                &mut output,
                &grid_header(columns.iter().copied()),
                text_limit,
            )?;
            for (row, cells) in values {
                let cells = columns
                    .iter()
                    .map(|column| cells.get(column).map(String::as_str).unwrap_or(""));
                add_line(&mut output, &grid_row(row as usize + 1, cells), text_limit)?;
            }
        }
        if !formulas.is_empty() {
            add_line(
                &mut output,
                "Formulas (shown separately; rows without stored values appear only here)",
                text_limit,
            )?;
            for (column, cells) in formulas {
                let mut cells = cells.into_iter().peekable();
                while let Some((first, formula)) = cells.next() {
                    let mut last = first;
                    while let Some((next, candidate)) = cells.peek() {
                        if *next != last + 1
                            || !continues_formula(&formula, candidate, first, *next, column)
                        {
                            break;
                        }
                        last = *next;
                        cells.next();
                    }
                    let column_name = column_name(column);
                    let start = format!("{column_name}{}", first + 1);
                    let formula = if formula.starts_with('=') {
                        formula
                    } else {
                        format!("={formula}")
                    };
                    let expression = if formula.chars().any(char::is_control) {
                        serde_json::to_string(&formula).map_err(|_| Failure::document())?
                    } else {
                        formula
                    };
                    let line = if first == last {
                        format!("{start}: {expression}")
                    } else {
                        format!(
                            "{start}:{column_name}{} (fill down from {start}): {expression}",
                            last + 1
                        )
                    };
                    add_line(&mut output, &line, text_limit)?;
                }
            }
        }
    }
    let mut parts = Vec::new();
    if !output.is_empty() {
        parts.push(text(output));
    }
    parts.extend(embedded_images(&mut archive, "xl/media/", image_limit)?);
    Ok(parts)
}

fn continues_formula(formula: &str, candidate: &str, from: u32, to: u32, column: u32) -> bool {
    // Excel structured and external references are kept verbatim.
    if formula.contains(['[', ']', '#', '@']) || formula == candidate {
        return false;
    }
    expand_shared_formula(formula, (from, column), (to, column))
        .is_ok_and(|expanded| expanded == candidate)
}

fn office_archive(bytes: &[u8]) -> Outcome<OfficeArchive<'_>> {
    let mut archive = ZipArchive::new(Cursor::new(bytes)).map_err(|_| Failure::document())?;
    if archive.len() > 2048 {
        return Err(context_limit());
    }
    let mut bytes_total = 0;
    for index in 0..archive.len() {
        let file = archive.by_index(index).map_err(|_| Failure::document())?;
        let remaining = MAX_ARCHIVE_BYTES as u64 - bytes_total;
        if file.size() > remaining {
            return Err(context_limit());
        }
        let expanded = io::copy(&mut file.take(remaining + 1), &mut io::sink())
            .map_err(|_| Failure::document())?;
        if expanded > remaining {
            return Err(context_limit());
        }
        bytes_total += expanded;
    }
    Ok(archive)
}

fn embedded_images(
    archive: &mut OfficeArchive<'_>,
    prefix: &str,
    limit: usize,
) -> Outcome<Vec<Part>> {
    let mut used = 0;
    let mut parts = Vec::new();
    for index in 0..archive.len() {
        let file = archive.by_index(index).map_err(|_| Failure::document())?;
        let Some(name) = file
            .name()
            .strip_prefix(prefix)
            .filter(|name| !name.is_empty() && !name.contains('/'))
            .map(str::to_owned)
        else {
            continue;
        };
        let remaining = (limit - used) / 4 * 3;
        if file.size() > remaining as u64 || parts.len() >= 40 {
            return Err(context_limit());
        }
        let mut bytes = Vec::new();
        file.take(remaining as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Failure::document())?;
        let media_type = match bytes.as_slice() {
            [0xff, 0xd8, 0xff, ..] => "image/jpeg",
            value if value.starts_with(b"\x89PNG\r\n\x1a\n") => "image/png",
            value if value.starts_with(b"GIF87a") || value.starts_with(b"GIF89a") => "image/gif",
            value if value.starts_with(b"RIFF") && value.get(8..12) == Some(b"WEBP") => {
                "image/webp"
            }
            _ => return Err(Failure::document()),
        };
        used += bytes.len().div_ceil(3) * 4 + media_type.len() + "data:;base64,".len();
        if used > limit {
            return Err(context_limit());
        }
        parts.push(text(format!("Embedded image: {name}")));
        parts.push(image(media_type, &bytes));
    }
    Ok(parts)
}

fn grid_header(columns: impl Iterator<Item = u32>) -> String {
    std::iter::once("Row".to_owned())
        .chain(columns.map(column_name))
        .collect::<Vec<_>>()
        .join(" | ")
}

fn grid_row<'a>(row: usize, cells: impl Iterator<Item = &'a str>) -> String {
    std::iter::once(row.to_string())
        .chain(cells.map(inline))
        .collect::<Vec<_>>()
        .join(" | ")
}

fn inline(value: &str) -> String {
    if value
        .chars()
        .any(|character| character.is_control() || matches!(character, '|' | '\\' | '"'))
        || value.trim() != value
    {
        serde_json::to_string(value)
            .expect("a string serializes as JSON")
            .replace('|', "\\u007c")
    } else {
        value.to_owned()
    }
}

fn column_name(column: u32) -> String {
    let mut index = column + 1;
    let mut letters = Vec::with_capacity(3);
    while index > 0 {
        let digit = (index - 1) % 26;
        letters.push(char::from(b'A' + digit as u8));
        index = (index - 1) / 26;
    }
    letters.into_iter().rev().collect()
}
