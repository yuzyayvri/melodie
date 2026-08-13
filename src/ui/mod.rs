pub mod list;
pub mod review;
pub mod theme;

use fltk::button::Button;
use fltk::enums::{Align, CallbackTrigger, Color, Event, FrameType};
use fltk::frame::Frame;
use fltk::group::Group;
use fltk::menu::Choice;
use fltk::prelude::*;
use fltk::valuator::HorNiceSlider;
use fltk::window::Window;

use list::TrackList;

pub const WIN_W: i32 = 900;
pub const WIN_H: i32 = 600;
const BAR_H: i32 = 76;
const PAD: i32 = 8;

pub struct MainWindow {
    pub win: Window,
    pub playlist_choice: Choice,
    pub sync_btn: Button,
    pub review_btn: Button,
    pub prev_btn: Button,
    pub play_btn: Button,
    pub next_btn: Button,
    pub loop_btn: Button,
    pub seek: HorNiceSlider,
    pub time_label: Frame,
    pub volume: HorNiceSlider,
    pub now_playing: Frame,
    pub list: TrackList,
}

/// Swaps a button's background between `base` and `hover` on mouse
/// enter/leave — pure event-driven feedback, no polling/timers.
fn add_hover(btn: &mut Button, base: Color, hover: Color) {
    btn.handle(move |w, ev| match ev {
        Event::Enter => {
            w.set_color(hover);
            w.redraw();
            true
        }
        Event::Leave => {
            w.set_color(base);
            w.redraw();
            true
        }
        _ => false,
    });
}

pub fn build() -> MainWindow {
    let mut win = Window::new(100, 100, WIN_W, WIN_H, "Melodie");
    win.set_color(theme::BG);

    let mut bar = Group::new(0, 0, WIN_W, BAR_H, None);
    bar.set_frame(FrameType::FlatBox);
    bar.set_color(theme::BG_ALT);

    let choice_w = 140;
    let sync_w = 50;
    let review_w = 64;
    let cluster_w = sync_w + 4 + review_w + 4 + choice_w;

    let mut playlist_choice = Choice::new(WIN_W - choice_w - PAD, 6, choice_w, 24, None);
    playlist_choice.set_color(theme::BG);
    playlist_choice.set_label_color(theme::FG);
    playlist_choice.set_text_color(theme::FG);

    let mut review_btn = Button::new(WIN_W - choice_w - review_w - PAD - 4, 6, review_w, 24, "Review");
    let mut sync_btn = Button::new(WIN_W - choice_w - review_w - sync_w - PAD - 8, 6, sync_w, 24, "Sync");
    for b in [&mut sync_btn, &mut review_btn] {
        b.set_color(theme::BG);
        b.set_label_color(theme::FG);
        b.set_label_size(theme::FONT_SIZE - 1);
        b.set_frame(theme::BUTTON_FRAME);
        add_hover(b, theme::BG, theme::BTN_HOVER);
    }

    let mut now_playing = Frame::new(PAD, 6, WIN_W - PAD * 3 - cluster_w, 24, None);
    now_playing.set_label_color(theme::FG);
    now_playing.set_label_size(theme::FONT_SIZE + 1);
    now_playing.set_align(Align::Left | Align::Inside);
    now_playing.set_label("Nothing playing");

    let btn_y = 36;
    let mut prev_btn = Button::new(PAD, btn_y, 36, 32, "@|<");
    let mut play_btn = Button::new(PAD + 40, btn_y, 40, 32, "@>");
    let mut next_btn = Button::new(PAD + 84, btn_y, 36, 32, "@>|");
    for b in [&mut prev_btn, &mut play_btn, &mut next_btn] {
        b.set_color(theme::BG);
        b.set_label_color(theme::FG);
        b.set_frame(theme::BUTTON_FRAME);
        add_hover(b, theme::BG, theme::BTN_HOVER);
    }

    let mut loop_btn = Button::new(PAD + 128, btn_y, 50, 32, "Loop");
    loop_btn.set_color(theme::BG);
    loop_btn.set_label_color(theme::FG_DIM);
    loop_btn.set_label_size(theme::FONT_SIZE - 1);
    loop_btn.set_frame(theme::BUTTON_FRAME);
    add_hover(&mut loop_btn, theme::BG, theme::BTN_HOVER);

    let time_w = 90;
    let vol_w = 100;
    let seek_x = PAD + 186;
    let seek_w = WIN_W - seek_x - time_w - vol_w - PAD * 2;
    let mut seek = HorNiceSlider::new(seek_x, btn_y, seek_w, 32, None);
    seek.set_range(0.0, 1.0);
    seek.set_trigger(CallbackTrigger::Release);
    seek.set_color(theme::BG);
    seek.set_selection_color(theme::ACCENT);

    let mut time_label = Frame::new(seek_x + seek_w + PAD, btn_y, time_w, 32, None);
    time_label.set_label_color(theme::FG_DIM);
    time_label.set_label("0:00 / 0:00");

    let mut volume = HorNiceSlider::new(WIN_W - vol_w - PAD, btn_y, vol_w, 32, None);
    volume.set_range(0.0, 1.0);
    volume.set_trigger(CallbackTrigger::Changed);
    volume.set_color(theme::BG);
    volume.set_selection_color(theme::ACCENT);

    bar.end();

    let list = TrackList::new(0, BAR_H, WIN_W, WIN_H - BAR_H);

    win.end();
    win.resizable(&list.widget());
    win.size_range(480, 320, 0, 0);

    MainWindow {
        win,
        playlist_choice,
        sync_btn,
        review_btn,
        prev_btn,
        play_btn,
        next_btn,
        loop_btn,
        seek,
        time_label,
        volume,
        now_playing,
        list,
    }
}

pub fn format_time(secs: f64) -> String {
    let secs = secs.max(0.0) as i64;
    format!("{}:{:02}", secs / 60, secs % 60)
}
