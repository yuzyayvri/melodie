use std::cell::RefCell;
use std::rc::Rc;

use fltk::draw;
use fltk::enums::{Align, Event, FrameType};
use fltk::group::Group;
use fltk::prelude::*;
use fltk::valuator::Scrollbar;
use fltk::widget::Widget;

use super::theme;

/// One line in the list. Kept generic-ish but tailored to what the library
/// view needs — three text columns.
#[derive(Debug, Clone, Default)]
pub struct Row {
    pub col1: String,
    pub col2: String,
    pub col3: String,
    pub duration_label: String,
}

struct State {
    rows: Vec<Row>,
    selected: Option<usize>,
    playing: Option<usize>,
    hover: Option<usize>,
    scroll: i32, // pixels
    on_activate: Option<Box<dyn FnMut(usize)>>,
}

/// A virtualised list: only the rows intersecting the visible area are laid
/// out on `draw()` (PLAN.md §6 tactic 3), so library size stops mattering
/// once you're past the first screenful.
#[derive(Clone)]
pub struct TrackList {
    group: Group,
    canvas: Widget,
    scrollbar: Scrollbar,
    state: Rc<RefCell<State>>,
}

impl TrackList {
    pub fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        let mut group = Group::new(x, y, w, h, None);
        group.set_frame(FrameType::FlatBox);

        let sb_w = 16;
        let mut canvas = Widget::new(x, y, w - sb_w, h, None);
        let mut scrollbar = Scrollbar::new(x + w - sb_w, y, sb_w, h, None);
        scrollbar.set_type(fltk::valuator::ScrollbarType::Vertical);
        group.end();
        group.resizable(&canvas);

        let state = Rc::new(RefCell::new(State {
            rows: Vec::new(),
            selected: None,
            playing: None,
            hover: None,
            scroll: 0,
            on_activate: None,
        }));

        canvas.draw({
            let state = state.clone();
            move |c| draw_canvas(c, &state)
        });

        {
            let state = state.clone();
            let mut canvas_for_handle = canvas.clone();
            canvas.handle(move |c, ev| handle_canvas(c, ev, &state, &mut canvas_for_handle));
        }

        {
            let state = state.clone();
            let mut canvas_redraw = canvas.clone();
            scrollbar.set_callback(move |sb| {
                state.borrow_mut().scroll = sb.value() as i32;
                canvas_redraw.redraw();
            });
        }

        let list = Self { group, canvas, scrollbar, state };
        list.sync_scrollbar();
        list
    }

    pub fn widget(&self) -> Group {
        self.group.clone()
    }

    pub fn set_rows(&mut self, rows: Vec<Row>) {
        {
            let mut s = self.state.borrow_mut();
            s.rows = rows;
            s.selected = None;
            s.hover = None;
            s.scroll = 0;
        }
        self.scrollbar.set_value(0.0);
        self.sync_scrollbar();
        self.canvas.redraw();
    }

    pub fn set_playing(&mut self, index: Option<usize>) {
        self.state.borrow_mut().playing = index;
        self.canvas.redraw();
    }

    /// Registers the double-click / Enter "activate this row" callback.
    pub fn set_on_activate<CB: FnMut(usize) + 'static>(&mut self, cb: CB) {
        self.state.borrow_mut().on_activate = Some(Box::new(cb));
    }

    fn sync_scrollbar(&self) {
        let s = self.state.borrow();
        let visible_rows = (self.canvas.height() / theme::ROW_HEIGHT).max(1);
        let total = s.rows.len() as i32;
        let max_scroll = ((total - visible_rows).max(0) * theme::ROW_HEIGHT) as f64;
        let mut sb = self.scrollbar.clone();
        sb.set_bounds(0.0, max_scroll);
        sb.set_slider_size(if total == 0 { 1.0 } else { (visible_rows as f64 / total as f64).min(1.0) as f32 });
    }
}

fn visible_range(state: &State, height: i32) -> std::ops::Range<usize> {
    let first = (state.scroll / theme::ROW_HEIGHT).max(0) as usize;
    let count = (height / theme::ROW_HEIGHT) as usize + 2;
    let last = (first + count).min(state.rows.len());
    first..last
}

fn draw_canvas(c: &mut Widget, state: &Rc<RefCell<State>>) {
    let (x, y, w, h) = (c.x(), c.y(), c.w(), c.h());
    draw::push_clip(x, y, w, h);
    draw::draw_rect_fill(x, y, w, h, theme::BG);

    let s = state.borrow();
    draw::set_font(theme::FONT, theme::FONT_SIZE);

    for i in visible_range(&s, h) {
        let row = &s.rows[i];
        let row_top = y + (i as i32 * theme::ROW_HEIGHT) - s.scroll;
        if row_top + theme::ROW_HEIGHT < y || row_top > y + h {
            continue;
        }

        let bg = if Some(i) == s.selected {
            theme::ROW_SELECTED
        } else if Some(i) == s.hover {
            theme::ROW_HOVER
        } else if i % 2 == 0 {
            theme::BG
        } else {
            theme::BG_ALT
        };
        draw::draw_rect_fill(x, row_top, w, theme::ROW_HEIGHT, bg);

        let playing = Some(i) == s.playing;
        if playing {
            draw::draw_rect_fill(x, row_top, 3, theme::ROW_HEIGHT, theme::ACCENT);
        }

        let fg = if playing { theme::ACCENT } else { theme::FG };
        let col_w = (w - 90) / 3;
        let text_y = row_top;
        draw::set_draw_color(fg);
        draw::set_font(theme::FONT_BOLD, theme::FONT_SIZE);
        draw::draw_text2(&row.col1, x + 12, text_y, col_w, theme::ROW_HEIGHT, Align::Left);
        draw::set_font(theme::FONT, theme::FONT_SIZE);
        draw::draw_text2(&row.col2, x + 8 + col_w, text_y, col_w, theme::ROW_HEIGHT, Align::Left);
        draw::draw_text2(&row.col3, x + 8 + col_w * 2, text_y, col_w, theme::ROW_HEIGHT, Align::Left);
        draw::set_draw_color(theme::FG_DIM);
        draw::draw_text2(&row.duration_label, x + w - 90, text_y, 80, theme::ROW_HEIGHT, Align::Right);
    }
    draw::pop_clip();
}

fn handle_canvas(c: &mut Widget, ev: Event, state: &Rc<RefCell<State>>, canvas: &mut Widget) -> bool {
    match ev {
        Event::Focus | Event::Unfocus => true,
        Event::Move => {
            let my = fltk::app::event_y() - c.y();
            let row_len = state.borrow().rows.len();
            let scroll = state.borrow().scroll;
            let idx = ((scroll + my) / theme::ROW_HEIGHT) as usize;
            let new_hover = if idx < row_len { Some(idx) } else { None };
            let mut s = state.borrow_mut();
            if s.hover != new_hover {
                s.hover = new_hover;
                drop(s);
                canvas.redraw();
            }
            true
        }
        Event::Leave => {
            let mut s = state.borrow_mut();
            if s.hover.is_some() {
                s.hover = None;
                drop(s);
                canvas.redraw();
            }
            true
        }
        Event::Push => {
            let _ = c.take_focus();
            let my = fltk::app::event_y() - c.y();
            let row_len = state.borrow().rows.len();
            let scroll = state.borrow().scroll;
            let idx = ((scroll + my) / theme::ROW_HEIGHT) as usize;
            if idx < row_len {
                state.borrow_mut().selected = Some(idx);
                canvas.redraw();
                if fltk::app::event_clicks() {
                    activate(state, idx);
                }
            }
            true
        }
        Event::MouseWheel => {
            let dy = fltk::app::event_dy();
            let delta = match dy {
                fltk::app::MouseWheel::Down => theme::ROW_HEIGHT * 3,
                fltk::app::MouseWheel::Up => -theme::ROW_HEIGHT * 3,
                _ => 0,
            };
            if delta != 0 {
                let mut s = state.borrow_mut();
                let max_scroll = ((s.rows.len() as i32 - c.h() / theme::ROW_HEIGHT).max(0)) * theme::ROW_HEIGHT;
                s.scroll = (s.scroll + delta).clamp(0, max_scroll);
                drop(s);
                canvas.redraw();
            }
            true
        }
        Event::KeyDown => {
            let key = fltk::app::event_key();
            if key == fltk::enums::Key::Enter {
                if let Some(idx) = state.borrow().selected {
                    activate(state, idx);
                }
                return true;
            }
            let mut s = state.borrow_mut();
            let len = s.rows.len();
            if len == 0 {
                return false;
            }
            let next = match key {
                fltk::enums::Key::Down => s.selected.map(|i| (i + 1).min(len - 1)).or(Some(0)),
                fltk::enums::Key::Up => s.selected.map(|i| i.saturating_sub(1)).or(Some(0)),
                _ => None,
            };
            if let Some(n) = next {
                s.selected = Some(n);
                drop(s);
                canvas.redraw();
                return true;
            }
            false
        }
        _ => false,
    }
}

fn activate(state: &Rc<RefCell<State>>, idx: usize) {
    let mut s = state.borrow_mut();
    if let Some(cb) = s.on_activate.as_mut() {
        cb(idx);
    }
}
