//! Turns one spritesheet frame into icon artwork.
//!
//! The menu bar gets a one-colour silhouette, black for a light system theme
//! and white for a dark one. A plain opacity mask would fill in the face, so
//! dark details (eyes, outline) are cut out as holes. The app icon keeps the
//! frame's colours. Both are cropped to the pet itself, because frames carry
//! a lot of transparent padding that would make the pet tiny at icon sizes.

/// Premultiplied RGBA pixels, rows from top to bottom.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rgba {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u8>,
}

/// Pixels at least this opaque count as the pet. Lower values pull in soft
/// glows and shadows, which blur the silhouette at 18 points.
const SOLID_ALPHA: u8 = 96;

/// Solid pixels darker than this fraction of full brightness are cut out of
/// the silhouette, so eyes and other dark features stay visible.
const DARK_LUMINANCE: f64 = 0.35;

/// Below this share of light pixels the pet is mostly dark (a black cat, say)
/// and cutting out dark pixels would erase it; use the plain shape instead.
const MIN_LIGHT_SHARE: f64 = 0.6;

impl Rgba {
    pub fn new(width: usize, height: usize) -> Rgba {
        Rgba {
            width,
            height,
            pixels: vec![0; width * height * 4],
        }
    }

    fn alpha(&self, x: usize, y: usize) -> u8 {
        self.pixels[(y * self.width + x) * 4 + 3]
    }

    /// The smallest rectangle holding every solid pixel, as (x, y, width, height).
    fn solid_bounds(&self) -> Option<(usize, usize, usize, usize)> {
        let mut bounds: Option<(usize, usize, usize, usize)> = None;
        for y in 0..self.height {
            for x in 0..self.width {
                if self.alpha(x, y) < SOLID_ALPHA {
                    continue;
                }
                let (min_x, min_y, max_x, max_y) = bounds.unwrap_or((x, y, x, y));
                bounds = Some((min_x.min(x), min_y.min(y), max_x.max(x), max_y.max(y)));
            }
        }
        bounds.map(|(min_x, min_y, max_x, max_y)| {
            (min_x, min_y, max_x - min_x + 1, max_y - min_y + 1)
        })
    }

    fn crop(&self, x: usize, y: usize, width: usize, height: usize) -> Rgba {
        let mut cropped = Rgba::new(width, height);
        for row in 0..height {
            let source = ((y + row) * self.width + x) * 4;
            let target = row * width * 4;
            cropped.pixels[target..target + width * 4]
                .copy_from_slice(&self.pixels[source..source + width * 4]);
        }
        cropped
    }

    /// Centres the image on a transparent square canvas.
    fn into_square(self) -> Rgba {
        let side = self.width.max(self.height);
        let mut square = Rgba::new(side, side);
        let offset_x = (side - self.width) / 2;
        let offset_y = (side - self.height) / 2;
        for row in 0..self.height {
            let source = row * self.width * 4;
            let target = ((offset_y + row) * side + offset_x) * 4;
            square.pixels[target..target + self.width * 4]
                .copy_from_slice(&self.pixels[source..source + self.width * 4]);
        }
        square
    }
}

/// The frame cropped to the pet. `None` when the frame has no solid pixels.
pub fn crop_to_pet(frame: &Rgba) -> Option<Rgba> {
    let (x, y, width, height) = frame.solid_bounds()?;
    Some(frame.crop(x, y, width, height))
}

/// A silhouette of the frame in the opaque `ink` colour (RGB), cropped to
/// the pet, with dark details cut out. `None` when the frame has no solid pixels.
///
/// Pass the frame already scaled to icon size: thresholding after scaling
/// gives clean shapes, while scaling a thresholded frame leaves speckles.
pub fn silhouette(frame: &Rgba, ink: [u8; 3]) -> Option<Rgba> {
    let (x, y, width, height) = frame.solid_bounds()?;
    let mut shape = frame.crop(x, y, width, height);
    let pixels = shape.pixels.as_chunks_mut::<4>().0;

    let solid = pixels.iter().filter(|pixel| is_solid(pixel)).count();
    let light = pixels
        .iter()
        .filter(|pixel| is_solid(pixel) && !is_dark(pixel))
        .count();
    let cut_out_dark = light as f64 >= solid as f64 * MIN_LIGHT_SHARE;

    let [red, green, blue] = ink;
    for pixel in pixels {
        let inked = is_solid(pixel) && !(cut_out_dark && is_dark(pixel));
        *pixel = if inked {
            [red, green, blue, 255]
        } else {
            [0, 0, 0, 0]
        };
    }
    Some(shape)
}

fn is_solid(pixel: &[u8; 4]) -> bool {
    pixel[3] >= SOLID_ALPHA
}

/// Compares premultiplied luminance against the pixel's own alpha, so a
/// half-transparent white pixel still counts as light.
fn is_dark(pixel: &[u8; 4]) -> bool {
    let [red, green, blue, alpha] = pixel.map(f64::from);
    let luminance = 0.2126 * red + 0.7152 * green + 0.0722 * blue;
    luminance < DARK_LUMINANCE * alpha
}

/// The frame in colour, cropped to the pet and centred on a square.
/// `None` when the frame has no solid pixels.
pub fn app_icon(frame: &Rgba) -> Option<Rgba> {
    Some(crop_to_pet(frame)?.into_square())
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLACK: [u8; 3] = [0, 0, 0];
    const WHITE: [u8; 3] = [255, 255, 255];

    fn paint(frame: &mut Rgba, x: usize, y: usize, rgba: [u8; 4]) {
        let index = (y * frame.width + x) * 4;
        frame.pixels[index..index + 4].copy_from_slice(&rgba);
    }

    /// A 6x4 frame with a red T-shaped pet (3x2) and a faint glow pixel beside it.
    fn frame_with_pet() -> Rgba {
        let mut frame = Rgba::new(6, 4);
        for (x, y) in [(2, 1), (3, 1), (4, 1), (3, 2)] {
            paint(&mut frame, x, y, [200, 0, 0, 255]);
        }
        paint(&mut frame, 1, 1, [10, 10, 10, 20]);
        frame
    }

    /// A 3x3 face with one black eye in the middle.
    fn face_with_eye(face: [u8; 4]) -> Rgba {
        let mut frame = Rgba::new(3, 3);
        for y in 0..3 {
            for x in 0..3 {
                paint(&mut frame, x, y, face);
            }
        }
        paint(&mut frame, 1, 1, [0, 0, 0, 255]);
        frame
    }

    fn ink_at(shape: &Rgba, x: usize, y: usize) -> bool {
        shape.alpha(x, y) == 255
    }

    #[test]
    fn silhouette_is_one_colour_and_cropped_to_the_pet() {
        let shape = silhouette(&frame_with_pet(), WHITE).unwrap();

        assert_eq!((shape.width, shape.height), (3, 2));
        let one_colour = shape
            .pixels
            .as_chunks::<4>()
            .0
            .iter()
            .all(|pixel| *pixel == [255, 255, 255, 255] || *pixel == [0, 0, 0, 0]);
        assert!(one_colour);
    }

    #[test]
    fn dark_eyes_stay_visible_as_holes() {
        let shape = silhouette(&face_with_eye([255, 255, 255, 255]), BLACK).unwrap();

        assert!(ink_at(&shape, 0, 0));
        assert!(!ink_at(&shape, 1, 1));
    }

    #[test]
    fn a_mostly_dark_pet_keeps_its_whole_shape() {
        let shape = silhouette(&face_with_eye([20, 20, 20, 255]), BLACK).unwrap();

        assert!(ink_at(&shape, 0, 0));
        assert!(ink_at(&shape, 1, 1));
    }

    #[test]
    fn app_icon_keeps_colour_on_a_square_canvas() {
        let icon = app_icon(&frame_with_pet()).unwrap();

        assert_eq!((icon.width, icon.height), (3, 3));
        assert!(icon.pixels.as_chunks::<4>().0.contains(&[200, 0, 0, 255]));
    }

    #[test]
    fn empty_frame_has_no_icon() {
        assert!(silhouette(&Rgba::new(4, 4), BLACK).is_none());
        assert!(app_icon(&Rgba::new(4, 4)).is_none());
    }
}
