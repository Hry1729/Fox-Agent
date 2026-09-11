//! Spreadsheet extraction preserves sheet names and cell addresses. Shared
//! string IDs are resolved before text reaches the model; formulas never run.
use super::{
    append_decoded_xml_text, append_text_section, parse_error, ImportError, SafeZipArchive,
    MAX_EXTRACTED_TEXT_BYTES, MAX_XML_DEPTH,
};
use quick_xml::{
    events::{BytesStart, Event},
    Reader, XmlVersion,
};
use std::collections::BTreeMap;

#[derive(Default)]
struct Item {
    attributes: BTreeMap<String, String>,
    text: String,
    value: String,
    formula: Option<String>,
}

fn item_attributes(event: &BytesStart<'_>, reader: &Reader<&[u8]>) -> Result<Item, ImportError> {
    let mut item = Item::default();
    for attribute in event.attributes() {
        let attribute = attribute.map_err(|_| parse_error("invalid OOXML attribute"))?;
        let key = if attribute.key.local_name().as_ref() == b"id"
            && attribute.key.as_ref().contains(&b':')
        {
            "relationshipId".to_owned()
        } else {
            String::from_utf8_lossy(attribute.key.local_name().as_ref()).into_owned()
        };
        let value = attribute
            .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
            .map_err(|_| parse_error("invalid OOXML attribute value"))?
            .into_owned();
        item.attributes.insert(key, value);
    }
    Ok(item)
}

fn items(xml: &[u8], target: &[u8]) -> Result<Vec<Item>, ImportError> {
    let mut reader = Reader::from_reader(xml);
    let mut stack = Vec::<Vec<u8>>::new();
    let mut result = Vec::new();
    let mut current: Option<Item> = None;
    let mut text_bytes = 0usize;
    loop {
        match reader
            .read_event()
            .map_err(|_| parse_error("invalid OOXML XML"))?
        {
            Event::Start(event) => {
                let name = event.local_name().as_ref().to_vec();
                if stack.len() >= MAX_XML_DEPTH {
                    return Err(parse_error("OOXML nesting limit exceeded"));
                }
                if name == target {
                    current = Some(item_attributes(&event, &reader)?);
                }
                if name == b"f" {
                    if let Some(item) = &mut current {
                        item.formula = Some(String::new());
                    }
                }
                stack.push(name);
            }
            Event::Empty(event) => {
                if event.local_name().as_ref() == target {
                    result.push(item_attributes(&event, &reader)?);
                }
                if event.local_name().as_ref() == b"f" {
                    if let Some(item) = &mut current {
                        item.formula = Some(String::new());
                    }
                }
            }
            Event::End(event) => {
                if event.local_name().as_ref() == target {
                    if let Some(item) = current.take() {
                        result.push(item);
                    }
                }
                stack.pop();
            }
            Event::Text(event) => {
                let text = event
                    .decode()
                    .map_err(|_| parse_error("invalid OOXML text encoding"))?;
                append_fragment(&mut current, &stack, &text, &mut text_bytes)?;
            }
            Event::CData(event) => {
                let text = event
                    .decode()
                    .map_err(|_| parse_error("invalid OOXML text encoding"))?;
                append_fragment(&mut current, &stack, &text, &mut text_bytes)?;
            }
            Event::GeneralRef(event) => {
                let name = event
                    .decode()
                    .map_err(|_| parse_error("invalid OOXML entity"))?;
                let mut decoded = String::new();
                append_decoded_xml_text(&mut decoded, &format!("&{name};"))?;
                append_fragment(&mut current, &stack, &decoded, &mut text_bytes)?;
            }
            Event::DocType(_) => return Err(parse_error("OOXML document types are not supported")),
            Event::Eof => break,
            _ => {}
        }
        if result.len() > 100_000 {
            return Err(parse_error("OOXML element limit exceeded"));
        }
    }
    if !stack.is_empty() {
        return Err(parse_error("truncated OOXML XML"));
    }
    Ok(result)
}

fn append_fragment(
    current: &mut Option<Item>,
    stack: &[Vec<u8>],
    text: &str,
    count: &mut usize,
) -> Result<(), ImportError> {
    let Some(item) = current else {
        return Ok(());
    };
    let Some(name) = stack.last() else {
        return Ok(());
    };
    // Phonetic guides are annotations, not a second copy of the cell value.
    if stack.iter().any(|name| name == b"rPh") {
        return Ok(());
    }
    let destination = match name.as_slice() {
        b"t" => &mut item.text,
        b"v" => &mut item.value,
        b"f" => item.formula.get_or_insert_with(String::new),
        _ => return Ok(()),
    };
    *count = count.saturating_add(text.len());
    if *count > MAX_EXTRACTED_TEXT_BYTES {
        return Err(parse_error("OOXML text limit exceeded"));
    }
    destination.push_str(text);
    Ok(())
}

pub(super) fn xlsx_text(archive: &SafeZipArchive<'_>) -> Result<String, ImportError> {
    xlsx_text_with_outline(archive).map(|(text, _, _)| text)
}

pub(super) fn xlsx_text_with_outline(archive: &SafeZipArchive<'_>) -> Result<(String, Vec<super::AttachmentSheetOutline>, usize), ImportError> {
    let shared = archive
        .read_entry("xl/sharedStrings.xml")?
        .map(|xml| items(&xml, b"si"))
        .transpose()?
        .unwrap_or_default();
    let mut sheets = Vec::<(String, String)>::new();
    if let Some(workbook) = archive.read_entry("xl/workbook.xml")? {
        let relationships = archive
            .read_entry("xl/_rels/workbook.xml.rels")?
            .ok_or_else(|| parse_error("XLSX workbook relationships are missing"))?;
        let relationships = items(&relationships, b"Relationship")?;
        for sheet in items(&workbook, b"sheet")? {
            let id = sheet
                .attributes
                .get("relationshipId")
                .ok_or_else(|| parse_error("XLSX sheet relationship ID is missing"))?;
            let relation = relationships
                .iter()
                .find(|item| item.attributes.get("Id") == Some(id))
                .ok_or_else(|| parse_error("XLSX sheet relationship is missing"))?;
            if relation
                .attributes
                .get("TargetMode")
                .is_some_and(|value| value == "External")
            {
                return Err(parse_error(
                    "external XLSX sheet relationships are not supported",
                ));
            }
            let target = relation
                .attributes
                .get("Target")
                .ok_or_else(|| parse_error("XLSX sheet target is missing"))?;
            let target = if target.starts_with('/') {
                target.trim_start_matches('/').to_owned()
            } else {
                format!("xl/{target}")
            };
            if !target.starts_with("xl/worksheets/") || target.split('/').any(|part| part == "..") {
                // Chartsheets have no worksheet cells and are explicitly skipped.
                if target.starts_with("xl/chartsheets/") {
                    continue;
                }
                return Err(parse_error("unsupported XLSX worksheet target"));
            }
            sheets.push((
                sheet
                    .attributes
                    .get("name")
                    .cloned()
                    .unwrap_or_else(|| id.clone()),
                target,
            ));
        }
    } else {
        // Minimal producer files may omit the workbook manifest.
        let mut names = archive
            .entry_names()
            .filter(|name| name.starts_with("xl/worksheets/sheet") && name.ends_with(".xml"))
            .map(str::to_owned)
            .collect::<Vec<_>>();
        names.sort_by_key(|name| {
            name.strip_prefix("xl/worksheets/sheet")
                .and_then(|s| s.strip_suffix(".xml"))
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(u32::MAX)
        });
        sheets.extend(names.into_iter().map(|name| (name.clone(), name)));
    }
    if sheets.is_empty() {
        return Err(parse_error("XLSX contains no worksheet XML"));
    }
    let mut output = String::new();
    append_text_section(&mut output, "Spreadsheet cell values (formulas use saved cached values, not recalculated; numeric cells retain raw stored values, including date serials).")?;
    let sheet_count = sheets.len();
    let mut outline = Vec::new();
    let mut characters = output.chars().count();
    for (name, path) in sheets {
        let xml = archive
            .read_entry(&path)?
            .ok_or_else(|| parse_error("XLSX worksheet XML is missing"))?;
        let previous_end = output.len();
        append_text_section(&mut output, &format!("[Sheet: {name}]"))?;
        let header_start = previous_end + output[previous_end..].find('[').unwrap_or(0);
        let offset = characters + output[previous_end..header_start].chars().count();
        for cell in items(&xml, b"c")? {
            let address = cell
                .attributes
                .get("r")
                .map(String::as_str)
                .unwrap_or("(unspecified cell)");
            let cell_type = cell.attributes.get("t").map(String::as_str).unwrap_or("n");
            let value = match cell_type {
                "s" => {
                    let index = cell
                        .value
                        .trim()
                        .parse::<usize>()
                        .map_err(|_| parse_error("invalid XLSX shared string index"))?;
                    shared
                        .get(index)
                        .ok_or_else(|| parse_error("XLSX shared string index is out of range"))?
                        .text
                        .clone()
                }
                "inlineStr" => cell.text,
                "b" => {
                    if cell.value.trim() == "1" {
                        "TRUE".into()
                    } else if cell.value.trim() == "0" {
                        "FALSE".into()
                    } else {
                        return Err(parse_error("invalid XLSX boolean value"));
                    }
                }
                "e" => format!("[Cell error: {}]", cell.value),
                _ => cell.value,
            };
            if let Some(formula) = cell.formula {
                append_text_section(
                    &mut output,
                    &format!(
                        "{address}: {} [formula: ={formula}; saved result, not recalculated]",
                        if value.is_empty() {
                            "(no cached value)"
                        } else {
                            &value
                        }
                    ),
                )?;
            } else if !value.is_empty() {
                append_text_section(&mut output, &format!("{address}: {value}"))?;
            }
        }
        // Derive the directory from the workbook manifest, never from cell text.
        // Bound metadata independently of the extracted-text and ZIP limits.
        if outline.len() < 64 {
            outline.push(super::AttachmentSheetOutline {
                name: name.chars().take(256).collect(),
                name_truncated: name.chars().count() > 256,
                offset,
                preview: output[header_start..].chars().take(400).collect(),
            });
        }
        characters += output[previous_end..].chars().count();
    }
    Ok((output, outline, sheet_count))
}

pub(super) fn pptx_text(archive: &SafeZipArchive<'_>) -> Result<String, ImportError> {
    let mut slides = Vec::new();
    if let Some(presentation) = archive.read_entry("ppt/presentation.xml")? {
        let relationships = archive
            .read_entry("ppt/_rels/presentation.xml.rels")?
            .ok_or_else(|| parse_error("PPTX presentation relationships are missing"))?;
        let relationships = items(&relationships, b"Relationship")?;
        for slide in items(&presentation, b"sldId")? {
            let id = slide
                .attributes
                .get("relationshipId")
                .ok_or_else(|| parse_error("PPTX slide relationship ID is missing"))?;
            let relation = relationships
                .iter()
                .find(|item| item.attributes.get("Id") == Some(id))
                .ok_or_else(|| parse_error("PPTX slide relationship is missing"))?;
            if relation
                .attributes
                .get("TargetMode")
                .is_some_and(|value| value == "External")
            {
                return Err(parse_error(
                    "external PPTX slide relationships are not supported",
                ));
            }
            let target = relation
                .attributes
                .get("Target")
                .ok_or_else(|| parse_error("PPTX slide target is missing"))?;
            let path = if target.starts_with('/') {
                target.trim_start_matches('/').to_owned()
            } else {
                format!("ppt/{target}")
            };
            if !path.starts_with("ppt/slides/") || path.split('/').any(|part| part == "..") {
                return Err(parse_error("unsupported PPTX slide target"));
            }
            slides.push(path);
        }
    } else {
        slides = archive
            .entry_names()
            .filter(|name| name.starts_with("ppt/slides/slide") && name.ends_with(".xml"))
            .map(str::to_owned)
            .collect();
        slides.sort_by_key(|name| {
            name.strip_prefix("ppt/slides/slide")
                .and_then(|s| s.strip_suffix(".xml"))
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(u32::MAX)
        });
    }
    if slides.is_empty() {
        return Err(parse_error("PPTX contains no slide XML"));
    }
    let mut output = String::new();
    for (index, path) in slides.iter().enumerate() {
        let xml = archive
            .read_entry(path)?
            .ok_or_else(|| parse_error("PPTX slide XML is missing"))?;
        append_text_section(
            &mut output,
            &format!(
                "[Slide {}]\n{}",
                index + 1,
                super::extract_xml_text(&xml, &["t"])?
            ),
        )?;
    }
    Ok(output)
}
