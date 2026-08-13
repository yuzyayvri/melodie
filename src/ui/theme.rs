use fltk::enums::{Color, Font, FrameType};

pub const BG: Color = Color::from_rgb(0x1a, 0x1b, 0x1e);
pub const BG_ALT: Color = Color::from_rgb(0x22, 0x23, 0x27);
pub const ROW_HOVER: Color = Color::from_rgb(0x2c, 0x2d, 0x32);
pub const ROW_SELECTED: Color = Color::from_rgb(0x35, 0x52, 0x82);
pub const FG: Color = Color::from_rgb(0xec, 0xec, 0xee);
pub const FG_DIM: Color = Color::from_rgb(0x8d, 0x8d, 0x94);
pub const ACCENT: Color = Color::from_rgb(0x6c, 0xa8, 0xff);
pub const BTN_HOVER: Color = Color::from_rgb(0x30, 0x32, 0x38);

pub const FONT: Font = Font::Helvetica;
pub const FONT_BOLD: Font = Font::HelveticaBold;
pub const FONT_SIZE: i32 = 13;
pub const ROW_HEIGHT: i32 = 26;
/// Rounded-flat box: modern look, no extra draw cost over the default frame.
pub const BUTTON_FRAME: FrameType = FrameType::RFlatBox;

/// Applies the app-wide scheme and default colours. Called once at startup.
pub fn apply() {
    fltk::app::background(0x1a, 0x1b, 0x1e);
    fltk::app::background2(0x22, 0x23, 0x27);
    fltk::app::foreground(0xec, 0xec, 0xee);
    fltk::app::set_font(FONT);
    fltk::app::set_font_size(FONT_SIZE);
}
