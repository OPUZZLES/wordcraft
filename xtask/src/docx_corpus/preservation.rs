//! Bounded, opt-in checks of selected package data, before import and after save.
//! These are inventories, not a complete OOXML equivalence test: formatting, field
//! nesting/results and note-anchor positions still need their own checks.

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::rc::Rc;

use quick_xml::events::Event;
use quick_xml::name::ResolveResult;
use quick_xml::{NsReader, XmlVersion};
use sha2::{Digest, Sha256};

const PART_LIMIT: u64 = 8 * 1024 * 1024;
const TOTAL_LIMIT: u64 = 64 * 1024 * 1024;
const ELEMENT_LIMIT: usize = 100_000;
const DEPTH_LIMIT: usize = 64;
const DIRECTORY_LIMIT: u64 = 8 * 1024 * 1024;
const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const P: &str = "http://schemas.openxmlformats.org/package/2006/relationships";

#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Snapshot {
    fields: BTreeMap<String, usize>,
    notes: Vec<(String, String)>,
    references: BTreeMap<(String, String), usize>,
    mail_merge: Vec<Node>,
}

impl Snapshot {
    pub(super) fn differences(&self, saved: &Self) -> Vec<&'static str> {
        let mut changed = Vec::new();
        if self.fields != saved.fields {
            changed.push("field instructions");
        }
        if self.notes != saved.notes || self.references != saved.references {
            changed.push("note text or references");
        }
        if self.mail_merge != saved.mail_merge {
            changed.push("mail-merge settings or referenced data");
        }
        changed
    }
}

#[derive(Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
struct Node {
    name: String,
    attrs: BTreeMap<String, String>,
    text: String,
    children: Vec<Node>,
}

impl Node {
    fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.get(name).map(String::as_str)
    }

    fn visit(&self, f: &mut impl FnMut(&Self) -> Result<(), String>) -> Result<(), String> {
        f(self)?;
        for child in &self.children {
            child.visit(f)?;
        }
        Ok(())
    }
}

/// Canonical namespace names: aliases and Strict/Transitional spelling are immaterial.
fn name(ns: ResolveResult<'_>, local: &[u8]) -> Result<String, String> {
    if local.len() > 1024 {
        return Err("XML name limit".into());
    }
    let local = std::str::from_utf8(local).map_err(|e| e.to_string())?;
    let prefix = match ns {
        ResolveResult::Unbound => return Ok(local.to_string()),
        ResolveResult::Unknown(_) => return Err("unbound XML namespace prefix".into()),
        ResolveResult::Bound(ns) if ns.as_ref().len() > 1024 => return Err("XML namespace limit".into()),
        ResolveResult::Bound(ns) => match ns.as_ref() {
            n if n == W.as_bytes() || n == b"http://purl.oclc.org/ooxml/wordprocessingml/main" => "w",
            n if n == R.as_bytes() || n == b"http://purl.oclc.org/ooxml/officeDocument/relationships" => "r",
            n if n == P.as_bytes() => "p",
            n => return Ok(format!("{{{}}}{local}", std::str::from_utf8(n).map_err(|e| e.to_string())?)),
        },
    };
    Ok(format!("{prefix}:{local}"))
}

fn xml(bytes: &[u8]) -> Result<Node, String> {
    let mut reader = NsReader::from_reader(bytes);
    let mut stack: Vec<Node> = Vec::new();
    let mut root = None;
    let mut elements = 0;
    let mut name_bytes = 0usize;
    let mut buf = Vec::new();
    loop {
        buf.clear();
        let (ns, event) = reader.read_resolved_event_into(&mut buf).map_err(|e| e.to_string())?;
        match event {
            Event::Start(ref e) | Event::Empty(ref e) => {
                elements += 1;
                if elements > ELEMENT_LIMIT || stack.len() >= DEPTH_LIMIT {
                    return Err("XML element/depth limit".into());
                }
                let mut node = Node { name: name(ns, e.local_name().as_ref())?, ..Node::default() };
                name_bytes = name_bytes.saturating_add(node.name.len());
                for attr in e.attributes() {
                    let attr = attr.map_err(|e| e.to_string())?;
                    if attr.key.as_ref() == b"xmlns" || attr.key.as_ref().starts_with(b"xmlns:") {
                        continue;
                    }
                    let (ns, local) = reader.resolver_mut().resolve_attribute(attr.key);
                    let key = name(ns, local.as_ref())?;
                    name_bytes = name_bytes.saturating_add(key.len());
                    let value = attr.normalized_value(XmlVersion::Explicit1_0).map_err(|e| e.to_string())?.into_owned();
                    if name_bytes > PART_LIMIT as usize {
                        return Err("expanded XML name limit".into());
                    }
                    if node.attrs.insert(key, value).is_some() {
                        return Err("duplicate XML attribute".into());
                    }
                }
                if name_bytes > PART_LIMIT as usize {
                    return Err("expanded XML name limit".into());
                }
                if matches!(event, Event::Empty(_)) {
                    attach(node, &mut stack, &mut root)?;
                } else {
                    stack.push(node);
                }
            }
            Event::End(_) => {
                let node = stack.pop().ok_or("unmatched XML end element")?;
                attach(node, &mut stack, &mut root)?;
            }
            Event::Text(e) => {
                let text = e.xml10_content().map_err(|e| e.to_string())?;
                if let Some(node) = stack.last_mut() {
                    node.text.push_str(&text);
                } else if !text.trim().is_empty() {
                    return Err("text outside XML root".into());
                }
            }
            Event::CData(e) => {
                let node = stack.last_mut().ok_or("CDATA outside XML root")?;
                node.text.push_str(&e.decode().map_err(|e| e.to_string())?);
            }
            Event::GeneralRef(e) => {
                let text = e.decode().map_err(|e| e.to_string())?;
                let escaped = format!("&{text};");
                let value = quick_xml::escape::unescape(&escaped).map_err(|e| e.to_string())?;
                stack.last_mut().ok_or("reference outside XML root")?.text.push_str(&value);
            }
            Event::DocType(_) => return Err("DOCTYPE is not supported in preservation checks".into()),
            Event::Eof => break,
            _ => {}
        }
    }
    if !stack.is_empty() {
        return Err("unclosed XML element".into());
    }
    root.ok_or_else(|| "missing XML root".into())
}

fn attach(node: Node, stack: &mut [Node], root: &mut Option<Node>) -> Result<(), String> {
    if let Some(parent) = stack.last_mut() {
        parent.children.push(node);
    } else if root.replace(node).is_some() {
        return Err("multiple XML roots".into());
    }
    Ok(())
}

fn u16_at(bytes: &[u8], at: usize) -> Result<u16, String> {
    match bytes.get(at..at.saturating_add(2)) {
        Some([a, b]) => Ok(u16::from_le_bytes([*a, *b])),
        _ => Err("truncated ZIP metadata".into()),
    }
}

fn u32_at(bytes: &[u8], at: usize) -> Result<u32, String> {
    match bytes.get(at..at.saturating_add(4)) {
        Some([a, b, c, d]) => Ok(u32::from_le_bytes([*a, *b, *c, *d])),
        _ => Err("truncated ZIP metadata".into()),
    }
}

/// Support ordinary single-disk ZIP32 packages. Reject unsupported/ambiguous end records
/// before zip allocates its central-directory entries or deduplicates their names.
fn preflight_zip(file: &mut File) -> Result<ZipBounds, String> {
    let size = file.metadata().map_err(|e| e.to_string())?.len();
    let tail_len = size.min(65_557);
    file.seek(SeekFrom::End(-(tail_len as i64))).map_err(|e| e.to_string())?;
    let mut tail = vec![0u8; tail_len as usize];
    file.read_exact(&mut tail).map_err(|e| e.to_string())?;
    let end = tail.windows(4).rposition(|w| w == b"PK\x05\x06").ok_or("ZIP end record missing")?;
    let record = tail.get(end..).ok_or("ZIP end record missing")?;
    if 22usize.saturating_add(usize::from(u16_at(record, 20)?)) != record.len() {
        return Err("unsupported or ambiguous ZIP end record".into());
    }
    if end >= 20 && tail.get(end - 20..end - 16) == Some(b"PK\x06\x07") {
        return Err("ZIP64 preservation checks are not supported".into());
    }
    let count = usize::from(u16_at(record, 10)?);
    if count > 20_000 {
        return Err("preservation ZIP entry limit".into());
    }
    if u16_at(record, 4)? != 0 || u16_at(record, 6)? != 0 || usize::from(u16_at(record, 8)?) != count {
        return Err("multi-disk ZIP is not supported".into());
    }
    let directory_size = u64::from(u32_at(record, 12)?);
    let directory_start = u64::from(u32_at(record, 16)?);
    let end_position = size - tail_len + end as u64;
    if directory_size > DIRECTORY_LIMIT {
        return Err("preservation ZIP directory limit".into());
    }
    if directory_start.checked_add(directory_size) != Some(end_position) {
        return Err("unsupported ZIP directory offsets".into());
    }
    file.seek(SeekFrom::Start(directory_start)).map_err(|e| e.to_string())?;
    let mut directory = vec![0u8; directory_size as usize];
    file.read_exact(&mut directory).map_err(|e| e.to_string())?;
    if directory.windows(4).any(|w| w == b"PK\x05\x06") {
        return Err("ambiguous ZIP directory end signature".into());
    }
    let mut names = BTreeSet::new();
    let mut pos = 0usize;
    for _ in 0..count {
        let header = directory.get(pos..pos.saturating_add(46)).ok_or("truncated ZIP directory")?;
        if header.get(..4) != Some(b"PK\x01\x02") {
            return Err("invalid ZIP directory record".into());
        }
        let name_start = pos + 46;
        let name_end = name_start + usize::from(u16_at(header, 28)?);
        let next = name_end + usize::from(u16_at(header, 30)?) + usize::from(u16_at(header, 32)?);
        let name = directory.get(name_start..name_end).ok_or("truncated ZIP part name")?;
        if name.is_empty() || !names.insert(name) {
            return Err("empty or duplicate package part name".into());
        }
        if next > directory.len() {
            return Err("truncated ZIP entry metadata".into());
        }
        pos = next;
    }
    if pos != directory.len() {
        return Err("ZIP directory count/size mismatch".into());
    }
    file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    Ok(ZipBounds { count, directory_start })
}

struct ZipBounds {
    count: usize,
    directory_start: u64,
}

/// During metadata construction, hide older end records in the payload. zip may retry
/// earlier EOCDs on an invalid directory; none except the preflighted one may be visible.
struct MetadataReader {
    file: File,
    directory_start: u64,
    metadata_only: Rc<Cell<bool>>,
}

impl Read for MetadataReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let pos = self.file.stream_position()?;
        if self.metadata_only.get() && pos < self.directory_start {
            let n = (self.directory_start - pos).min(buf.len() as u64) as usize;
            buf.get_mut(..n).ok_or_else(|| std::io::Error::other("invalid metadata read"))?.fill(0);
            self.file.seek(SeekFrom::Start(pos + n as u64))?;
            Ok(n)
        } else {
            self.file.read(buf)
        }
    }
}

impl Seek for MetadataReader {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.file.seek(pos)
    }
}

struct Package {
    zip: zip::ZipArchive<MetadataReader>,
    total: u64,
    compressed_total: u64,
}

impl Package {
    fn open(path: &Path) -> Result<Self, String> {
        let mut file = File::open(path).map_err(|e| e.to_string())?;
        let bounds = preflight_zip(&mut file)?;
        let metadata_only = Rc::new(Cell::new(true));
        let reader = MetadataReader { file, directory_start: bounds.directory_start, metadata_only: Rc::clone(&metadata_only) };
        let config = zip::read::Config { archive_offset: zip::read::ArchiveOffset::Known(0) };
        let zip = zip::ZipArchive::with_config(config, reader).map_err(|e| e.to_string())?;
        if zip.len() != bounds.count {
            return Err("duplicate decoded package names".into());
        }
        metadata_only.set(false);
        Ok(Self { zip, total: 0, compressed_total: 0 })
    }

    fn read(&mut self, part: &str) -> Result<Option<Vec<u8>>, String> {
        let entry = match self.zip.by_name(part) {
            Ok(entry) => entry,
            Err(zip::result::ZipError::FileNotFound) => return Ok(None),
            Err(e) => return Err(e.to_string()),
        };
        if entry.size() > PART_LIMIT || entry.compressed_size() > PART_LIMIT {
            return Err(format!("{part}: above the 8 MiB preservation part limit"));
        }
        self.compressed_total = self.compressed_total.saturating_add(entry.compressed_size());
        if self.compressed_total > TOTAL_LIMIT {
            return Err("preservation compressed read limit".into());
        }
        let mut bytes = Vec::new();
        entry.take(PART_LIMIT + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        self.total = self.total.saturating_add(bytes.len() as u64);
        if bytes.len() as u64 > PART_LIMIT || self.total > TOTAL_LIMIT {
            return Err("preservation package read limit".into());
        }
        Ok(Some(bytes))
    }

    fn required(&mut self, part: &str) -> Result<Vec<u8>, String> {
        self.read(part)?.ok_or_else(|| format!("missing package part {part}"))
    }

    fn rels(&mut self, part: &str) -> Result<BTreeMap<String, Rel>, String> {
        let (dir, file) = part.rsplit_once('/').unwrap_or(("", part));
        let path = if part.is_empty() { "_rels/.rels".to_string() } else { format!("{dir}/_rels/{file}.rels").trim_start_matches('/').to_string() };
        let Some(bytes) = self.read(&path)? else { return Ok(BTreeMap::new()) };
        let root = xml(&bytes)?;
        if root.name != "p:Relationships" {
            return Err(format!("{path}: not OPC relationships"));
        }
        let mut rels = BTreeMap::new();
        for node in &root.children {
            if node.name != "p:Relationship" {
                continue;
            }
            let id = node.attr("Id").ok_or("relationship without Id")?.to_string();
            let kind = node.attr("Type").ok_or("relationship without Type")?.to_string();
            let target = node.attr("Target").ok_or("relationship without Target")?.to_string();
            let external = match node.attr("TargetMode") {
                None | Some("Internal") => false,
                Some("External") => true,
                _ => return Err("invalid relationship TargetMode".into()),
            };
            if rels.insert(id, Rel { kind, target, external }).is_some() {
                return Err("duplicate relationship Id".into());
            }
        }
        Ok(rels)
    }
}

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect()
}

struct Rel {
    kind: String,
    target: String,
    external: bool,
}

impl Rel {
    fn is(&self, kind: &str) -> bool {
        self.kind == format!("{R}/{kind}") || self.kind == format!("http://purl.oclc.org/ooxml/officeDocument/relationships/{kind}")
    }
}

fn target(owner: &str, rel: &Rel) -> Result<String, String> {
    if rel.external || rel.target.contains(['\\', '?', '#', '%', ':']) {
        return Err("unsupported internal package target".into());
    }
    let mut parts: Vec<&str> =
        if rel.target.starts_with('/') { Vec::new() } else { owner.rsplit_once('/').map(|(d, _)| d.split('/').collect()).unwrap_or_default() };
    for part in rel.target.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop().ok_or("package target escapes root")?;
            }
            part => parts.push(part),
        }
    }
    if parts.is_empty() { Err("empty package target".into()) } else { Ok(parts.join("/")) }
}

/// Inspect package bytes, without importing through WordCraft or following external links.
pub(super) fn read(path: &Path) -> Result<Snapshot, String> {
    let mut package = Package::open(path)?;
    let roots = package.rels("")?;
    let mains: Vec<_> = roots.values().filter(|r| r.is("officeDocument")).collect();
    let [main] = mains.as_slice() else { return Err("expected one main document relationship".into()) };
    let main = target("", main)?;
    let main_rels = package.rels(&main)?;
    let body = xml(&package.required(&main)?)?;
    if body.name != "w:document" {
        return Err("main part is not a Word document".into());
    }
    let mut stories = vec![("body", body)];
    let mut snapshot = Snapshot::default();
    let mut notes = BTreeMap::new();
    let mut seen = BTreeSet::new();
    for rel in main_rels.values() {
        if rel.is("settings") {
            let part = target(&main, rel)?;
            let settings = xml(&package.required(&part)?)?;
            if settings.name != "w:settings" {
                return Err("invalid settings root".into());
            }
            let rels = package.rels(&part)?;
            let mut resolved = BTreeMap::new();
            for mut merge in settings.children.into_iter().filter(|n| n.name == "w:mailMerge") {
                resolve_merge(&mut merge, &part, &rels, &mut package, &mut resolved)?;
                snapshot.mail_merge.push(merge);
            }
        } else if let Some(kind) = ["header", "footer", "footnotes", "endnotes"].into_iter().find(|k| rel.is(k)) {
            let part = target(&main, rel)?;
            if !seen.insert((kind, part.clone())) {
                continue;
            }
            let root = xml(&package.required(&part)?)?;
            let expected_root = match kind {
                "header" => "w:hdr",
                "footer" => "w:ftr",
                "footnotes" => "w:footnotes",
                _ => "w:endnotes",
            };
            if root.name != expected_root {
                return Err(format!("invalid {kind} root"));
            }
            if kind == "footnotes" || kind == "endnotes" {
                let note_kind = if kind == "footnotes" { "w:footnote" } else { "w:endnote" };
                for note in root.children.iter().filter(|n| n.name == note_kind && matches!(n.attr("w:type"), None | Some("normal"))) {
                    let id = note.attr("w:id").ok_or("note without id")?.to_string();
                    let text = note_text(note)?;
                    snapshot.notes.push((kind.to_string(), text.clone()));
                    if notes.insert((kind.to_string(), id), digest(text.as_bytes())).is_some() {
                        return Err("duplicate note id".into());
                    }
                }
            }
            stories.push((kind, root));
        }
    }
    for (_, story) in &stories {
        let mut stack = Vec::new();
        fields(story, &mut stack, &mut snapshot.fields)?;
        if !stack.is_empty() {
            return Err("unclosed complex field".into());
        }
        story.visit(&mut |node| {
            let kind = match node.name.as_str() {
                "w:footnoteReference" => "footnotes",
                "w:endnoteReference" => "endnotes",
                _ => return Ok(()),
            };
            let id = node.attr("w:id").ok_or("note reference without id")?;
            let text = notes.get(&(kind.to_string(), id.to_string())).ok_or("dangling note reference")?;
            *snapshot.references.entry((kind.to_string(), text.clone())).or_default() += 1;
            Ok(())
        })?;
    }
    snapshot.notes.sort();
    snapshot.mail_merge.sort();
    Ok(snapshot)
}

fn resolve_merge(
    node: &mut Node,
    owner: &str,
    rels: &BTreeMap<String, Rel>,
    package: &mut Package,
    resolved: &mut BTreeMap<String, String>,
) -> Result<(), String> {
    // Ignore indentation, never field/query text. Names/attributes already have canonical namespaces.
    if node.text.trim().is_empty() {
        node.text.clear();
    }
    if let Some(id) = node.attrs.get_mut("r:id") {
        let signature = if let Some(signature) = resolved.get(id) {
            signature.clone()
        } else {
            let rel = rels.get(id).ok_or("mail-merge relationship missing")?;
            let kind = match rel.kind.strip_prefix("http://purl.oclc.org/ooxml/officeDocument/relationships/") {
                Some(kind) => format!("{R}/{kind}"),
                None => rel.kind.clone(),
            };
            let value = if rel.external {
                serde_json::to_vec(&(&kind, "external", &rel.target))
            } else {
                let bytes = package.required(&target(owner, rel)?)?;
                serde_json::to_vec(&(&kind, "internal-sha256", digest(&bytes)))
            }
            .map_err(|e| e.to_string())?;
            let signature = digest(&value);
            resolved.insert(id.clone(), signature.clone());
            signature
        };
        *id = signature;
    }
    for child in &mut node.children {
        resolve_merge(child, owner, rels, package, resolved)?;
    }
    Ok(())
}

fn note_text(node: &Node) -> Result<String, String> {
    let mut out = String::new();
    fn walk(node: &Node, out: &mut String) -> Result<(), String> {
        match node.name.as_str() {
            "w:t" => out.push_str(&node.text),
            "w:tab" | "w:ptab" => out.push('\t'),
            "w:br" | "w:cr" => out.push('\n'),
            "w:noBreakHyphen" => out.push('\u{2011}'),
            "w:softHyphen" => out.push('\u{ad}'),
            "w:sym" => {
                let ch = node.attr("w:char").and_then(|s| u32::from_str_radix(s, 16).ok()).and_then(char::from_u32).ok_or("invalid note symbol")?;
                out.push(ch);
            }
            _ => {
                for child in &node.children {
                    walk(child, out)?;
                }
            }
        }
        if node.name == "w:p" {
            out.push('\n');
        }
        Ok(())
    }
    walk(node, &mut out)?;
    Ok(out)
}

#[derive(Default)]
struct Field {
    instruction: String,
    separated: bool,
}

fn fields(node: &Node, stack: &mut Vec<Field>, out: &mut BTreeMap<String, usize>) -> Result<(), String> {
    if node.name == "w:fldSimple" {
        let instruction = node.attr("w:instr").ok_or("simple field without instruction")?;
        *out.entry(instruction.trim().to_string()).or_default() += 1;
    } else if node.name == "w:fldChar" {
        match node.attr("w:fldCharType") {
            Some("begin") => stack.push(Field::default()),
            Some("separate") => stack.last_mut().ok_or("unmatched field separator")?.separated = true,
            Some("end") => {
                let field = stack.pop().ok_or("unmatched field end")?;
                *out.entry(field.instruction.trim().to_string()).or_default() += 1;
            }
            _ => return Err("invalid complex field marker".into()),
        }
    } else if node.name == "w:instrText" {
        let field = stack.last_mut().filter(|f| !f.separated).ok_or("instruction outside field code")?;
        field.instruction.push_str(&node.text);
    }
    for child in &node.children {
        fields(child, stack, out)?;
    }
    Ok(())
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use std::io::Write;

    pub(in crate::docx_corpus) fn fixture(field: bool, note: &str, merge: Option<(&str, &str, &[u8])>, alias: bool) -> Vec<u8> {
        let mut entries = BTreeMap::new();
        let rels = |s: &str| format!(r#"<Relationships xmlns="{P}">{s}</Relationships>"#);
        let rel = |id: &str, kind: &str, target: &str, mode: &str| {
            format!(r#"<Relationship Id="{id}" Type="{R}/{kind}" Target="{target}" {mode}/ >"#).replace("/ >", "/>")
        };
        entries.insert("_rels/.rels", rels(&rel("main", "officeDocument", "custom/body.xml", "")).into_bytes());
        let id = if alias { "99" } else { "3" };
        let content =
            if field { r#"<w:fldSimple w:instr=" MERGEFIELD Name "><w:r><w:t>Ada</w:t></w:r></w:fldSimple>"# } else { "<w:r><w:t>Ada</w:t></w:r>" };
        entries.insert(
            "custom/body.xml",
            format!(r#"<w:document xmlns:w="{W}"><w:body><w:p>{content}<w:r><w:footnoteReference w:id="{id}"/></w:r></w:p></w:body></w:document>"#)
                .into_bytes(),
        );
        entries.insert("custom/note.xml", format!(r#"<w:footnotes xmlns:w="{W}"><w:footnote w:id="{id}"><w:p><w:r><w:footnoteRef/><w:t>{note}</w:t></w:r></w:p></w:footnote></w:footnotes>"#).into_bytes());
        let mut links = rel("note", "footnotes", "note.xml", "");
        if let Some((mode, target, data)) = merge {
            links.push_str(&rel("settings", "settings", "settings.xml", ""));
            let rid = if alias { "newId" } else { "oldId" };
            entries.insert("custom/settings.xml", format!(r#"<w:settings xmlns:w="{W}" xmlns:r="{R}"><w:mailMerge><w:mainDocumentType w:val="formLetters"/><w:dataSource r:id="{rid}"/></w:mailMerge></w:settings>"#).into_bytes());
            let mode_attr = format!(r#"TargetMode="{mode}""#);
            entries.insert("custom/_rels/settings.xml.rels", rels(&rel(rid, "mailMergeSource", target, &mode_attr)).into_bytes());
            if mode == "Internal" {
                let path = format!("custom/{target}");
                // Test fixture paths have static lifetimes below.
                let key = if path.ends_with("data2.bin") { "custom/data2.bin" } else { "custom/data.bin" };
                entries.insert(key, data.to_vec());
            }
        }
        entries.insert("custom/_rels/body.xml.rels", rels(&links).into_bytes());
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, mut data) in entries {
            if alias && name.ends_with(".xml") {
                data = String::from_utf8(data).unwrap().replace("xmlns:w=", "xmlns:x=").replace("w:", "x:").into_bytes();
            }
            zip.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
            zip.write_all(&data).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    fn snapshot(bytes: &[u8]) -> Result<Snapshot, String> {
        let temp = super::super::TempDir::new().unwrap();
        let path = temp.path.join("case.docx");
        std::fs::write(&path, bytes).unwrap();
        read(&path)
    }

    #[test]
    fn aliases_note_ids_and_relationship_ids_do_not_change_the_inventory() {
        let merge = Some(("External", "file:///synthetic.csv", &b""[..]));
        let original = snapshot(&fixture(true, "Original note.", merge, false)).unwrap();
        let renamed = snapshot(&fixture(true, "Original note.", merge, true)).unwrap();
        assert!(original.differences(&renamed).is_empty());
        assert_eq!(original.fields.get("MERGEFIELD Name"), Some(&1));
        assert_eq!(original.references.get(&("footnotes".into(), digest(b"Original note.\n"))), Some(&1));
    }

    #[test]
    fn flattening_a_field_or_changing_a_note_is_visible_without_body_text_changes() {
        let original = snapshot(&fixture(true, "Original note.", None, false)).unwrap();
        let flattened = snapshot(&fixture(false, "Original note.", None, false)).unwrap();
        assert_eq!(original.differences(&flattened), ["field instructions"]);
        let changed = snapshot(&fixture(true, "Changed note.", None, false)).unwrap();
        assert_eq!(original.differences(&changed), ["note text or references"]);
    }

    #[test]
    fn merge_removal_retargeting_and_internal_data_changes_are_detected() {
        let external = snapshot(&fixture(true, "Note", Some(("External", "file:///synthetic.csv", b"")), false)).unwrap();
        let removed = snapshot(&fixture(true, "Note", None, false)).unwrap();
        let retargeted = snapshot(&fixture(true, "Note", Some(("External", "file:///other.csv", b"")), false)).unwrap();
        assert_eq!(external.differences(&removed), ["mail-merge settings or referenced data"]);
        assert_eq!(external.differences(&retargeted), ["mail-merge settings or referenced data"]);
        let internal = snapshot(&fixture(true, "Note", Some(("Internal", "data.bin", b"original")), false)).unwrap();
        let renamed = snapshot(&fixture(true, "Note", Some(("Internal", "data2.bin", b"original")), true)).unwrap();
        let changed = snapshot(&fixture(true, "Note", Some(("Internal", "data.bin", b"changed")), false)).unwrap();
        assert!(internal.differences(&renamed).is_empty());
        assert_eq!(internal.differences(&changed), ["mail-merge settings or referenced data"]);
    }

    #[test]
    fn split_complex_instructions_and_nested_fields_are_counted() {
        let root = xml(format!(r#"<w:p xmlns:w="{W}"><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText> DOCPROPERTY </w:instrText></w:r><w:r><w:instrText>Name </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:fldSimple w:instr="PAGE"><w:r><w:t>1</w:t></w:r></w:fldSimple><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#).as_bytes()).unwrap();
        let mut out = BTreeMap::new();
        let mut stack = Vec::new();
        fields(&root, &mut stack, &mut out).unwrap();
        assert!(stack.is_empty());
        assert_eq!(out, BTreeMap::from([("DOCPROPERTY Name".into(), 1), ("PAGE".into(), 1)]));
    }

    #[test]
    fn malformed_or_oversized_parts_are_check_errors() {
        assert!(snapshot(b"not a zip").is_err());
        for source in ["<x:a/>", "<a><b></a>", "<a/><b/>", "<!DOCTYPE a><a/>"] {
            assert!(xml(source.as_bytes()).is_err(), "{source}");
        }
        let nested = format!("{}{}", "<a>".repeat(DEPTH_LIMIT + 1), "</a>".repeat(DEPTH_LIMIT + 1));
        assert!(xml(nested.as_bytes()).is_err());
        let too_many = format!("<a>{}</a>", "<b/>".repeat(ELEMENT_LIMIT));
        assert!(xml(too_many.as_bytes()).is_err());
        let large = "x".repeat(PART_LIMIT as usize);
        assert!(snapshot(&fixture(true, &large, None, false)).unwrap_err().contains("limit"));
    }

    #[test]
    fn namespace_expansion_has_a_budget_before_allocating_names() {
        let uri = "u".repeat(2048);
        assert!(xml(format!(r#"<a xmlns:q="{uri}"><q:b/></a>"#).as_bytes()).unwrap_err().contains("namespace limit"));
        let uri = "u".repeat(512);
        let input = format!(r#"<a xmlns:q="{uri}">{}</a>"#, "<q:b/>".repeat(20_000));
        assert!(xml(input.as_bytes()).unwrap_err().contains("expanded XML name limit"));
    }

    #[test]
    fn note_character_elements_are_not_silently_dropped() {
        let root = xml(format!(r#"<w:p xmlns:w="{W}"><w:r><w:t>well</w:t><w:noBreakHyphen/><w:t>known</w:t><w:softHyphen/><w:sym w:char="2605"/><w:tab/><w:br/></w:r></w:p>"#).as_bytes()).unwrap();
        assert_eq!(note_text(&root).unwrap(), "well\u{2011}known\u{ad}\u{2605}\t\n\n");
        let root = xml(format!(r#"<w:p xmlns:w="{W}"><w:r><w:sym w:char="invalid"/></w:r></w:p>"#).as_bytes()).unwrap();
        assert!(note_text(&root).is_err());
    }

    fn preflight(bytes: &[u8]) -> Result<usize, String> {
        let temp = super::super::TempDir::new().unwrap();
        let path = temp.path.join("case.docx");
        std::fs::write(&path, bytes).unwrap();
        preflight_zip(&mut File::open(path).unwrap()).map(|bounds| bounds.count)
    }

    #[test]
    fn zip_metadata_is_bounded_before_archive_construction() {
        let original = fixture(true, "Note", None, false);
        assert_eq!(preflight(&original).unwrap(), 4);
        let end = original.len() - 22;
        let mut count = original.clone();
        count[end + 8..end + 10].copy_from_slice(&20_001u16.to_le_bytes());
        count[end + 10..end + 12].copy_from_slice(&20_001u16.to_le_bytes());
        assert!(preflight(&count).unwrap_err().contains("entry limit"));
        let mut metadata = original.clone();
        metadata[end + 12..end + 16].copy_from_slice(&((DIRECTORY_LIMIT + 1) as u32).to_le_bytes());
        assert!(preflight(&metadata).unwrap_err().contains("directory limit"));
        let mut ambiguous = original.clone();
        ambiguous[end + 20..end + 22].copy_from_slice(&4u16.to_le_bytes());
        ambiguous.extend_from_slice(b"PK\x05\x06");
        assert!(preflight(&ambiguous).is_err());
    }

    #[test]
    fn duplicate_zip_directory_names_are_rejected_before_deduplication() {
        let mut bytes = fixture(true, "Note", None, false);
        let end = bytes.len() - 22;
        let start = u32_at(&bytes, end + 16).unwrap() as usize;
        let size = u32_at(&bytes, end + 12).unwrap() as usize;
        let count = u16_at(&bytes, end + 10).unwrap();
        let first_len = 46
            + usize::from(u16_at(&bytes, start + 28).unwrap())
            + usize::from(u16_at(&bytes, start + 30).unwrap())
            + usize::from(u16_at(&bytes, start + 32).unwrap());
        let duplicate = bytes[start..start + first_len].to_vec();
        let mut record = bytes.split_off(end);
        bytes.extend_from_slice(&duplicate);
        record[8..10].copy_from_slice(&(count + 1).to_le_bytes());
        record[10..12].copy_from_slice(&(count + 1).to_le_bytes());
        record[12..16].copy_from_slice(&((size + first_len) as u32).to_le_bytes());
        bytes.extend_from_slice(&record);
        // zip 3's filename map hides the duplicate, so checking len/file_names afterward is too late.
        assert_eq!(zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap().len(), usize::from(count));
        assert!(preflight(&bytes).unwrap_err().contains("duplicate"));
        assert!(snapshot(&bytes).unwrap_err().contains("duplicate"));
    }

    fn rewrite(bytes: &[u8], mut change: impl FnMut(&str, &[u8]) -> Vec<u8>) -> Vec<u8> {
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index).unwrap();
            let name = entry.name().to_string();
            let mut data = Vec::new();
            entry.read_to_end(&mut data).unwrap();
            writer.start_file(&name, zip::write::SimpleFileOptions::default()).unwrap();
            writer.write_all(&change(&name, &data)).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn repeated_merge_relationships_use_cached_fixed_size_fingerprints() {
        let target = format!("https://example.invalid/{}", "x".repeat(4096));
        let original = fixture(true, "Note", Some(("External", &target, b"")), false);
        let bytes = rewrite(&original, |name, data| {
            if name == "custom/settings.xml" {
                String::from_utf8(data.to_vec())
                    .unwrap()
                    .replace("</w:mailMerge>", &format!("{}</w:mailMerge>", r#"<w:dataSource r:id="oldId"/>"#.repeat(1000)))
                    .into_bytes()
            } else {
                data.to_vec()
            }
        });
        let snapshot = snapshot(&bytes).unwrap();
        let ids: Vec<_> = snapshot.mail_merge[0].children.iter().filter_map(|n| n.attr("r:id")).collect();
        assert_eq!(ids.len(), 1001);
        assert!(ids.iter().all(|id| id.len() == 64 && *id == ids[0]));
    }

    #[test]
    fn a_bad_final_directory_cannot_fall_back_to_an_earlier_archive() {
        let mut bytes = fixture(true, "Note", None, false);
        let end = bytes.len() - 22;
        let start = u32_at(&bytes, end + 16).unwrap() as usize;
        let mut directory = bytes[start..end].to_vec();
        let new_start = bytes.len();
        // Valid raw record sizes, but the first local entry points into the new directory.
        directory[42..46].copy_from_slice(&(new_start as u32).to_le_bytes());
        let mut record = bytes[end..].to_vec();
        record[16..20].copy_from_slice(&(new_start as u32).to_le_bytes());
        bytes.extend_from_slice(&directory);
        bytes.extend_from_slice(&record);
        assert_eq!(preflight(&bytes).unwrap(), 4);
        // The unguarded library retries and accepts the older directory.
        assert_eq!(zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap().len(), 4);
        assert!(snapshot(&bytes).is_err());
    }

    /// One decoded byte behind many legal empty DEFLATE stored blocks.
    fn padded_deflate(empty_blocks: usize) -> (Vec<u8>, usize) {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        writer.start_file("data.bin", zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored)).unwrap();
        writer.write_all(b"x").unwrap();
        let original = writer.finish().unwrap().into_inner();
        let end = original.len() - 22;
        let directory_start = u32_at(&original, end + 16).unwrap() as usize;
        let data_start = 30 + usize::from(u16_at(&original, 26).unwrap()) + usize::from(u16_at(&original, 28).unwrap());
        let mut bytes = original[..data_start].to_vec();
        bytes[8..10].copy_from_slice(&8u16.to_le_bytes());
        let compressed = empty_blocks * 5 + 6;
        bytes[18..22].copy_from_slice(&(compressed as u32).to_le_bytes());
        for _ in 0..empty_blocks {
            bytes.extend_from_slice(&[0, 0, 0, 255, 255]);
        }
        bytes.extend_from_slice(&[1, 1, 0, 254, 255, b'x']);
        let new_start = bytes.len();
        let mut directory = original[directory_start..end].to_vec();
        directory[10..12].copy_from_slice(&8u16.to_le_bytes());
        directory[20..24].copy_from_slice(&(compressed as u32).to_le_bytes());
        bytes.extend_from_slice(&directory);
        let mut record = original[end..].to_vec();
        record[16..20].copy_from_slice(&(new_start as u32).to_le_bytes());
        bytes.extend_from_slice(&record);
        (bytes, compressed)
    }

    #[test]
    fn compressed_input_is_bounded_even_when_decoded_output_is_one_byte() {
        let temp = super::super::TempDir::new().unwrap();
        let path = temp.path.join("padded.zip");
        let (large, _) = padded_deflate(PART_LIMIT as usize / 5 + 1);
        std::fs::write(&path, large).unwrap();
        let mut package = Package::open(&path).unwrap();
        assert!(package.read("data.bin").unwrap_err().contains("part limit"));
        let (padded, compressed) = padded_deflate(PART_LIMIT as usize / 10);
        let path = temp.path.join("repeated.zip");
        std::fs::write(&path, padded).unwrap();
        let mut package = Package::open(&path).unwrap();
        for _ in 0..TOTAL_LIMIT as usize / compressed {
            assert_eq!(package.read("data.bin").unwrap().unwrap(), b"x");
        }
        assert!(package.read("data.bin").unwrap_err().contains("compressed read limit"));
    }
}
