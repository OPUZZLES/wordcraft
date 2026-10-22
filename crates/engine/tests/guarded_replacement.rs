use serde_json::{Value, json};
use wordcraft_doc::{Document, Pos, StoryRef};
use wordcraft_engine::Session;

fn commented() -> Session {
    let mut s = Session::new(Document::from_text("Prefix β text. Strategy views show selections. End."));
    s.run("select.text", &json!({"text":"Strategy views show selections."})).unwrap();
    s.run("review.newComment", &json!({"text":"Synthetic review note"})).unwrap();
    s
}

fn args(s: &Session, expected: &str, text: &str) -> Value {
    json!({"documentId":s.document_id(), "revision":s.rev(), "story":"body", "path":[0], "expected":expected, "text":text})
}

fn state(s: &Session) -> Value {
    json!({"doc":s.doc,"selection":s.sel,"dirty":s.dirty,"undo":s.undo_depth(),"redo":s.can_redo(),"revision":s.rev(),"documentId":s.document_id(),"pending":s.pending,"columnMode":s.column_mode})
}

fn refuses(s: &mut Session, v: Value) {
    let before = state(s);
    let e = s.run("edit.replaceText", &v).unwrap_err();
    assert!(!e.to_string().contains("unknown command"), "the checked command exists: {e}");
    assert_eq!(state(s), before, "refused {v}: {e}");
}

#[test]
fn guarded_replace_keeps_comments_and_roundtrips() {
    let mut s = commented();
    let before = serde_json::to_value(&s.doc).unwrap();
    let undo = s.undo_depth();
    let request = args(&s, "End.", "Done 😀.");
    let result = s.run("edit.replaceText", &request).expect("guarded replacement should be available");
    assert_eq!(result["replaced"], 1);
    assert_eq!(s.doc.plain_text(StoryRef::Body), "Prefix β text. Strategy views show selections. Done 😀.");
    assert_eq!(s.undo_depth(), undo + 1);
    let raw = s.doc.para_at(&Pos::body(0, 0)).unwrap().text.clone();
    assert!(raw.contains("\u{fffc}Strategy views show selections.\u{fffc}"));
    assert_eq!(s.doc.comments.len(), 1);
    let mut doc = s.doc.clone();
    for _ in 0..2 {
        doc = wordcraft_docx::read(&wordcraft_docx::write(&doc).unwrap()).unwrap();
        assert_eq!(doc.plain_text(StoryRef::Body), s.doc.plain_text(StoryRef::Body));
        assert_eq!(doc.comments.len(), 1);
        assert_eq!(doc.para_at(&Pos::body(0, 0)).unwrap().text, raw);
    }
    assert!(s.undo());
    assert_eq!(serde_json::to_value(&s.doc).unwrap(), before);
}

#[test]
fn paragraph_reads_expose_the_session_tokens() {
    let mut s = commented();
    for command in ["document.paragraph", "document.inspect"] {
        let v = s.run(command, &json!({"story":"body","path":[0]})).unwrap();
        assert_eq!(v["documentId"], s.document_id());
        assert_eq!(v["revision"], s.rev());
    }
}

#[test]
fn replacements_are_literal_unique_and_independent_of_the_caret() {
    for (body, old, new, expected) in [
        ("β 😀 needle!", "needle", "Ω", "β 😀 Ω!"),
        ("a.*b and aaab", "a.*b", "literal", "literal and aaab"),
        ("left remove right", "remove ", "", "left right"),
    ] {
        let mut s = Session::new(Document::from_text(body));
        let v = args(&s, old, new);
        s.run("edit.replaceText", &v).unwrap();
        assert_eq!(s.doc.plain_text(StoryRef::Body), expected);
    }
    for (body, old) in [("repeat repeat", "repeat"), ("aaa", "aa"), ("same", "missing"), ("same", "")] {
        let mut s = Session::new(Document::from_text(body));
        let v = args(&s, old, "replacement");
        refuses(&mut s, v);
    }
}

#[test]
fn stale_reads_and_replaced_documents_are_refused() {
    let mut s = commented();
    let old = args(&s, "End.", "Done.");
    s.run("caret.docEnd", &json!({})).unwrap();
    s.run("text.insert", &json!({"text":"!","raw":true})).unwrap();
    refuses(&mut s, old);
    let old = args(&s, "End.", "Done.");
    s.set_document(s.doc.clone());
    refuses(&mut s, old);
}

#[test]
fn invalid_targets_and_marker_crossings_do_not_mutate() {
    let mut s = commented();
    for (key, value) in [
        ("documentId", Value::Null),
        ("revision", Value::Null),
        ("story", json!("not-a-story")),
        ("story", json!({"part":999})),
        ("path", json!([])),
        ("path", json!([999])),
        ("path", json!([0, 0])),
        ("expected", json!("selections.\u{fffc} End.")),
        ("text", json!("new\u{fffc}")),
        ("text", json!("new\nparagraph")),
        ("expected", json!("new\rparagraph")),
    ] {
        let mut v = args(&s, "End.", "Done.");
        v[key] = value;
        refuses(&mut s, v);
    }
    for key in ["documentId", "revision", "story", "path", "expected", "text"] {
        let mut v = args(&s, "End.", "Done.");
        v.as_object_mut().unwrap().remove(key);
        refuses(&mut s, v);
    }
}

#[test]
fn protection_and_control_locks_are_enforced_at_the_target() {
    for mode in ["readOnly", "comments", "forms"] {
        let mut s = commented();
        s.run("review.restrict", &json!({"mode":mode})).unwrap();
        let v = args(&s, "End.", "Done.");
        refuses(&mut s, v);
    }
    let mut s = Session::new(Document::from_text("Locked and Editable."));
    s.run("select.text", &json!({"text":"Editable"})).unwrap();
    s.run("review.restrict", &json!({"exception":"add"})).unwrap();
    s.run("review.restrict", &json!({"mode":"readOnly"})).unwrap();
    let v = args(&s, "Locked", "Changed");
    refuses(&mut s, v);
    let v = args(&s, "Editable", "Allowed");
    s.run("edit.replaceText", &v).unwrap();
    assert_eq!(s.doc.plain_text(StoryRef::Body), "Locked and Allowed.");
    let mut s = Session::new(Document::from_text("Locked target"));
    s.run("select.text", &json!({"text":"target"})).unwrap();
    s.run("developer.plainText", &json!({"tag":"test"})).unwrap();
    s.run("developer.properties", &json!({"control":"test","lockEdit":true})).unwrap();
    let v = args(&s, "target", "Changed");
    refuses(&mut s, v);
}

#[test]
fn tracked_replacements_can_be_rejected_and_deleted_text_is_not_a_target() {
    let mut s = commented();
    s.run("review.trackChanges", &json!({"value":true})).unwrap();
    let v = args(&s, "End.", "Done.");
    s.run("edit.replaceText", &v).unwrap();
    assert!(!s.doc.revisions.is_empty());
    s.run("review.rejectAll", &json!({})).unwrap();
    assert_eq!(s.doc.plain_text(StoryRef::Body), "Prefix β text. Strategy views show selections. End.");
    let mut s = Session::new(Document::from_text("Remove target."));
    s.run("review.trackChanges", &json!({"value":true})).unwrap();
    s.run("select.text", &json!({"text":"target"})).unwrap();
    s.run("text.delete", &json!({})).unwrap();
    let v = args(&s, "target", "replacement");
    refuses(&mut s, v);
}

#[test]
fn explicit_story_does_not_replace_a_matching_body_phrase() {
    let mut s = Session::new(Document::from_text("End."));
    s.run("insert.header", &json!({"text":"End."})).unwrap();
    let story = s.sel.focus.story;
    let path = s.sel.focus.path.clone();
    assert_ne!(story, StoryRef::Body);
    let mut v = args(&s, "End.", "Header.");
    v["story"] = json!(story);
    v["path"] = json!(path);
    s.run("edit.replaceText", &v).unwrap();
    assert_eq!(s.doc.plain_text(story), "Header.");
    assert_eq!(s.doc.plain_text(StoryRef::Body), "End.");
}

#[test]
fn placeholder_expansion_and_rectangular_context_are_refused() {
    let mut s = Session::new(Document::new());
    s.run("developer.plainText", &json!({"tag":"name"})).unwrap();
    let control = s.doc.control_ranges(StoryRef::Body)[0].clone();
    let expected = s.doc.para_at(&control.content_start()).unwrap().text[control.content_start().off..control.end.off].to_string();
    let v = args(&s, &expected, "Ada");
    refuses(&mut s, v);
    let mut s = Session::new(Document::from_text("A target."));
    s.run("select.column", &json!({"on":true})).unwrap();
    let v = args(&s, "target", "replacement");
    refuses(&mut s, v);
}

#[test]
fn pending_formatting_and_failed_forced_tracking_do_not_leak() {
    let mut s = Session::new(Document::from_text("A target."));
    s.pending = Some(wordcraft_doc::props::CharProps { bold: Some(true), ..Default::default() });
    let pending = s.pending.clone();
    let original = s.doc.para_at(&Pos::body(0, 0)).unwrap().props_of_char(2).bold;
    let v = args(&s, "target", "replacement");
    s.run("edit.replaceText", &v).unwrap();
    assert_eq!(s.pending, pending);
    assert_eq!(s.doc.para_at(&Pos::body(0, 0)).unwrap().props_of_char(2).bold, original);
    let mut s = Session::new(Document::from_text("A target."));
    s.run("select.text", &json!({"text":"target"})).unwrap();
    s.run("developer.plainText", &json!({"tag":"locked"})).unwrap();
    s.run("developer.properties", &json!({"control":"locked","lockEdit":true})).unwrap();
    // An imported document can require tracked changes without having tracking switched on.
    s.doc.settings.protection = Some("trackedChanges".into());
    s.doc.settings.track_changes = false;
    let v = args(&s, "target", "replacement");
    refuses(&mut s, v);
}

#[test]
fn protected_refusal_keeps_the_pending_undo_join() {
    let mut s = Session::new(Document::from_text("A"));
    s.run("format.bold", &json!({})).unwrap();
    let depth = s.undo_depth();
    s.doc.settings.protection = Some("readOnly".into());
    let request = args(&s, "A", "replacement");
    s.join_next_undo();
    refuses(&mut s, request);
    s.doc.settings.protection = None;
    s.run("text.insert", &json!({"text":"C", "raw":true})).unwrap();
    assert_eq!(s.undo_depth(), depth, "the refusal must not consume the pending undo join");
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(s.doc.plain_text(StoryRef::Body), "A");
}

#[test]
fn protected_refusal_keeps_macro_repeat_and_tracked_move_state() {
    let mut s = Session::new(Document::from_text("Move this sentence. Locked and Editable."));
    s.run("select.text", &json!({"text":"Editable"})).unwrap();
    s.run("review.restrict", &json!({"exception":"add"})).unwrap();
    s.run("review.trackChanges", &json!({"value":true})).unwrap();
    s.run("select.text", &json!({"text":"Move this sentence."})).unwrap();
    s.run("edit.cut", &json!({})).unwrap();
    assert!(s.pending_move.is_some());
    s.doc.settings.protection = Some("readOnly".into());
    s.run("tools.recordMacro", &json!({"name":"Refusal control"})).unwrap();
    let recording = s.recording.clone();
    let repeat = s.last_command.clone();
    let tracked_move = format!("{:?}", s.pending_move);
    let request = args(&s, "Locked", "Changed");
    refuses(&mut s, request);
    assert_eq!(s.recording, recording);
    assert_eq!(s.last_command, repeat);
    assert_eq!(format!("{:?}", s.pending_move), tracked_move);
    let request = args(&s, "Editable", "Allowed");
    s.run("edit.replaceText", &request).unwrap();
    assert_eq!(s.recording.as_ref().unwrap().1.len(), 1);
    assert_eq!(s.last_command.as_ref().unwrap().0, "edit.replaceText");
    assert!(s.pending_move.is_none());
}
