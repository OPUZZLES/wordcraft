use serde_json::json;
use wordcraft_engine::Session;

#[test]
fn word_xml_sources_import_save_reopen_and_undo() {
    let root = std::env::temp_dir().join(format!("wordcraft-bibxml-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("Sources.xml");
    std::fs::write(
        &path,
        r#"<b:Sources xmlns:b="http://schemas.openxmlformats.org/officeDocument/2006/bibliography">
      <b:Source><b:Tag>Original2031</b:Tag><b:SourceType>Book</b:SourceType>
        <b:Author><b:Author><b:NameList><b:Person><b:Last>Müller</b:Last><b:First>Zoë</b:First></b:Person></b:NameList></b:Author></b:Author>
        <b:Title>Original research &amp; exchange</b:Title><b:Year>2031</b:Year><b:Publisher>Example Press</b:Publisher>
      </b:Source>
    </b:Sources>"#,
    )
    .unwrap();
    let mut s = Session::new(wordcraft_doc::Document::from_text("Report"));
    let before = s.doc.clone();
    let result = s.run("references.importSources", &json!({"path": path})).expect("Word XML source library should import");
    assert_eq!(result["added"], 1);
    assert_eq!(s.doc.sources.len(), 1);
    assert_eq!(s.doc.sources[0].author, "Müller, Zoë");
    assert_eq!(s.doc.sources[0].title, "Original research & exchange");
    let saved = wordcraft_docx::write(&s.doc).unwrap();
    let reopened = wordcraft_docx::read(&saved).unwrap();
    assert_eq!(reopened.sources, s.doc.sources);
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(s.doc.sources, before.sources);
    assert_eq!(s.doc.plain_text(wordcraft_doc::StoryRef::Body), before.plain_text(wordcraft_doc::StoryRef::Body));
    std::fs::remove_dir_all(root).unwrap();
}

const NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/bibliography";
struct Library(std::path::PathBuf);
impl Library {
    fn new(name: &str, text: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("wordcraft-bibxml-{name}-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, text).unwrap();
        Self(path)
    }
}
impl Drop for Library {
    fn drop(&mut self) {
        std::fs::remove_dir_all(self.0.parent().unwrap()).unwrap();
    }
}
fn xml(year: &str) -> String {
    format!(
        r#"<b:Sources xmlns:b="{NS}"><b:Source><b:Tag>Original</b:Tag><b:SourceType>Book</b:SourceType>
        <b:Author><b:Author><b:NameList><b:Person><b:Last>Example</b:Last></b:Person></b:NameList></b:Author></b:Author>
        <b:Title>Original research</b:Title><b:Year>{year}</b:Year></b:Source></b:Sources>"#
    )
}

#[test]
fn importing_again_reuses_tags_updates_citations_and_is_one_undo() {
    let file = Library::new("repeat.xml", &xml("2031"));
    let mut s = Session::new(wordcraft_doc::Document::new());
    let import = |s: &mut Session| s.run("references.importSources", &json!({"path": file.0})).unwrap();
    assert_eq!(import(&mut s)["added"], 1);
    s.run("references.citation", &json!({"tag": "Original"})).unwrap();
    let sources = s.doc.sources.clone();
    let text = s.doc.plain_text(wordcraft_doc::StoryRef::Body);
    assert!(text.contains("2031"), "{text}");
    std::fs::write(&file.0, xml("2032")).unwrap();
    let result = import(&mut s);
    assert_eq!((result["added"].as_u64(), result["updated"].as_u64(), result["total"].as_u64()), (Some(0), Some(1), Some(1)));
    assert_eq!(s.doc.sources.len(), 1);
    assert_eq!(s.doc.sources[0].year, "2032");
    assert!(s.doc.plain_text(wordcraft_doc::StoryRef::Body).contains("2032"));
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(s.doc.sources, sources);
    assert_eq!(s.doc.plain_text(wordcraft_doc::StoryRef::Body), text);
}

#[test]
fn a_refused_library_preserves_content_selection_dirty_and_undo() {
    let file = Library::new("invalid.xml", &(xml("2031") + "<broken>"));
    let mut s = Session::new(wordcraft_doc::Document::from_text("Seed"));
    s.run("text.insert", &json!({"text": "One undo"})).unwrap();
    let doc = serde_json::to_value(&s.doc).unwrap();
    let sel = serde_json::to_value(&s.sel).unwrap();
    let dirty = s.dirty;
    assert!(s.run("references.importSources", &json!({"path": file.0})).is_err());
    assert_eq!(serde_json::to_value(&s.doc).unwrap(), doc);
    assert_eq!(serde_json::to_value(&s.sel).unwrap(), sel);
    assert_eq!(s.dirty, dirty);
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(s.doc.plain_text(wordcraft_doc::StoryRef::Body), "Seed");
}

#[test]
fn automatic_and_explicit_xml_import_reuse_the_same_transaction() {
    assert_eq!(wordcraft_docx::MAX_BIBLIOGRAPHY_BYTES, wordcraft_formats::MAX_LIBRARY_BYTES);
    for (name, format) in [("sniffed.data", "auto"), ("forced.data", "word-xml"), ("upper.XML", "auto")] {
        let file = Library::new(name, &xml("2031"));
        let mut s = Session::new(wordcraft_doc::Document::new());
        let result = s.run("references.importSources", &json!({"path": file.0, "format": format})).unwrap();
        assert_eq!(result["added"], 1);
        assert_eq!(s.doc.sources[0].tag, "Original");
    }
}
