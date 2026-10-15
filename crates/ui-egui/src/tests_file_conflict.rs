//! Save-conflict behavior at the UI dispatch boundary, with original synthetic files only.
#![allow(clippy::unwrap_used)]
use super::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::channel;
use wordcraft_doc::Document;
use wordcraft_engine::io;

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!("wordcraft-ui-conflict-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir(&p).unwrap();
        Self(p)
    }
    fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn app(d: &Scratch) -> WordApp {
    let p = d.file("original.docx");
    io::save_path(&p, &Document::from_text("Original")).unwrap();
    let mut a = WordApp::new(Session::new(Document::new()), Services::default());
    a.execute("file.open", json!({"path": p})).unwrap();
    a.execute("document.setText", json!({"text": "Local edit"})).unwrap();
    a
}
fn external(d: &Scratch) -> Vec<u8> {
    let b = io::save_bytes("external.docx", &Document::from_text("External version")).unwrap();
    std::fs::write(d.file("original.docx"), &b).unwrap();
    b
}
fn assert_local(a: &WordApp, d: &Scratch, b: &[u8]) {
    assert!(a.session.dirty);
    assert_eq!(a.session.doc.plain_text(wordcraft_doc::StoryRef::Body), "Local edit");
    assert_eq!(std::fs::read(d.file("original.docx")).unwrap(), b);
}
#[test]
fn scripts_get_error_user_gets_dialog_and_keep_editing_retains_both_versions() {
    let d = Scratch::new();
    let mut a = app(&d);
    let b = external(&d);
    assert!(a.execute("file.save", json!({})).is_err());
    assert!(a.dialog.is_none());
    assert!(a.run("file.save", json!({})).is_err());
    assert!(matches!(a.dialog, Some(dialogs::Dialog::SaveConflict { .. })));
    assert_local(&a, &d, &b);
    assert!(a.run("ui.saveConflict", json!({"answer": "overwrite"})).is_err());
    assert!(a.dialog.is_some());
    a.run("ui.saveConflict", json!({"answer": "cancel"})).unwrap();
    assert!(a.dialog.is_none());
    assert_local(&a, &d, &b);
}
#[test]
fn save_copy_picker_success_cancel_and_changed_document_are_bound_to_the_conflict() {
    for mode in ["copy", "cancel", "new-document", "same-path"] {
        let d = Scratch::new();
        let mut a = app(&d);
        let b = external(&d);
        let (tx, rx) = channel();
        let rx = std::cell::RefCell::new(Some(rx));
        a.services.file_dialog = Some(Box::new(move |request| {
            assert!(matches!(request, file_dialogs::FileDialogRequest::Save { name } if name == "original copy.docx"));
            rx.borrow_mut().take().unwrap()
        }));
        assert!(a.run("file.save", json!({})).is_err());
        a.run("ui.saveConflict", json!({"answer": "copy"})).unwrap();
        assert!(a.file_dialog_open());
        let copy = d.file("copy.docx");
        if mode == "new-document" {
            a.execute("file.new", json!({})).unwrap();
        }
        tx.send(Ok(match mode {
            "cancel" => None,
            "same-path" => Some(d.file("original.docx").display().to_string()),
            _ => Some(copy.display().to_string()),
        }))
        .unwrap();
        let result = a.poll_file_dialog().unwrap();
        if mode == "copy" {
            result.unwrap();
            assert!(!a.session.dirty);
            assert_eq!(a.session.path.as_ref(), Some(&copy));
            assert_eq!(io::open_path(&copy).unwrap().plain_text(wordcraft_doc::StoryRef::Body), "Local edit");
        } else {
            assert!(!copy.exists());
            if mode == "cancel" {
                result.unwrap();
                assert_local(&a, &d, &b);
            } else {
                assert!(result.is_err());
            }
            if mode == "same-path" {
                assert_local(&a, &d, &b);
                assert!(matches!(a.dialog, Some(dialogs::Dialog::SaveConflict { .. })));
            }
        }
        assert_eq!(std::fs::read(d.file("original.docx")).unwrap(), b);
    }
}
#[test]
fn autosave_pauses_after_conflict_and_pending_close_does_not_discard_edits() {
    let d = Scratch::new();
    let mut a = app(&d);
    a.execute("file.save", json!({})).unwrap();
    a.execute("document.setText", json!({"text": "Local edit"})).unwrap();
    let b = external(&d);
    a.autosave_tick(now_ms() + 10_000.0);
    assert!(!a.autosaves());
    assert!(matches!(a.dialog, Some(dialogs::Dialog::SaveConflict { .. })));
    assert_local(&a, &d, &b);
    a.run("ui.saveConflict", json!({"answer": "cancel"})).unwrap();
    a.autosave_tick(now_ms() + 20_000.0);
    assert!(a.dialog.is_none());
    assert_eq!(a.run("file.close", json!({})).unwrap()["pending"], "saveChanges");
    let error = a.run("ui.saveChanges", json!({"answer": "save"})).unwrap_err();
    assert!(error.contains("file changed or is unavailable"));
    assert!(
        matches!(&a.dialog, Some(dialogs::Dialog::SaveChanges { then, message, .. }) if then == "file.close" && message.contains("file changed or is unavailable"))
    );
    assert!(a.save_conflict.is_none());
    assert!(!a.quit_requested);
    assert_local(&a, &d, &b);
}

#[test]
fn dialog_buttons_keep_edits_and_render_selected_direction() {
    use egui_kittest::kittest::Queryable;
    let d = Scratch::new();
    let mut a = app(&d);
    let b = external(&d);
    a.run("file.save", json!({})).unwrap_err();
    let mut fonts = false;
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1280.0, 800.0)).build_ui_state(
        move |ui, app: &mut WordApp| {
            let ctx = ui.ctx().clone();
            crate::i18n::set_current(crate::i18n::Lang::EN);
            if fonts {
                dialogs::show(app, &ctx);
            } else {
                crate::theme::install_fonts_for(&ctx, false);
                fonts = true;
            }
        },
        a,
    );
    h.run();
    h.get_by_label("Keep editing").click();
    h.run();
    assert!(h.state().dialog.is_none());
    assert_local(h.state(), &d, &b);
    h.state_mut().run("file.save", json!({})).unwrap_err();
    h.run();
    h.get_by_label("Save copy…").click();
    h.run();
    // This harness has no file picker, so Save Copy is cancelled without writing.
    assert!(h.state().dialog.is_none());
    assert_local(h.state(), &d, &b);
}

#[test]
fn save_copy_name_has_no_trailing_dot_without_an_extension() {
    assert_eq!(copy_name("Report.docx"), "Report copy.docx");
    assert_eq!(copy_name("Report"), "Report copy");
    assert_eq!(copy_name("Report."), "Report copy");
}
