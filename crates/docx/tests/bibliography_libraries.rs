use wordcraft_docx::{MAX_BIBLIOGRAPHY_BYTES, read_bibliography_sources as read};

const NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/bibliography";

fn library(body: &str) -> String {
    format!(r#"<b:Sources xmlns:b="{NS}">{body}</b:Sources>"#)
}

#[test]
fn namespaces_and_encodings_preserve_supported_metadata() {
    let original = library(
        r#"<b:Source><b:Tag>Study</b:Tag><b:SourceType>JournalArticle</b:SourceType>
        <b:Author><b:Author><b:NameList>
        <b:Person><b:First>Zoë</b:First><b:Middle>É</b:Middle><b:Last>Müller</b:Last></b:Person>
        <b:Person><b:Last>李</b:Last><b:First>明</b:First></b:Person>
        </b:NameList></b:Author><b:Editor><b:NameList><b:Person><b:Last>Example</b:Last></b:Person></b:NameList></b:Editor></b:Author>
        <b:Title>Research &amp; exchange &#x2192;</b:Title><b:Year>2031</b:Year><b:Month>4</b:Month><b:Day>3</b:Day>
        <b:JournalName>Original journal</b:JournalName><b:Volume>7</b:Volume><b:Issue>2</b:Issue><b:Pages>11-17</b:Pages>
        <b:Publisher>Example Press</b:Publisher><b:City>Berlin</b:City><b:Edition>2</b:Edition>
        <b:URL>https://example.org/study?a=1&amp;b=2</b:URL><b:DOI>10.0000/original</b:DOI>
        <b:YearAccessed>2032</b:YearAccessed><b:MonthAccessed>5</b:MonthAccessed><b:DayAccessed>6</b:DayAccessed>
        <b:Guid>original-unsupported-guid</b:Guid><b:LCID>1031</b:LCID>
        </b:Source>
        <b:Source><b:Tag>Corporate</b:Tag><b:SourceType>Report</b:SourceType>
        <b:Author><b:CorporateAuthor>Example &amp; Co.</b:CorporateAuthor></b:Author>
        <b:Title>Corporate research</b:Title><b:StandardNumber>978-0-00</b:StandardNumber></b:Source>"#,
    );
    let expected = read(original.as_bytes()).unwrap();
    assert_eq!(expected.len(), 2);
    let s = &expected[0];
    assert_eq!(s.author, "Müller, Zoë É; 李, 明");
    assert_eq!(s.editor, "Example");
    assert_eq!(s.title, "Research & exchange →");
    assert_eq!((&s.kind, &s.year, &s.month, &s.day), (&"article".into(), &"2031".into(), &"4".into(), &"3".into()));
    assert_eq!((&s.journal, &s.volume, &s.issue, &s.pages), (&"Original journal".into(), &"7".into(), &"2".into(), &"11-17".into()));
    assert_eq!((&s.publisher, &s.city, &s.edition), (&"Example Press".into(), &"Berlin".into(), &"2".into()));
    assert_eq!(s.url, "https://example.org/study?a=1&b=2");
    assert_eq!(s.doi, "10.0000/original");
    assert_eq!(s.accessed, "2032-5-6");
    assert_eq!(expected[1].author, "Example & Co.");
    assert_eq!(expected[1].doi, "978-0-00");
    for xml in [
        original.clone(),
        original.replace(NS, "http://schemas.microsoft.com/office/word/2004/10/bibliography"),
        original.replace("xmlns:b=", "xmlns:ref=").replace("<b:", "<ref:").replace("</b:", "</ref:"),
        original.replace("xmlns:b=", "xmlns=").replace("<b:", "<").replace("</b:", "</"),
    ] {
        assert_eq!(read(xml.as_bytes()).unwrap(), expected);
        let mut utf8 = vec![0xef, 0xbb, 0xbf];
        utf8.extend_from_slice(xml.as_bytes());
        assert_eq!(read(&utf8).unwrap(), expected);
        for little in [true, false] {
            let mut utf16 = if little { vec![0xff, 0xfe] } else { vec![0xfe, 0xff] };
            for unit in xml.encode_utf16() {
                utf16.extend(if little { unit.to_le_bytes() } else { unit.to_be_bytes() });
            }
            assert_eq!(read(&utf16).unwrap(), expected);
        }
    }
}

#[test]
fn malformed_or_external_xml_is_refused_as_a_whole() {
    for bad in [
        "<b:Sources",
        r#"<b:Sources xmlns:b="http://schemas.openxmlformats.org/officeDocument/2006/bibliography"><b:Source></b:Sources>"#,
        r#"<b:Sources xmlns:b="http://schemas.openxmlformats.org/officeDocument/2006/bibliography"><b:Source>"#,
        &format!("{}<another/>", library("")),
        &format!("{}trailing text", library("")),
        &format!("{}&unknown;", library("")),
        &format!("{}<![CDATA[extra]]>", library("")),
        &library("<b:Source><b:Title>&unknown;</b:Title></b:Source>"),
        &format!(r#"<!DOCTYPE b:Sources [<!ENTITY x SYSTEM "file:///not-a-library">]>{}"#, library("<b:Source><b:Title>&x;</b:Title></b:Source>")),
        &format!(r#"<!DOCTYPE b:Sources SYSTEM "https://example.invalid/library.dtd">{}"#, library("")),
        r#"<Sources xmlns="urn:not-bibliography"/>"#,
        "<Sources/>",
    ] {
        assert!(read(bad.as_bytes()).is_err(), "accepted {bad}");
    }
    assert!(read(library("").as_bytes()).unwrap().is_empty());
}

#[test]
fn foreign_elements_cannot_replace_bibliography_values() {
    let xml = library(
        r#"<b:Source xmlns:f="urn:foreign"><b:Tag>Original</b:Tag>
        <f:Title>Wrong title</f:Title><b:Title>Right title</b:Title></b:Source>
        <f:Source xmlns:f="urn:foreign"><f:Tag>Not a source</f:Tag></f:Source>"#,
    );
    let sources = read(xml.as_bytes()).unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].title, "Right title");
}

#[test]
fn hostile_sizes_depth_and_unicode_values_are_bounded() {
    assert!(read(&vec![b' '; MAX_BIBLIOGRAPHY_BYTES + 1]).is_err());
    let deep = library(&format!("{}{}", "<b:Source>".repeat(33), "</b:Source>".repeat(33)));
    assert!(read(deep.as_bytes()).is_err());
    let long = library(&format!("<b:Source><b:Tag>X</b:Tag><b:Title>{}</b:Title></b:Source>", "🦀".repeat(4001)));
    let sources = read(long.as_bytes()).unwrap();
    assert_eq!(sources[0].title, "🦀".repeat(4000));
    let many = library(&"<b:Source><b:Tag>X</b:Tag></b:Source>".repeat(10001));
    assert_eq!(read(many.as_bytes()).unwrap().len(), 10000);
}

#[test]
fn standard_corporate_author_imports_as_author_text() {
    let xml = library(
        "<b:Source><b:Tag>Corporate</b:Tag><b:Author><b:Author><b:Corporate>Example &amp; Co.</b:Corporate></b:Author></b:Author><b:Title>Report</b:Title></b:Source>",
    );
    assert_eq!(read(xml.as_bytes()).unwrap()[0].author, "Example & Co.");
}

#[test]
fn invalid_utf8_in_cdata_is_refused() {
    let mut xml = library("<b:Source><b:Tag>X</b:Tag><b:Title><![CDATA[INVALID]]></b:Title></b:Source>").into_bytes();
    let i = xml.windows(7).position(|x| x == b"INVALID").unwrap();
    xml[i] = 0xff;
    assert!(read(&xml).is_err());
}

#[test]
fn invalid_xml_characters_are_refused() {
    for body in [
        "<b:Source><b:Title>&#x1;</b:Title></b:Source>",
        "<b:Source><b:Title>\u{1}</b:Title></b:Source>",
        "<b:Source><b:Title><![CDATA[\u{1}]]></b:Title></b:Source>",
        "<b:Source bad=\"&#x1;\"><b:Title>Title</b:Title></b:Source>",
    ] {
        assert!(read(library(body).as_bytes()).is_err(), "accepted {body:?}");
    }
}

#[test]
fn malformed_utf16_is_refused() {
    let xml = library("<b:Source><b:Title>X</b:Title></b:Source>");
    let mut bytes = vec![0xff, 0xfe];
    for c in xml.encode_utf16() {
        bytes.extend(c.to_le_bytes());
    }
    let mut odd = bytes.clone();
    odd.push(0);
    assert!(read(&odd).is_err());
    let i = bytes.windows(2).position(|x| x == [b'X', 0]).unwrap();
    bytes[i..i + 2].copy_from_slice(&0xd800u16.to_le_bytes());
    assert!(read(&bytes).is_err());
}

#[test]
fn malformed_comments_and_declarations_are_refused() {
    for xml in [
        library("") + "<!--bad--comment-->",
        library("") + r#"<?xml version="1.0"?>"#,
        library(r#"<?xml version="1.0"?><b:Source/>"#),
        r#"<?xml?><b:Sources xmlns:b="http://schemas.openxmlformats.org/officeDocument/2006/bibliography"/>"#.into(),
        r#"<?XML version="1.0"?><b:Sources xmlns:b="http://schemas.openxmlformats.org/officeDocument/2006/bibliography"/>"#.into(),
        library("<b:1Invalid/>"),
    ] {
        assert!(read(xml.as_bytes()).is_err(), "accepted {xml}");
    }
    let valid = format!(r#"<?xml version="1.0" encoding="UTF-8"?>{}<!-- valid -->"#, library(""));
    assert!(read(valid.as_bytes()).unwrap().is_empty());
}
