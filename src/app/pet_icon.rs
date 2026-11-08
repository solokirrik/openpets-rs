//! Menu bar and app icons made from the current pet's first idle frame.
//!
//! CoreGraphics does the pixel I/O (decode the frame, wrap the result as an
//! image); `crate::icon` decides which pixels make the icon.

use objc2::AnyThread;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSApplication, NSImage, NSWorkspace,
    NSWorkspaceIconCreationOptions,
};
use objc2_core_foundation::{CFRetained, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGBitmapContextCreateImage, CGBitmapContextGetBytesPerRow,
    CGBitmapContextGetData, CGColorSpace, CGContext, CGImage, CGImageAlphaInfo,
    CGImageByteOrderInfo, CGInterpolationQuality,
};
use objc2_foundation::{MainThreadMarker, NSArray, NSBundle, NSSize};

use crate::icon::{self, Rgba};
use crate::pet::COLUMNS;

/// Menu bar icons are drawn 18 points tall.
const MENU_BAR_HEIGHT: f64 = 18.0;
/// Wide pets are capped so the item stays compact.
const MENU_BAR_MAX_WIDTH: f64 = 24.0;
const APP_ICON_SIZE: f64 = 512.0;

/// The frame that represents a pet: the first frame of its idle row.
const KEY_ROW: usize = 0;
const KEY_COLUMN: usize = 0;

/// The menu bar icon's colour scheme, following the system light/dark theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    Light,
    Dark,
}

impl Theme {
    /// The system theme right now. AppKit updates it as soon as the user
    /// switches (or Auto switches at sunset), so polling it is enough.
    pub fn current(mtm: MainThreadMarker) -> Theme {
        let appearance = NSApplication::sharedApplication(mtm).effectiveAppearance();
        // SAFETY: AppKit's appearance name constants are immutable statics.
        let (light, dark) = unsafe { (NSAppearanceNameAqua, NSAppearanceNameDarkAqua) };
        let best =
            appearance.bestMatchFromAppearancesWithNames(&NSArray::from_slice(&[light, dark]));
        if best.is_some_and(|name| &*name == dark) {
            Theme::Dark
        } else {
            Theme::Light
        }
    }

    fn ink(self) -> [u8; 3] {
        match self {
            Theme::Light => [0, 0, 0],
            Theme::Dark => [255, 255, 255],
        }
    }
}

/// The status item image for `theme`, or `None` if the frame is blank.
///
/// Not a template image: macOS tints templates by the menu bar's own look,
/// which on recent macOS follows the wallpaper rather than the system theme.
pub fn menu_bar_icon(
    spritesheet: &NSImage,
    rows: usize,
    theme: Theme,
) -> Option<Retained<NSImage>> {
    let pet = icon::crop_to_pet(&key_frame(spritesheet, rows)?)?;

    let aspect = pet.width as f64 / pet.height as f64;
    let size = NSSize::new(
        (MENU_BAR_HEIGHT * aspect).min(MENU_BAR_MAX_WIDTH),
        MENU_BAR_HEIGHT,
    );
    // Threshold at the pixel size the menu bar shows (2x for Retina).
    let small = scaled(
        &pet,
        (size.width * 2.0).round() as usize,
        (size.height * 2.0) as usize,
    )?;
    let shape = icon::silhouette(&small, theme.ink())?;
    let image = cg_image(&shape)?;
    Some(NSImage::initWithCGImage_size(
        NSImage::alloc(),
        &image,
        size,
    ))
}

/// Stamps the pet onto the app bundle's Finder icon, as Ghostty does for its
/// custom icons. Does nothing when not running from an `.app` bundle.
pub fn set_app_icon(spritesheet: &NSImage, rows: usize) {
    let bundle_path = NSBundle::mainBundle().bundlePath();
    if !bundle_path.to_string().ends_with(".app") {
        return;
    }
    let Some(image) = key_frame(spritesheet, rows)
        .and_then(|frame| icon::app_icon(&frame))
        .and_then(|square| cg_image(&square))
    else {
        eprintln!("[icon] pet frame is blank; keeping the app icon");
        return;
    };

    let icon = NSImage::initWithCGImage_size(
        NSImage::alloc(),
        &image,
        NSSize::new(APP_ICON_SIZE, APP_ICON_SIZE),
    );
    let workspace = NSWorkspace::sharedWorkspace();
    let updated = workspace.setIcon_forFile_options(
        Some(&icon),
        &bundle_path,
        NSWorkspaceIconCreationOptions(0),
    );
    if !updated {
        eprintln!("[icon] macOS refused the app icon update");
    }
}

/// Decodes the key frame into premultiplied RGBA pixels.
fn key_frame(spritesheet: &NSImage, rows: usize) -> Option<Rgba> {
    // SAFETY: a null proposed rect asks for the image at its natural size.
    let sheet = unsafe {
        spritesheet.CGImageForProposedRect_context_hints(std::ptr::null_mut(), None, None)
    }?;

    let frame_width = CGImage::width(Some(&sheet)) as f64 / COLUMNS;
    let frame_height = CGImage::height(Some(&sheet)) as f64 / rows as f64;
    // CGImage pixel coordinates start at the top-left, like sheet rows.
    let rect = CGRect::new(
        CGPoint::new(
            KEY_COLUMN as f64 * frame_width,
            KEY_ROW as f64 * frame_height,
        ),
        CGSize::new(frame_width, frame_height),
    );
    let frame = CGImage::with_image_in_rect(Some(&sheet), rect)?;

    let width = frame_width as usize;
    let height = frame_height as usize;
    let context = bitmap_context(width, height)?;
    let bounds = CGRect::new(
        CGPoint::new(0.0, 0.0),
        CGSize::new(frame_width, frame_height),
    );
    CGContext::draw_image(Some(&context), bounds, Some(&frame));

    let mut pixels = Rgba::new(width, height);
    copy_rows(&context, &mut pixels, Direction::OutOfContext)?;
    Some(pixels)
}

/// Resamples `pixels` to `width` x `height` with high-quality interpolation.
fn scaled(pixels: &Rgba, width: usize, height: usize) -> Option<Rgba> {
    let source = cg_image(pixels)?;
    let context = bitmap_context(width, height)?;
    CGContext::set_interpolation_quality(Some(&context), CGInterpolationQuality::High);
    let bounds = CGRect::new(
        CGPoint::new(0.0, 0.0),
        CGSize::new(width as f64, height as f64),
    );
    CGContext::draw_image(Some(&context), bounds, Some(&source));

    let mut result = Rgba::new(width, height);
    copy_rows(&context, &mut result, Direction::OutOfContext)?;
    Some(result)
}

/// Copies `pixels` into a new image.
pub(super) fn cg_image(pixels: &Rgba) -> Option<CFRetained<CGImage>> {
    let context = bitmap_context(pixels.width, pixels.height)?;
    let mut source = pixels.clone();
    copy_rows(&context, &mut source, Direction::IntoContext)?;
    CGBitmapContextCreateImage(Some(&context))
}

/// An RGBA drawing context whose memory CoreGraphics owns, so images made
/// from it stay valid however CoreGraphics shares that memory.
fn bitmap_context(width: usize, height: usize) -> Option<CFRetained<CGContext>> {
    let color_space = CGColorSpace::new_device_rgb()?;
    let bitmap_info = CGImageAlphaInfo::PremultipliedLast.0 | CGImageByteOrderInfo::Order32Big.0;
    // SAFETY: a null data pointer makes CoreGraphics allocate the buffer;
    // bytes-per-row 0 lets it choose the row stride.
    unsafe {
        CGBitmapContextCreate(
            std::ptr::null_mut(),
            width,
            height,
            8,
            0,
            Some(&color_space),
            bitmap_info,
        )
    }
}

enum Direction {
    IntoContext,
    OutOfContext,
}

/// Copies pixel rows between `pixels` and the context's buffer, honouring
/// the context's row stride (which may include padding).
fn copy_rows(context: &CGContext, pixels: &mut Rgba, direction: Direction) -> Option<()> {
    let data = CGBitmapContextGetData(Some(context)).cast::<u8>();
    let stride = CGBitmapContextGetBytesPerRow(Some(context));
    let row_bytes = pixels.width * 4;
    if data.is_null() || stride < row_bytes {
        return None;
    }
    for row in 0..pixels.height {
        let ours = &mut pixels.pixels[row * row_bytes..(row + 1) * row_bytes];
        // SAFETY: the context was created for `pixels.width` x `pixels.height`
        // RGBA pixels, so each row starts at `row * stride` and holds at
        // least `row_bytes` bytes; we hold the only reference to the context.
        let theirs = unsafe { std::slice::from_raw_parts_mut(data.add(row * stride), row_bytes) };
        match direction {
            Direction::IntoContext => theirs.copy_from_slice(ours),
            Direction::OutOfContext => ours.copy_from_slice(theirs),
        }
    }
    Some(())
}
