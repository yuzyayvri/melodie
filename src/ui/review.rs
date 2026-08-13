//! SpotiSync match-confirmation screen (PLAN.md §5.4): tracks scored 40-75
//! land here instead of auto-downloading, so a live-version or wrong-artist
//! match can be caught before it's written into the library.

use fltk::browser::HoldBrowser;
use fltk::button::Button;
use fltk::enums::FrameType;
use fltk::prelude::*;
use fltk::window::Window;

use crate::db::PendingReview;
use crate::ui::theme;

pub struct ReviewWindow {
    pub win: Window,
    pub browser: HoldBrowser,
    pub confirm_btn: Button,
    pub reject_btn: Button,
    pub close_btn: Button,
    entries: Vec<PendingReview>,
}

pub fn build() -> ReviewWindow {
    let mut win = Window::new(200, 200, 520, 360, "Review matches");
    win.set_color(theme::BG);

    let mut browser = HoldBrowser::new(8, 8, 504, 300, None);
    browser.set_frame(FrameType::DownBox);
    browser.set_color(theme::BG_ALT);
    browser.set_text_size(theme::FONT_SIZE);

    let btn_y = 316;
    let mut confirm_btn = Button::new(8, btn_y, 100, 32, "Confirm");
    let mut reject_btn = Button::new(116, btn_y, 100, 32, "Reject");
    let mut close_btn = Button::new(412, btn_y, 100, 32, "Close");
    for b in [&mut confirm_btn, &mut reject_btn, &mut close_btn] {
        b.set_color(theme::BG_ALT);
        b.set_label_color(theme::FG);
    }

    win.end();

    ReviewWindow { win, browser, confirm_btn, reject_btn, close_btn, entries: Vec::new() }
}

impl ReviewWindow {
    pub fn set_entries(&mut self, entries: Vec<PendingReview>) {
        self.browser.clear();
        for e in &entries {
            self.browser.add(&format!(
                "{}  —  {}   [{}]   score {:.0}   youtu.be/{}",
                e.artist, e.title, e.album, e.score, e.youtube_id
            ));
        }
        self.entries = entries;
        let count = self.entries.len();
        self.win.set_label(&if count == 0 {
            "Review matches — nothing pending".to_string()
        } else {
            format!("Review matches ({count} pending)")
        });
    }

    /// The currently-selected entry's Spotify URI, if any (browser rows are
    /// 1-indexed; 0 means "nothing selected").
    pub fn selected_uri(&self) -> Option<String> {
        let idx = self.browser.value();
        if idx <= 0 {
            return None;
        }
        self.entries.get((idx - 1) as usize).map(|e| e.spotify_uri.clone())
    }
}
