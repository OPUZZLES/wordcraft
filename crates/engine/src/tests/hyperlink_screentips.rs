use super::*;
use crate::Selection;
use wordcraft_doc::CharProps;

fn linked() -> Session {
    let mut s = s();
    s.doc
        .insert_text(
            &Pos::body(0, 0),
            "Linked text",
            &CharProps {
                link: Some("https://example.invalid/original".into()),
                link_tooltip: Some("Original hint Ω".into()),
                bold: Some(true),
                ..Default::default()
            },
        )
        .unwrap();
    s.sel = Selection { anchor: Pos::body(0, 0), focus: Pos::body(0, 11) };
    s
}

#[test]
fn hyperlink_screentips_follow_links_through_formatting_history_and_replacement() {
    let mut s = linked();
    run(&mut s, "format.clear", json!({}));
    assert_eq!(s.doc.body[0].as_para().unwrap().props_of_char(0).link_tooltip.as_deref(), Some("Original hint Ω"));
    run(&mut s, "insert.removeLink", json!({}));
    let props = s.doc.body[0].as_para().unwrap().props_of_char(0);
    assert_eq!((props.link.as_deref(), props.link_tooltip.as_deref()), (None, None));
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(s.doc.body[0].as_para().unwrap().props_of_char(0).link_tooltip.as_deref(), Some("Original hint Ω"));
    run(&mut s, "insert.link", json!({"url":"https://example.invalid/replacement"}));
    let props = s.doc.body[0].as_para().unwrap().props_of_char(0);
    assert_eq!(props.link.as_deref(), Some("https://example.invalid/replacement"));
    assert_eq!(props.link_tooltip, None);
}

#[test]
fn hyperlink_screentips_do_not_escape_the_link_or_become_painted_formatting() {
    let mut s = linked();
    s.sel = Selection::caret(Pos::body(0, 3));
    assert_eq!(s.typing_props().link_tooltip.as_deref(), Some("Original hint Ω"));
    s.sel = Selection::caret(Pos::body(0, 11));
    assert_eq!(s.typing_props().link_tooltip, None);
    s.sel = Selection::caret(Pos::body(0, 0));
    run(&mut s, "edit.copyFormat", json!({}));
    s.doc
        .insert_text(
            &Pos::body(0, 11),
            "Other",
            &CharProps {
                link: Some("https://example.invalid/destination".into()),
                link_tooltip: Some("Destination hint".into()),
                ..Default::default()
            },
        )
        .unwrap();
    s.sel = Selection { anchor: Pos::body(0, 11), focus: Pos::body(0, 16) };
    run(&mut s, "edit.pasteFormat", json!({}));
    let props = s.doc.body[0].as_para().unwrap().props_of_char(11);
    assert_eq!(props.link.as_deref(), Some("https://example.invalid/destination"));
    assert_eq!(props.link_tooltip.as_deref(), Some("Destination hint"));
    assert_eq!(props.bold, Some(true));
}

#[test]
fn hyperlink_screentip_patch_normalizes_unlinked_format_set_and_preserves_history() {
    let mut s = s();
    run(&mut s, "text.insert", json!({"text":"Plain text"}));
    run(&mut s, "select.all", json!({}));
    run(&mut s, "format.set", json!({"props":{"linkTooltip":"Orphan hint","bold":true}}));
    let props = s.doc.body[0].as_para().unwrap().props_of_char(0);
    assert_eq!((props.link.as_deref(), props.link_tooltip.as_deref()), (None, None));
    assert_eq!(props.bold, Some(true));
    run(&mut s, "insert.link", json!({"url":"https://example.invalid/existing"}));
    run(&mut s, "format.set", json!({"props":{"linkTooltip":"Updated hint"}}));
    assert_eq!(s.doc.body[0].as_para().unwrap().props_of_char(0).link_tooltip.as_deref(), Some("Updated hint"));
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(s.doc.body[0].as_para().unwrap().props_of_char(0).link_tooltip, None);
    run(&mut s, "edit.redo", json!({}));
    assert_eq!(s.doc.body[0].as_para().unwrap().props_of_char(0).link_tooltip.as_deref(), Some("Updated hint"));
    run(&mut s, "insert.removeLink", json!({}));
    run(&mut s, "format.set", json!({"props":{"link":"","linkTooltip":"Empty target"}}));
    assert_eq!(s.doc.body[0].as_para().unwrap().props_of_char(0).link_tooltip, None);
}
