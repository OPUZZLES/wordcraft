//! Original OOXML fixtures for imported picture effects; no files produced by Word.
use std::io::{Read, Write};
fn fixture() -> Vec<u8> {
    let image = image::RgbaImage::from_fn(240, 160, |x, y| image::Rgba([30 + (x / 3) as u8, 60 + (y / 2) as u8, 180, 255]));
    let mut png = std::io::Cursor::new(Vec::new());
    image.write_to(&mut png, image::ImageFormat::Png).unwrap();
    let mut body = String::new();
    for (i, rad) in [None, Some(0), Some(76_200), Some(317_500)].into_iter().enumerate() {
        let effect = rad.map(|v| format!(r#"<a:effectLst><a:softEdge rad="{v}"/></a:effectLst>"#)).unwrap_or_default();
        body.push_str(&format!(r#"<w:p><w:r><w:drawing><wp:inline><wp:extent cx="1828800" cy="1219200"/><wp:docPr id="{}" name="Original picture"/><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic><pic:nvPicPr><pic:cNvPr id="0" name="synthetic.png"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip r:embed="rIdImage"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="1828800" cy="1219200"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom>{effect}</pic:spPr></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"#, i+1));
    }
    let document = format!(
        r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"><w:body>{body}<w:sectPr><w:pgSz w:w="8640" w:h="14400"/><w:pgMar w:top="720" w:bottom="720" w:left="720" w:right="720"/></w:sectPr></w:body></w:document>"#
    );
    let rels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdImage" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/synthetic.png"/></Relationships>"#;
    let root = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdMain" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;
    let types = r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="png" ContentType="image/png"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#;
    let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, bytes) in [
        ("[Content_Types].xml", types.as_bytes()),
        ("_rels/.rels", root.as_bytes()),
        ("word/document.xml", document.as_bytes()),
        ("word/_rels/document.xml.rels", rels.as_bytes()),
        ("word/media/synthetic.png", png.get_ref().as_slice()),
    ] {
        z.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
        z.write_all(bytes).unwrap();
    }
    z.finish().unwrap().into_inner()
}
#[test]
fn imported_picture_soft_edges_survive_two_saves() {
    let mut bytes = fixture();
    let original = wordcraft_docx::read(&bytes).unwrap();
    let original_media = original.media.values().next().unwrap().clone();
    for save in 1..=2 {
        let doc = wordcraft_docx::read(&bytes).unwrap();
        assert_eq!(doc.body.iter().filter_map(|b| b.as_para()).flat_map(|p| &p.objects).count(), 4);
        let radii: Vec<_> = doc
            .body
            .iter()
            .filter_map(|b| b.as_para())
            .flat_map(|p| &p.objects)
            .filter_map(|o| match o {
                wordcraft_doc::InlineObject::Image { soft_edge, .. } => Some(*soft_edge),
                _ => None,
            })
            .collect();
        assert_eq!(radii, vec![None, None, Some(6.0), Some(25.0)]);
        assert_eq!(doc.media.len(), 1, "effects belong to the picture, not duplicate media");
        assert_eq!(doc.media.values().next().unwrap().as_slice(), original_media.as_slice());
        bytes = wordcraft_docx::write(&doc).unwrap();
        let mut z = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
        let mut xml = String::new();
        z.by_name("word/document.xml").unwrap().read_to_string(&mut xml).unwrap();
        assert!(xml.contains(r#"<a:softEdge rad="76200""#), "save {save}: missing imported 6pt soft edge");
        assert!(xml.contains(r#"<a:softEdge rad="317500""#), "save {save}: missing imported 25pt soft edge");
        assert_eq!(xml.matches("<a:softEdge ").count(), 2, "zero and absent must stay plain");
    }
}

fn change_xml(input: &[u8], change: impl Fn(String) -> String) -> Vec<u8> {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(input)).unwrap();
    let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for i in 0..z.len() {
        let mut part = z.by_index(i).unwrap();
        let mut bytes = Vec::new();
        part.read_to_end(&mut bytes).unwrap();
        if part.name() == "word/document.xml" {
            bytes = change(String::from_utf8(bytes).unwrap()).into_bytes();
        }
        out.start_file(part.name(), zip::write::SimpleFileOptions::default()).unwrap();
        out.write_all(&bytes).unwrap();
    }
    out.finish().unwrap().into_inner()
}

#[test]
fn picture_soft_edge_radius_is_bounded_and_only_read_from_picture_properties() {
    for (value, expected) in [
        ("-12700", None),
        ("NaN", None),
        ("", None),
        ("0", None),
        ("9223372036854775807", Some(wordcraft_doc::effects::MAX_SOFT_EDGE)),
        ("9223372036854775808", None),
    ] {
        let bytes = change_xml(&fixture(), |xml| xml.replace(r#"rad="76200""#, &format!(r#"rad="{value}""#)));
        let doc = wordcraft_docx::read(&bytes).unwrap();
        let o = doc.body[2].as_para().unwrap().objects.first().unwrap();
        assert!(matches!(o, wordcraft_doc::InlineObject::Image { soft_edge, .. } if *soft_edge == expected), "{value}: {o:?}");
        let saved = wordcraft_docx::write(&doc).unwrap();
        let back = wordcraft_docx::read(&saved).unwrap();
        assert_eq!(doc.body, back.body);
    }
    let bytes = change_xml(&fixture(), |xml| {
        xml.replace(r#"<a:effectLst><a:softEdge rad="76200"/></a:effectLst>"#, "")
            .replace(r#"<a:blip r:embed="rIdImage"/>"#, r#"<a:blip r:embed="rIdImage"><a:softEdge rad="76200"/></a:blip>"#)
    });
    let doc = wordcraft_docx::read(&bytes).unwrap();
    assert!(matches!(doc.body[2].as_para().unwrap().objects[0], wordcraft_doc::InlineObject::Image { soft_edge: None, .. }));
}

#[test]
fn picture_soft_edges_preserve_crop_rotation_and_media_on_reopen() {
    let mut bytes = change_xml(&fixture(), |xml| {
        xml.replace("<a:stretch>", r#"<a:srcRect l="12500" t="25000" r="5000" b="0"/><a:stretch>"#)
            .replace("<a:xfrm>", r#"<a:xfrm rot="2700000" flipH="1">"#)
    });
    let source = wordcraft_docx::read(&bytes).unwrap();
    for _ in 0..2 {
        bytes = wordcraft_docx::write(&wordcraft_docx::read(&bytes).unwrap()).unwrap();
        let doc = wordcraft_docx::read(&bytes).unwrap();
        let radii: Vec<_> = doc
            .body
            .iter()
            .filter_map(|b| b.as_para())
            .flat_map(|p| &p.objects)
            .map(|o| match o {
                wordcraft_doc::InlineObject::Image { soft_edge, .. } => *soft_edge,
                _ => None,
            })
            .collect();
        assert_eq!(radii, vec![None, None, Some(6.0), Some(25.0)]);
        assert_eq!(source.media, doc.media);
        for o in doc.body.iter().filter_map(|b| b.as_para()).flat_map(|p| &p.objects) {
            assert!(matches!(o, wordcraft_doc::InlineObject::Image { crop, float, .. }
                if *crop == [0.125, 0.25, 0.05, 0.0] && float.rot == 45.0 && float.flip_h));
        }
    }
}
