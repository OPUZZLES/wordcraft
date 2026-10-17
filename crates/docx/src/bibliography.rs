//! Word bibliography source libraries (ISO/IEC 29500-1 §22.6).
use crate::{
    DocxError,
    xml::{self, El},
};
use wordcraft_doc::Source;

/// Most bytes accepted for a standalone source library.
pub const MAX_BIBLIOGRAPHY_BYTES: usize = 16 << 20;
pub(crate) const MAX_SOURCES: usize = 10_000;
const MAX_VALUE: usize = 4_000;
const LEGACY_NS: &str = "http://schemas.microsoft.com/office/word/2004/10/bibliography";

/// Read a standalone Word Sources.xml library, not a document or a master-list synchronisation.
/// Keeps the model's supported fields only. GUID, LCID and other unsupported metadata are not
/// imported. Input is capped at 16 MiB, 32 levels, 10,000 sources and 4,000 characters per value.
pub fn read(bytes: &[u8]) -> Result<Vec<Source>, DocxError> {
    if bytes.len() > MAX_BIBLIOGRAPHY_BYTES {
        return Err(DocxError::Limit("source library exceeds 16 MiB".into()));
    }
    validate(bytes)?;
    let root = xml::parse_ns(bytes, &[("b", LEGACY_NS)])?;
    if root.name != "b:Sources" {
        return Err(DocxError::Xml("expected Word bibliography Sources root".into()));
    }
    let mut sources = Vec::new();
    for el in root.els().filter(|e| e.name == "b:Source").take(MAX_SOURCES) {
        let mut src = source(el);
        for value in [
            &mut src.tag,
            &mut src.author,
            &mut src.title,
            &mut src.year,
            &mut src.city,
            &mut src.publisher,
            &mut src.journal,
            &mut src.volume,
            &mut src.pages,
            &mut src.url,
            &mut src.editor,
            &mut src.edition,
            &mut src.issue,
            &mut src.month,
            &mut src.day,
            &mut src.accessed,
            &mut src.doi,
        ] {
            if let Some((end, _)) = value.char_indices().nth(MAX_VALUE) {
                value.truncate(end);
            }
        }
        sources.push(src);
    }
    Ok(sources)
}

/// Validate before building the existing namespace-aware tree. The streaming preflight
/// refuses DTDs before entity processing and caps depth; xml-rs checks well-formedness.
fn validate(bytes: &[u8]) -> Result<(), DocxError> {
    use quick_xml::events::Event;
    let decoded = xml::utf16_to_utf8(bytes);
    let text = decoded.as_ref().map_or(bytes, |s| s.as_bytes());
    let mut reader = quick_xml::Reader::from_reader(text);
    let mut depth = 0usize;
    loop {
        match reader.read_event().map_err(|e| DocxError::Xml(e.to_string()))? {
            Event::DocType(_) => return Err(DocxError::Xml("DTDs are not accepted in source libraries".into())),
            Event::Start(_) | Event::Empty(_) if depth >= 32 => return Err(DocxError::Limit("XML nesting too deep".into())),
            Event::Start(_) => depth += 1,
            Event::End(_) => depth = depth.saturating_sub(1),
            Event::Eof => break,
            _ => {}
        }
    }
    let reader = ::xml::reader::ParserConfig::new()
        .allow_multiple_root_elements(false)
        .max_entity_expansion_length(0)
        .max_entity_expansion_depth(0)
        .create_reader(bytes);
    for event in reader {
        let event = event.map_err(|e| DocxError::Xml(e.to_string()))?;
        match event {
            ::xml::reader::XmlEvent::StartDocument { version, .. } if version != ::xml::common::XmlVersion::Version10 => {
                return Err(DocxError::Xml("source libraries require XML 1.0".into()));
            }
            ::xml::reader::XmlEvent::StartElement { name, attributes, .. }
                if !valid_name(&name) || attributes.iter().any(|a| !valid_name(&a.name)) =>
            {
                return Err(DocxError::Xml("invalid XML qualified name".into()));
            }
            _ => {}
        }
    }
    Ok(())
}

// xml-rs checks XML Names; namespace-qualified names also require NCName components.
fn valid_name(name: &::xml::name::OwnedName) -> bool {
    let ncname = |text: &str| {
        let mut chars = text.chars();
        chars.next().is_some_and(|c| c != ':' && ::xml::common::is_name_start_char(c)) && chars.all(|c| c != ':' && ::xml::common::is_name_char(c))
    };
    ncname(&name.local_name) && name.prefix.as_deref().is_none_or(ncname)
}

/// Supported source mapping shared by package customXml and standalone source libraries.
pub(crate) fn source(src_el: &El) -> Source {
    let prefix = src_el.name.rsplit_once(':').map(|(p, _)| p);
    let named = |e: &El, local: &str| e.local() == local && e.name.rsplit_once(':').map(|(p, _)| p) == prefix;
    let child_text = |el: &El, local: &str| el.els().find(|e| named(e, local)).map(|e| e.text()).unwrap_or_default();
    let parse_persons = |parent: &El| -> String {
        let mut names = Vec::new();
        let person_els: Vec<&El> = {
            let in_lists: Vec<&El> = parent.els().filter(|e| named(e, "NameList")).flat_map(|n| n.els().filter(|e| named(e, "Person"))).collect();
            if in_lists.is_empty() { parent.els().filter(|e| named(e, "Person")).collect() } else { in_lists }
        };
        for name_el in person_els {
            let last = child_text(name_el, "Last");
            let first = child_text(name_el, "First");
            let middle = child_text(name_el, "Middle");
            let name = if !first.is_empty() && !middle.is_empty() {
                format!("{last}, {first} {middle}")
            } else if !first.is_empty() {
                format!("{last}, {first}")
            } else {
                last
            };
            if !name.is_empty() {
                names.push(name);
            }
        }
        names.join("; ")
    };

    let name = |el: &El| {
        let people = parse_persons(el);
        if people.is_empty() { child_text(el, "Corporate").trim().to_string() } else { people }
    };

    let tag = child_text(src_el, "Tag");
    let raw_kind = child_text(src_el, "SourceType");
    let kind = crate::write::from_source_type(&raw_kind);
    let (author, editor) = match src_el.els().find(|e| named(e, "Author")) {
        Some(a) => {
            let auth = a.els().find(|e| named(e, "Author")).map(name).filter(|s| !s.is_empty()).unwrap_or_else(|| {
                // A source with no person names may name a corporate author, verbatim.
                let corporate = a.els().find(|e| named(e, "CorporateAuthor")).map(|c| c.text().trim().to_string()).filter(|s| !s.is_empty());
                corporate.unwrap_or_else(|| name(a))
            });
            let ed = a.els().find(|e| named(e, "Editor")).map(name).unwrap_or_default();
            (auth, ed)
        }
        None => (String::new(), String::new()),
    };
    let editor = if editor.is_empty() { src_el.els().find(|e| named(e, "Editor")).map(name).unwrap_or_default() } else { editor };
    let title = child_text(src_el, "Title");
    let year = child_text(src_el, "Year");
    let month = child_text(src_el, "Month");
    let day = child_text(src_el, "Day");
    let city = child_text(src_el, "City");
    let publisher = child_text(src_el, "Publisher");
    let journal = child_text(src_el, "JournalName");
    let edition = child_text(src_el, "Edition");
    let volume = child_text(src_el, "Volume");
    let issue = child_text(src_el, "Issue");
    let pages = child_text(src_el, "Pages");
    let url = child_text(src_el, "URL");
    let accessed = match child_text(src_el, "Accessed") {
        a if !a.is_empty() => a,
        _ => {
            let y = child_text(src_el, "YearAccessed");
            let m = child_text(src_el, "MonthAccessed");
            let d = child_text(src_el, "DayAccessed");
            if !y.is_empty() && !m.is_empty() && !d.is_empty() {
                format!("{y}-{m}-{d}")
            } else if !y.is_empty() && !m.is_empty() {
                format!("{y}-{m}")
            } else {
                y
            }
        }
    };
    let doi = match child_text(src_el, "DOI") {
        d if !d.is_empty() => d,
        _ => child_text(src_el, "StandardNumber"),
    };

    wordcraft_doc::Source {
        tag,
        kind,
        author,
        title,
        year,
        city,
        publisher,
        journal,
        volume,
        pages,
        url,
        editor,
        edition,
        issue,
        month,
        day,
        accessed,
        doi,
    }
}
