//! View only ([`Session::view_only`]): a host that shows a document to read (`?readonly=1` on the
//! web) turns it on, and then no command changes a document or writes a file. Read Mode and
//! Immersive Reader only change how the page looks; this is the lock behind them.
//!
//! Refused (and greyed out, through [`Session::why_disabled`]): every command that changes the
//! document (`mutates`), and the commands that change none but write it out or change which file
//! it is ([`WRITES`]): Save, Save As, the exports, Print to PDF, Save as Template, encryption,
//! versions and recovery, and starting a new document. Reading, selecting, copying, zooming,
//! navigating, Find and printing keep working.
//!
//! [`Session::run`] refuses before anything runs, so it holds for every caller: the ribbon, keys,
//! the control channel, MCP and the web page's command API.

use crate::Session;

/// Why a command is off in a view-only session: shown in its tooltip and returned as its error.
pub const REASON: &str = "This document is open to read only";

/// The commands that change no document but write it to a file, hand it to the host, change which
/// file it is or start another one to edit.
pub const WRITES: &[&str] = &[
    "file.save",
    "file.saveAs",
    "file.saveTemplate",
    "file.exportPdf",
    "file.exportPng",
    "file.printPdf",
    "file.encrypt",
    "file.rename",
    "file.autosave",
    "file.versions",
    "file.recover",
    "file.new",
    "file.newFromTemplate",
    "mailings.finish",
];

/// Why `id` (which `mutates` or not) can't run in `s`: [`REASON`] in a view-only session for a
/// command that changes a document or writes a file, else `None`.
pub fn refusal(s: &Session, id: &str, mutates: bool) -> Option<&'static str> {
    (s.view_only && (mutates || WRITES.contains(&id))).then_some(REASON)
}

impl Session {
    /// Why command `id` is refused because the session is view only; `None` when it may run (or
    /// doesn't exist). For front ends that write files themselves (the web build's downloads)
    /// instead of running the command.
    pub fn view_only_refusal(&self, id: &str) -> Option<&'static str> {
        self.registry.get(id).and_then(|spec| refusal(self, id, spec.mutates))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{REASON, WRITES};
    use crate::{CmdError, Session};

    /// A sample document, open view only, with its text selected (so a command acting on the
    /// selection has something to act on).
    fn viewing() -> Session {
        let mut s = Session::new(crate::sample::sample_document());
        s.run("select.all", &json!({})).unwrap();
        s.view_only = true;
        s
    }

    /// A fresh, empty folder for files a command might write.
    fn folder(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("wc-view-only-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn refused(r: &Result<serde_json::Value, CmdError>) -> bool {
        matches!(r, Err(CmdError::Disabled(why)) if why.ends_with(REASON))
    }

    /// Every registered command that changes a document, and every one that writes it out, is
    /// refused before it runs, is greyed out with the reason, and leaves the document as it was.
    #[test]
    fn every_editing_and_writing_command_is_refused() {
        let mut s = viewing();
        let before = s.doc.clone();
        let specs: Vec<(&'static str, bool)> = s.registry.all().iter().map(|spec| (spec.id, spec.mutates)).collect();
        let mut ran = vec![];
        for (id, mutates) in specs.iter().copied().filter(|(id, mutates)| *mutates || WRITES.contains(id)) {
            let r = s.run(id, &json!({}));
            if !refused(&r) {
                ran.push(format!("{id} (mutates: {mutates}): {r:?}"));
            }
            assert_eq!(s.why_disabled(id), Some(REASON), "{id} is not greyed out");
        }
        assert!(ran.is_empty(), "these ran in a view-only session: {ran:#?}");
        assert!(s.doc.same_content(&before) && !s.dirty && s.undo_depth() == 0, "a refused command left a trace");
        // Every listed writer is a real command: a renamed one would silently stop being refused.
        for id in WRITES {
            assert!(s.registry.get(id).is_some(), "{id} is not a command");
        }
    }

    /// The commands left to run in a view-only session change nothing: run each with no
    /// parameters and with a file to write, the document's content, its unsaved-changes flag and
    /// its undo history stay as they were, and no file appears. A command that edits but isn't
    /// marked as editing (nor listed in `WRITES`) fails here. Closing the document (which leaves
    /// a blank one, view only too) puts another document in front; it edits neither.
    #[test]
    fn no_command_left_running_changes_the_document_or_writes_a_file() {
        let dir = folder("writes");
        let mut s = viewing();
        let before = s.doc.clone();
        let viewed = s.document_id();
        let ids: Vec<&'static str> = s.registry.all().iter().filter(|spec| s.why_disabled(spec.id) != Some(REASON)).map(|spec| spec.id).collect();
        assert!(ids.len() > 100, "the view-only session can still read, select, view and find");
        let mut changed = vec![];
        for id in ids {
            for ext in ["docx", "pdf", "png", "odt", "txt"] {
                let path = dir.join(format!("probe.{ext}")).to_string_lossy().into_owned();
                for params in [json!({}), json!({"path": path, "value": true, "text": "x", "name": "x"})] {
                    let _ = s.run(id, &params);
                    if s.document_id() != viewed {
                        s = viewing();
                        continue;
                    }
                    if !s.doc.same_content(&before) || s.dirty || s.undo_depth() > 0 {
                        changed.push(format!("{id} {params}"));
                        // Start the next command from the document as it was.
                        s = viewing();
                    }
                }
            }
        }
        let written: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(Result::ok).map(|e| e.file_name()).collect();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(changed.is_empty(), "these changed the document in a view-only session: {changed:#?}");
        assert!(written.is_empty(), "these files were written in a view-only session: {written:?}");
    }

    /// Reading keeps working: selecting, copying, zooming, moving the caret, Find and Print.
    #[test]
    fn reading_selecting_copying_zooming_and_printing_still_run() {
        let mut s = viewing();
        let zoom = s.view.zoom;
        for (id, params) in [
            ("select.all", json!({})),
            ("edit.copy", json!({})),
            ("view.zoom", json!({"value": 150})),
            ("caret.right", json!({})),
            ("edit.find", json!({"text": "the"})),
            ("file.print", json!({})),
            ("view.readMode", json!({})),
        ] {
            assert_eq!(s.why_disabled(id), None, "{id} is greyed out");
            assert!(s.run(id, &params).is_ok(), "{id} failed in a view-only session");
        }
        assert!(!s.clipboard_text.is_empty(), "Copy copied the selection");
        assert!(s.view.zoom > zoom, "Zoom zoomed");
    }

    /// The same session with view only off edits as usual: the refusal comes from the flag.
    #[test]
    fn without_view_only_the_same_commands_run() {
        let mut s = viewing();
        s.view_only = false;
        assert_eq!(s.why_disabled("format.bold"), None);
        assert!(s.run("format.bold", &json!({})).is_ok());
        assert!(s.dirty && s.undo_depth() == 1, "Bold was an edit");
        assert_eq!(s.view_only_refusal("file.save"), None);
        s.view_only = true;
        assert_eq!(s.view_only_refusal("file.save"), Some(REASON));
        assert_eq!(s.view_only_refusal("edit.copy"), None);
    }
}
