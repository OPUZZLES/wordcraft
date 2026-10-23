//! Phonetic Guide takes typing on opening, without changing the selected document until OK.

use egui_kittest::{Harness, kittest::Queryable};
use serde_json::json;
use wordcraft_doc::Document;
use wordcraft_engine::{Session, cmd::phonetic};
use wordcraft_ui_egui::{Services, WordApp, dialogs::Dialog};

fn harness(theme: &str, existing: bool) -> Harness<'static, WordApp> {
    let mut app = WordApp::new(Session::new(Document::from_text("Reading remains")), Services::default());
    app.ui.language = "en".into();
    app.run("ui.theme", json!({"value": theme})).unwrap();
    app.run("select.text", json!({"text": "Reading"})).unwrap();
    if existing {
        app.run("format.phonetic", json!({"ruby": "reed", "align": "left", "size": 7, "offset": 2})).unwrap();
    }
    let mut h = Harness::builder().with_size(egui::vec2(1440.0, 900.0)).build_ui_state(
        |ui, app: &mut WordApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.run_steps(6);
    assert!(h.state().canvas.focused, "the document initially has the keyboard");
    h
}

fn open(h: &mut Harness<'_, WordApp>) {
    h.get_by_label("Phonetic Guide").click();
    h.run_steps(3);
    assert!(matches!(h.state().dialog, Some(Dialog::Phonetic { .. })));
}

fn ruby<'a>(h: &'a Harness<'_, WordApp>) -> &'a str {
    match &h.state().dialog {
        Some(Dialog::Phonetic { ruby, .. }) => ruby,
        _ => panic!("Phonetic Guide is open"),
    }
}

#[test]
fn phonetic_guide_takes_typing_on_each_open_without_stealing_later_focus() {
    for theme in ["light", "dark"] {
        let mut h = harness(theme, false);
        let before = serde_json::to_value(&h.state().session.doc).unwrap();
        let selection = h.state().session.sel.clone();
        for escape in [false, true] {
            open(&mut h);
            h.event(egui::Event::Text("reed-ing".into()));
            h.run_steps(2);
            assert_eq!(ruby(&h), "reed-ing", "typing after opening goes to Ruby text ({theme})");
            assert_eq!(serde_json::to_value(&h.state().session.doc).unwrap(), before);
            assert_eq!(h.state().session.sel, selection);

            let first = h.ctx.memory(|m| m.focused()).unwrap();
            h.key_press(egui::Key::Tab);
            h.run_steps(2);
            let next = h.ctx.memory(|m| m.focused()).unwrap();
            assert_ne!(first, next, "Tab leaves Ruby text");
            h.run_steps(3);
            assert_eq!(h.ctx.memory(|m| m.focused()), Some(next), "later frames keep the new focus");
            h.key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::Tab);
            h.run_steps(2);
            assert_eq!(h.ctx.memory(|m| m.focused()), Some(first), "Shift+Tab returns to Ruby text");
            if escape {
                h.key_press(egui::Key::Escape);
            } else {
                h.get_by_label("Cancel").click();
            }
            h.run_steps(3);
            assert!(h.state().dialog.is_none());
            assert_eq!(serde_json::to_value(&h.state().session.doc).unwrap(), before, "cancel leaves the document untouched");
            assert_eq!(h.state().session.sel, selection);
        }
    }
}

#[test]
fn phonetic_guide_preserves_existing_values_until_ok_and_round_trips() {
    let mut h = harness("light", true);
    let before = serde_json::to_value(&h.state().session.doc).unwrap();
    let selection = h.state().session.sel.clone();
    open(&mut h);
    assert_eq!(ruby(&h), "reed", "opening does not replace the existing guide");
    let Some(Dialog::Phonetic { align, size, offset, .. }) = &h.state().dialog else { panic!("Phonetic Guide is open") };
    assert_eq!((align.as_str(), *size, *offset), ("left", 7.0, 2.0));
    h.event(egui::Event::Text("!".into()));
    h.run_steps(2);
    assert_eq!(ruby(&h), "reed!");
    assert_eq!(serde_json::to_value(&h.state().session.doc).unwrap(), before);
    assert_eq!(h.state().session.sel, selection);
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert!(h.state().dialog.is_none());
    assert_eq!(phonetic::state(&h.state().session)["ruby"], "reed!");
    assert_eq!(phonetic::state(&h.state().session)["base"], "Reading");
    h.state_mut().run("edit.undo", json!({})).unwrap();
    assert_eq!(serde_json::to_value(&h.state().session.doc).unwrap(), before);
    h.state_mut().run("edit.redo", json!({})).unwrap();
    assert_eq!(phonetic::state(&h.state().session)["ruby"], "reed!");

    let bytes = wordcraft_engine::io::save_bytes("guide.docx", &h.state().session.doc).unwrap();
    let doc = wordcraft_engine::io::open_bytes("guide.docx", &bytes).unwrap();
    let mut fresh = WordApp::new(Session::new(doc), Services::default());
    fresh.run("select.range", json!({"anchor": {"block": 0, "off": 0}, "focus": {"block": 0, "off": 0}})).unwrap();
    assert_eq!(phonetic::state(&fresh.session)["ruby"], "reed!");
    assert_eq!(phonetic::state(&fresh.session)["base"], "Reading");
    open(&mut h);
    assert_eq!(ruby(&h), "reed!");
    h.event(egui::Event::Text("?".into()));
    h.run_steps(2);
    assert_eq!(ruby(&h), "reed!?");
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    assert_eq!(phonetic::state(&h.state().session)["ruby"], "reed!", "Escape discards the reopened edit");
}
