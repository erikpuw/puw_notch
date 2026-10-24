//! Interface language: Vietnamese (default) or English, switched from the tray menu.
//! `tr("tiếng Việt", "English")` picks the text for the current language.

use std::sync::atomic::{AtomicBool, Ordering};

static ENGLISH: AtomicBool = AtomicBool::new(false);

pub fn set_english(on: bool) {
    ENGLISH.store(on, Ordering::Relaxed);
}

pub fn english() -> bool {
    ENGLISH.load(Ordering::Relaxed)
}

/// Text for the current language.
pub fn tr(vi: &'static str, en: &'static str) -> &'static str {
    if english() { en } else { vi }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switches() {
        set_english(false);
        assert_eq!(tr("Nhạc", "Music"), "Nhạc");
        set_english(true);
        assert_eq!(tr("Nhạc", "Music"), "Music");
        set_english(false);
    }
}
