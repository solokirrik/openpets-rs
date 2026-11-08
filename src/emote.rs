//! Pixel-art `!` and `?` shown over the pet's head, like game emotes.
//!
//! Each emote is drawn on a small grid, one cell per image pixel; the app
//! scales it up with nearest-neighbour filtering so the pixels stay crisp
//! and match the pets' pixel style.

use crate::icon::Rgba;
use crate::reaction::Badge;

/// `#` outline, `o` fill, `.` transparent.
const ALERT: [&str; 12] = [
    ".####.", //
    "#oooo#", //
    "#oooo#", //
    "#oooo#", //
    "#oooo#", //
    ".#oo#.", //
    ".#oo#.", //
    "..##..", //
    "......", //
    ".####.", //
    "#oooo#", //
    ".####.", //
];

const QUESTION: [&str; 12] = [
    ".######.", //
    "#oooooo#", //
    "#oo##oo#", //
    ".##.#oo#", //
    "...#ooo#", //
    "..#ooo#.", //
    "..#oo#..", //
    "..####..", //
    "........", //
    "..####..", //
    "..#oo#..", //
    "..####..", //
];

const OUTLINE: [u8; 4] = [26, 22, 30, 255];
const ALERT_FILL: [u8; 4] = [255, 200, 40, 255];
const QUESTION_FILL: [u8; 4] = [90, 200, 255, 255];

/// The emote for `badge` at one pixel per grid cell (opaque colours, so
/// premultiplied and straight alpha agree).
pub fn render(badge: Badge) -> Rgba {
    let (art, fill): (&[&str], _) = match badge {
        Badge::Alert => (&ALERT, ALERT_FILL),
        Badge::Question => (&QUESTION, QUESTION_FILL),
    };
    let width = art[0].len();
    let mut image = Rgba::new(width, art.len());
    for (y, row) in art.iter().enumerate() {
        for (x, cell) in row.bytes().enumerate() {
            let colour = match cell {
                b'#' => OUTLINE,
                b'o' => fill,
                _ => continue,
            };
            let index = (y * width + x) * 4;
            image.pixels[index..index + 4].copy_from_slice(&colour);
        }
    }
    image
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Editing the art by hand easily leaves a ragged row, which would
    /// smear the image (rows are copied at the first row's width).
    #[test]
    fn emote_art_is_rectangular() {
        for art in [&ALERT[..], &QUESTION[..]] {
            assert!(art.iter().all(|row| row.len() == art[0].len()));
        }
    }
}
