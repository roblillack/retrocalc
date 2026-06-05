//! A hair-thin wrapper over `arboard` for the Edit menu's Copy / Paste.
//!
//! The clipboard is opened fresh for each operation and every failure is
//! swallowed: in a headless session (CI, a `MockBackend` test) `Clipboard::new`
//! returns an error, and Copy / Paste simply become no-ops — the calculator
//! keeps working, exactly as saudade's own `TextEditor` degrades.

/// Put `text` on the system clipboard. Empty text (e.g. while an error is
/// latched) is skipped so Copy never clobbers the clipboard with nothing.
pub fn copy(text: &str) {
    if text.is_empty() {
        return;
    }
    if let Ok(mut clipboard) = arboard::Clipboard::new() {
        let _ = clipboard.set_text(text.to_owned());
    }
}

/// Read the system clipboard as text, or `None` if it's unavailable or empty.
pub fn paste() -> Option<String> {
    arboard::Clipboard::new()
        .ok()
        .and_then(|mut clipboard| clipboard.get_text().ok())
}
