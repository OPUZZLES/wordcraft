//! Removing a hyperlink changes its link, not its words or other formatting (#4560).

use super::*;
use crate::Selection;
use wordcraft_doc::props::CharProps;

fn linked() -> Session {
    let mut s = Session::new(wordcraft_doc::Document::from_text("Before Café one two after"));
    run(&mut s, "select.text", json!({"text": "Café one two"}));
    run(&mut s, "insert.link", json!({"url": "https://example.org/first"}));
    run(&mut s, "select.text", json!({"text": "Café"}));
    run(&mut s, "format.bold", json!({"value": true}));
    run(&mut s, "select.text", json!({"text": "one"}));
    run(&mut s, "format.charStyle", json!({"style": "Emphasis"}));
    run(&mut s, "select.text", json!({"text": "two"}));
    run(&mut s, "format.italic", json!({"value": true}));
    run(&mut s, "select.text", json!({"text": "after"}));
    run(&mut s, "insert.link", json!({"url": "https://example.org/other"}));
    s
}

fn props(s: &Session) -> Vec<(char, CharProps)> {
    let p = s.doc.para_at(&Pos::body(0, 0)).unwrap();
    p.text.char_indices().map(|(off, c)| (c, p.props_of_char(off).clone())).collect()
}

#[test]
fn remove_hyperlink_at_caret_clears_the_whole_link() {
    // Include the first/last boundary, a Unicode character, a space and a later word.
    for off in [7, 10, 12, 14, 19, 20] {
        let mut s = linked();
        run(&mut s, "caret.set", json!({"pos": {"block": 0, "off": off}}));
        let (before, sel, depth) = (s.doc.clone(), s.sel.clone(), s.undo_depth());
        let mut expected = props(&s);
        for (_, c) in &mut expected {
            if c.link.as_deref() == Some("https://example.org/first") {
                c.link = None;
                if c.style.as_deref() == Some("Hyperlink") {
                    c.style = None;
                }
            }
        }
        run(&mut s, "insert.removeLink", json!({}));
        assert_eq!(props(&s), expected, "caret byte {off}");
        assert_eq!(s.sel, sel, "removing the link does not select its text");
        assert_eq!(s.undo_depth(), depth + 1);
        let p = s.doc.para_at(&Pos::body(0, 0)).unwrap();
        let old = before.para_at(&Pos::body(0, 0)).unwrap();
        assert_eq!((&p.text, &p.props, &p.mark), (&old.text, &old.props, &old.mark));
        run(&mut s, "edit.undo", json!({}));
        assert_eq!(s.doc, before);
        assert_eq!(s.sel, sel);
        run(&mut s, "edit.redo", json!({}));
        assert_eq!(props(&s), expected);
    }
}

#[test]
fn remove_hyperlink_keeps_explicit_selection_boundaries() {
    for reversed in [false, true] {
        let mut s = linked();
        let (a, b) = (Pos::body(0, 13), Pos::body(0, 16));
        s.sel = if reversed { Selection { anchor: b, focus: a } } else { Selection { anchor: a, focus: b } };
        let sel = s.sel.clone();
        run(&mut s, "insert.removeLink", json!({}));
        let p = s.doc.para_at(&Pos::body(0, 0)).unwrap();
        assert_eq!(p.props_of_char(7).link.as_deref(), Some("https://example.org/first"));
        assert!(p.props_of_char(13).link.is_none());
        assert_eq!(p.props_of_char(17).link.as_deref(), Some("https://example.org/first"));
        assert_eq!(s.sel, sel);
    }
}

#[test]
fn remove_hyperlink_refuses_a_lock_in_the_expanded_range() {
    let mut s = linked();
    run(&mut s, "select.text", json!({"text": "two"}));
    run(&mut s, "developer.richText", json!({"tag": "locked-tail"}));
    let end = s.doc.para_at(&Pos::body(0, 0)).unwrap().text.find(" after").unwrap();
    // A link can include a control's boundary marks. The caret is in the unlocked word,
    // but removing its entire link would also change the later locked control's content.
    s.doc
        .format_range(&Pos::body(0, 7), &Pos::body(0, end), &|c| {
            c.link = Some("https://example.org/first".into());
            c.style = Some("Hyperlink".into());
        })
        .unwrap();
    run(&mut s, "developer.properties", json!({"control": "locked-tail", "lockEdit": true}));
    run(&mut s, "caret.set", json!({"pos": {"block": 0, "off": 14}}));
    let (doc, sel, depth, redo, pending) = (s.doc.clone(), s.sel.clone(), s.undo_depth(), s.can_redo(), s.pending.clone());
    assert!(s.run("insert.removeLink", &json!({})).is_err());
    assert_eq!((&s.doc, &s.sel, s.undo_depth(), s.can_redo(), &s.pending), (&doc, &sel, depth, redo, &pending));
}

#[test]
fn remove_hyperlink_respects_document_protection() {
    let mut s = linked();
    run(&mut s, "caret.set", json!({"pos": {"block": 0, "off": 14}}));
    run(&mut s, "review.restrict", json!({"mode": "readOnly"}));
    let (doc, sel, depth) = (s.doc.clone(), s.sel.clone(), s.undo_depth());
    assert!(s.run("insert.removeLink", &json!({})).is_err());
    assert_eq!((&s.doc, &s.sel, s.undo_depth()), (&doc, &sel, depth));
}

#[test]
fn remove_hyperlink_stops_at_a_different_adjacent_target() {
    let mut s = Session::new(wordcraft_doc::Document::from_text("firstsecond"));
    for (word, url) in [("first", "https://example.org/first"), ("second", "#place")] {
        run(&mut s, "select.text", json!({"text": word}));
        run(&mut s, "insert.link", json!({"url": url}));
    }
    run(&mut s, "caret.set", json!({"pos": {"block": 0, "off": 5}}));
    run(&mut s, "insert.removeLink", json!({}));
    let p = s.doc.para_at(&Pos::body(0, 0)).unwrap();
    assert_eq!(p.props_of_char(0).link.as_deref(), Some("https://example.org/first"));
    assert!(p.props_of_char(5).link.is_none());
    assert_eq!(p.text, "firstsecond");
}
