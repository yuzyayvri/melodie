//! Phone pairing (PLAN.md §7): a QR code carrying host, port and token —
//! "no login, no account, matching the rest of the project".

use std::net::IpAddr;

use fltk::enums::ColorDepth;
use fltk::frame::Frame;
use fltk::image::RgbImage;
use fltk::prelude::*;
use fltk::window::Window;

use crate::ui::theme;

/// The payload encoded in the QR and accepted by the companion app.
pub fn pairing_url(host: &IpAddr, port: u16, token: &str) -> String {
    format!("melodie://{host}:{port}/{token}")
}

/// Renders `data` as a QR code into a raw RGB8 buffer, returning it with the
/// square's side length in pixels.
///
/// `scale` is pixels per QR module; `quiet` is the mandatory white border in
/// modules (4 is the spec minimum, and scanners really do need it).
pub fn qr_rgb(data: &str, scale: usize, quiet: usize) -> Option<(Vec<u8>, i32)> {
    let code = qrcode::QrCode::new(data.as_bytes()).ok()?;
    let width = code.width();
    let modules = code.to_colors();
    let side = (width + quiet * 2) * scale;
    let mut buf = vec![255u8; side * side * 3];
    for y in 0..width {
        for x in 0..width {
            if modules[y * width + x] != qrcode::Color::Dark {
                continue;
            }
            for dy in 0..scale {
                for dx in 0..scale {
                    let py = (y + quiet) * scale + dy;
                    let px = (x + quiet) * scale + dx;
                    let i = (py * side + px) * 3;
                    buf[i] = 0;
                    buf[i + 1] = 0;
                    buf[i + 2] = 0;
                }
            }
        }
    }
    Some((buf, side as i32))
}

/// Opens a small window showing the QR plus the same details as text, so a
/// phone can be paired by scanning *or* by typing.
pub fn show(url: &str) {
    let Some((buf, side)) = qr_rgb(url, 6, 4) else {
        eprintln!("melodie: could not render the pairing QR for {url}");
        return;
    };
    let Ok(image) = RgbImage::new(&buf, side, side, ColorDepth::Rgb8) else {
        eprintln!("melodie: could not build the pairing image");
        return;
    };

    let pad = 16;
    let text_h = 52;
    let win_w = side + pad * 2;
    let win_h = side + pad * 2 + text_h;
    let mut win = Window::new(300, 200, win_w, win_h, "Pair phone");
    win.set_color(theme::BG);

    let mut art = Frame::new(pad, pad, side, side, None);
    art.set_image(Some(image));

    let mut label = Frame::new(pad, side + pad, side, text_h, None);
    label.set_label_color(theme::FG);
    label.set_label_size(theme::FONT_SIZE - 1);
    label.set_label(&format!("Scan in the Melodie app, or enter it by hand:\n{url}"));

    win.end();
    win.show();
    // Deliberately not modal: pairing shouldn't block playback controls.
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    #[test]
    fn pairing_url_is_the_format_the_android_app_parses() {
        let ip = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20));
        assert_eq!(pairing_url(&ip, 4533, "abc123"), "melodie://192.168.1.20:4533/abc123");
    }

    #[test]
    fn qr_rgb_produces_a_square_rgb_buffer_with_a_quiet_zone() {
        let (buf, side) = qr_rgb("melodie://192.168.1.20:4533/abc123", 4, 4).unwrap();
        assert!(side > 0);
        assert_eq!(buf.len(), (side as usize) * (side as usize) * 3);
        // The quiet zone must be white, or scanners fail to lock on.
        assert_eq!(&buf[0..3], &[255, 255, 255]);
        // And something must actually be drawn.
        assert!(buf.iter().any(|b| *b == 0), "QR matrix has no dark modules");
    }
}
