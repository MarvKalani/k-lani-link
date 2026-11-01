//! QR codes for pairing: drawn in the host's terminal, read from the client's camera frames.
use crate::diagnostic::Diagnostic;
use anyhow::Result;
use qrcode::{Color, EcLevel, QrCode};

/// Light modules around the code; scanners need the margin to find it.
const QUIET: usize = 4;
/// The largest camera frame the panel sends, per side.
pub const MAX_SIDE: u32 = 2000;

fn modules(text: &str) -> Result<(usize, Vec<bool>)> {
    let code = QrCode::with_error_correction_level(text, EcLevel::M)
        .map_err(|_| Diagnostic::err("the pairing data does not fit in a QR code"))?;
    let dark = code.to_colors().iter().map(|c| *c == Color::Dark).collect();
    Ok((code.width(), dark))
}

/// The code as terminal text: two module rows per line, in explicit black on white, so it
/// reads the same on a dark or a light terminal theme.
pub fn terminal(text: &str) -> Result<String> {
    let (width, dark) = modules(text)?;
    let side = width + 2 * QUIET;
    let at = |x: usize, y: usize| {
        (QUIET..width + QUIET).contains(&x)
            && (QUIET..width + QUIET).contains(&y)
            && dark[(y - QUIET) * width + x - QUIET]
    };
    let colour = |dark: bool| if dark { 0 } else { 15 };
    let mut out = String::new();
    for y in (0..side).step_by(2) {
        for x in 0..side {
            // Upper half block: the foreground paints the upper module, the background the lower.
            let (upper, lower) = (at(x, y), y + 1 < side && at(x, y + 1));
            out.push_str(&format!(
                "\x1b[38;5;{};48;5;{}m▀",
                colour(upper),
                colour(lower)
            ));
        }
        out.push_str("\x1b[0m\n");
    }
    Ok(out)
}

/// Lets the Windows console interpret the colour codes; Windows Terminal does already.
#[cfg(windows)]
pub fn enable_colours() {
    use windows_sys::Win32::System::Console::{
        ENABLE_VIRTUAL_TERMINAL_PROCESSING, GetConsoleMode, GetStdHandle, STD_OUTPUT_HANDLE,
        SetConsoleMode,
    };
    // SAFETY: plain console calls on this process's own standard output handle.
    unsafe {
        let handle = GetStdHandle(STD_OUTPUT_HANDLE);
        let mut mode = 0;
        if GetConsoleMode(handle, &mut mode) != 0 {
            SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING);
        }
    }
}
#[cfg(not(windows))]
pub fn enable_colours() {}

/// The text of the QR code in a greyscale frame, if there is one. A code shown light on dark
/// is read too.
pub fn read(width: u32, height: u32, luma: Vec<u8>) -> Option<String> {
    let find = |pixels: Vec<u8>| {
        let mut hints = rxing::DecodeHints::default().with(rxing::DecodeHintValue::TryHarder(true));
        rxing::helpers::detect_in_luma_with_hints(
            pixels,
            width,
            height,
            Some(rxing::BarcodeFormat::QR_CODE),
            &mut hints,
        )
        .ok()
        .map(|found| found.getText().to_owned())
    };
    let inverted = luma.iter().map(|v| 255 - v).collect();
    find(luma).or_else(|| find(inverted))
}

/// The code as a greyscale picture with `scale` pixels per module, for tests.
#[cfg(test)]
pub(crate) fn picture(text: &str, scale: usize) -> (u32, u32, Vec<u8>) {
    let (width, dark) = modules(text).unwrap();
    let side = (width + 2 * QUIET) * scale;
    let mut pixels = vec![255u8; side * side];
    for y in 0..side {
        for x in 0..side {
            let (mx, my) = (x / scale, y / scale);
            if (QUIET..width + QUIET).contains(&mx)
                && (QUIET..width + QUIET).contains(&my)
                && dark[(my - QUIET) * width + mx - QUIET]
            {
                pixels[y * side + x] = 0;
            }
        }
    }
    (side as u32, side as u32, pixels)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_code_is_read_back_in_either_polarity_and_from_a_blurred_tilted_photo() {
        let text = "k-lani-link test 0123456789";
        let (width, height, pixels) = picture(text, 6);
        assert_eq!(read(width, height, pixels.clone()).as_deref(), Some(text));
        let inverted = pixels.iter().map(|v| 255 - v).collect();
        assert_eq!(read(width, height, inverted).as_deref(), Some(text));
        // A camera: the code smaller, the top edge narrower, softened and noisy.
        let (w, h) = (640usize, 480usize);
        let mut photo = vec![60u8; w * h];
        let mut seed = 7u64;
        for y in 0..h {
            for x in 0..w {
                let (fx, fy) = (x as f64 / w as f64, y as f64 / h as f64);
                let squeeze = 1.0 - 0.12 * (1.0 - fy);
                let sx = ((0.5 + (fx - 0.5) / squeeze) * 1.3 - 0.15) * f64::from(width);
                let sy = (fy * 1.3 - 0.15) * f64::from(height);
                let mut value = if (0.0..f64::from(width)).contains(&sx)
                    && (0.0..f64::from(height)).contains(&sy)
                {
                    f64::from(pixels[sy as usize * width as usize + sx as usize]) * 0.8 + 20.0
                } else {
                    60.0
                };
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                value += (seed % 1000) as f64 / 1000.0 * 24.0 - 12.0;
                photo[y * w + x] = value.clamp(0.0, 255.0) as u8;
            }
        }
        let sharp = photo.clone();
        for y in 1..h - 1 {
            for x in 1..w - 1 {
                let sum: u32 = (0..9)
                    .map(|i| u32::from(sharp[(y + i / 3 - 1) * w + x + i % 3 - 1]))
                    .sum();
                photo[y * w + x] = (sum / 9) as u8;
            }
        }
        assert_eq!(read(w as u32, h as u32, photo).as_deref(), Some(text));
        assert_eq!(read(4, 4, vec![128; 16]), None);
    }

    #[test]
    fn the_terminal_code_uses_explicit_colours_and_fits_eighty_columns() {
        let text = "x".repeat(220);
        let drawn = terminal(&text).unwrap();
        let first = drawn.lines().next().unwrap();
        assert!(first.starts_with("\x1b[38;5;15;48;5;15m▀"));
        assert!(first.matches('▀').count() <= 80);
        assert!(drawn.lines().count() <= 40);
    }
}
