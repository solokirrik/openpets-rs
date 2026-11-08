//! The transparent floating panel that shows the pet, its speech bubble,
//! the timer countdown and the pixel-art `?`/`!` emote.
//!
//! The panel is wider and taller than the pet so the bubble fits above it.
//! It ignores the mouse except while the cursor is over the pet itself, so
//! the empty area never blocks clicks meant for windows underneath.

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AnyThread, MainThreadOnly};
use objc2_app_kit::{
    NSBackingStoreType, NSBox, NSBoxType, NSColor, NSFont, NSFontWeightSemibold, NSImage, NSPanel,
    NSStatusWindowLevel, NSTextField, NSTitlePosition, NSView, NSWindowCollectionBehavior,
    NSWindowStyleMask,
};
use objc2_core_foundation::CFRetained;
use objc2_foundation::{
    MainThreadMarker, NSArray, NSData, NSNumber, NSPoint, NSRect, NSSize, NSString, ns_string,
};
use objc2_quartz_core::{
    CAKeyframeAnimation, CALayer, CAMediaTiming, CATransaction, kCAFilterNearest,
};

use super::pet_icon;
use crate::emote;
use crate::pet::{COLUMNS, FRAME_HEIGHT, FRAME_WIDTH};
use crate::reaction::Badge;

pub const PET_SCALE: f64 = 0.5;
pub const PET_WIDTH: f64 = FRAME_WIDTH * PET_SCALE;
pub const PET_HEIGHT: f64 = FRAME_HEIGHT * PET_SCALE;

const WINDOW_WIDTH: f64 = 240.0;
/// Room above the pet for the pill, the emote and the bubble stacked up.
const BUBBLE_SPACE: f64 = 150.0;
const WINDOW_HEIGHT: f64 = PET_HEIGHT + BUBBLE_SPACE;
/// Horizontal offset of the pet inside the panel.
const PET_INSET_X: f64 = (WINDOW_WIDTH - PET_WIDTH) / 2.0;

const BUBBLE_TEXT_WIDTH: f64 = 200.0;
const BUBBLE_PADDING_X: f64 = 10.0;
const BUBBLE_PADDING_Y: f64 = 6.0;
const BUBBLE_GAP: f64 = 4.0;

const PILL_HEIGHT: f64 = 16.0;
const PILL_PADDING_X: f64 = 6.0;
/// Frames leave empty space above the pet's head; the pill sits in it.
const PILL_OVERLAP: f64 = 10.0;

/// Points per emote pixel: big, chunky pixels like the pets themselves.
const EMOTE_PIXEL: f64 = 4.0;
/// The emote sinks a little into the empty space above the pet's head.
const EMOTE_OVERLAP: f64 = 6.0;
const EMOTE_GAP: f64 = 2.0;
/// One small bounce, then a rest, repeated so a waiting emote catches the eye.
const EMOTE_BOUNCE_SECONDS: f64 = 2.0;

pub struct PetWindow {
    panel: Retained<NSPanel>,
    sprite: Retained<CALayer>,
    bubble: Retained<NSBox>,
    bubble_label: Retained<NSTextField>,
    pill: Retained<NSBox>,
    pill_label: Retained<NSTextField>,
    emote: Retained<CALayer>,
    /// The spritesheet the layer shows; kept alive here.
    image: Option<Retained<NSImage>>,
    rows: f64,
    ignores_mouse: bool,
}

impl PetWindow {
    /// `pet_view` receives the pet's mouse events; it is placed over the
    /// sprite at [`pet_frame`].
    pub fn new(mtm: MainThreadMarker, pet_view: &NSView) -> PetWindow {
        let panel = create_panel(mtm);
        let content = panel.contentView().expect("panel has a content view");
        content.setWantsLayer(true);

        let sprite = CALayer::new();
        sprite.setFrame(pet_frame());
        content
            .layer()
            .expect("layer-backed content view")
            .addSublayer(&sprite);

        pet_view.setFrame(pet_frame());
        content.addSubview(pet_view);

        let (bubble, bubble_label) = create_bubble(mtm);
        content.addSubview(&bubble);

        // Added after the pet view, so it draws on top; the panel only takes
        // the mouse over the pet, so the pill itself is never clickable.
        let (pill, pill_label) = create_pill(mtm);
        content.addSubview(&pill);
        // A plain layer added last, so it draws above the views.
        let emote = CALayer::new();
        // SAFETY: the filter constant is an immutable static.
        emote.setMagnificationFilter(unsafe { kCAFilterNearest });
        emote.setHidden(true);
        // AppKit may reorder its views' layers; keep the emote in front.
        emote.setZPosition(1.0);
        content
            .layer()
            .expect("layer-backed content view")
            .addSublayer(&emote);

        panel.orderFrontRegardless();

        PetWindow {
            panel,
            sprite,
            bubble,
            bubble_label,
            pill,
            pill_label,
            emote,
            image: None,
            rows: 9.0,
            ignores_mouse: true,
        }
    }

    /// Shows a new spritesheet. Fails when the bytes are not a decodable image.
    pub fn set_spritesheet(&mut self, bytes: &[u8]) -> Result<(), String> {
        let data = NSData::with_bytes(bytes);
        let image =
            NSImage::initWithData(NSImage::alloc(), &data).ok_or("image could not be decoded")?;

        let size = image.size();
        let rows = (size.height / FRAME_HEIGHT).round();
        let columns = (size.width / FRAME_WIDTH).round();
        if rows < 9.0 || columns != COLUMNS {
            return Err(format!(
                "expected an 8-column grid of 192x208 frames with at least 9 rows, got {}x{}",
                size.width, size.height
            ));
        }

        let contents: &AnyObject = &image;
        without_animation(|| {
            // SAFETY: CALayer accepts an NSImage as its contents on macOS.
            unsafe { self.sprite.setContents(Some(contents)) };
        });
        self.rows = rows;
        self.image = Some(image);
        eprintln!("[pet-window] spritesheet loaded with {rows} rows");
        Ok(())
    }

    /// The spritesheet on screen and its row count.
    pub fn spritesheet(&self) -> Option<(&NSImage, usize)> {
        let image = self.image.as_deref()?;
        Some((image, self.rows as usize))
    }

    pub fn show_frame(&self, row: u32, column: u32) {
        let width = 1.0 / COLUMNS;
        let height = 1.0 / self.rows;
        // contentsRect is in unit coordinates with the origin at the
        // bottom-left on macOS, while sheet rows count from the top.
        let x = f64::from(column) * width;
        let y = 1.0 - (f64::from(row) + 1.0) * height;
        let rect = NSRect::new(NSPoint::new(x, y), NSSize::new(width, height));
        without_animation(|| self.sprite.setContentsRect(rect));
    }

    /// Places the pet's bottom-left corner at screen point (`x`, `y`).
    pub fn move_pet_to(&self, x: f64, y: f64) {
        self.panel.setFrameOrigin(NSPoint::new(x - PET_INSET_X, y));
    }

    pub fn set_ignores_mouse(&mut self, ignores: bool) {
        if self.ignores_mouse != ignores {
            self.panel.setIgnoresMouseEvents(ignores);
            self.ignores_mouse = ignores;
        }
    }

    pub fn show_bubble(&self, message: &str) {
        self.bubble_label
            .setStringValue(&NSString::from_str(message));
        let text = self.bubble_label.fittingSize();
        let text = NSSize::new(text.width.min(BUBBLE_TEXT_WIDTH), text.height);
        self.bubble_label.setFrame(NSRect::new(
            NSPoint::new(BUBBLE_PADDING_X, BUBBLE_PADDING_Y),
            text,
        ));

        let width = text.width + BUBBLE_PADDING_X * 2.0;
        let height = (text.height + BUBBLE_PADDING_Y * 2.0).min(BUBBLE_SPACE - BUBBLE_GAP);
        let origin = NSPoint::new((WINDOW_WIDTH - width) / 2.0, self.bubble_bottom());
        self.bubble
            .setFrame(NSRect::new(origin, NSSize::new(width, height)));
        self.bubble.setHidden(false);
    }

    pub fn hide_bubble(&self) {
        self.bubble.setHidden(true);
    }

    /// The bubble rises above the pill and the emote while they show.
    fn bubble_bottom(&self) -> f64 {
        let mut bottom = PET_HEIGHT + BUBBLE_GAP;
        if !self.pill.isHidden() {
            let pill = self.pill.frame();
            bottom = bottom.max(pill.origin.y + pill.size.height + BUBBLE_GAP);
        }
        if !self.emote.isHidden() {
            let emote = self.emote.frame();
            bottom = bottom.max(emote.origin.y + emote.size.height + BUBBLE_GAP);
        }
        bottom
    }

    /// Shows the pixel-art emote for `badge` over the pet's head, bouncing
    /// gently until hidden.
    pub fn show_badge(&self, badge: Badge) {
        let art = emote::render(badge);
        let Some(image) = pet_icon::cg_image(&art) else {
            eprintln!("[pet-window] cannot draw the {badge:?} emote");
            return;
        };
        let size = NSSize::new(
            art.width as f64 * EMOTE_PIXEL,
            art.height as f64 * EMOTE_PIXEL,
        );
        // SAFETY: a CGImage is a valid CALayer contents object.
        let contents: &AnyObject = unsafe { &*CFRetained::as_ptr(&image).as_ptr().cast() };
        without_animation(|| {
            // SAFETY: as above.
            unsafe { self.emote.setContents(Some(contents)) };
            self.emote
                .setBounds(NSRect::new(NSPoint::new(0.0, 0.0), size));
            self.emote.setHidden(false);
            self.place_emote();
        });
        self.emote.removeAllAnimations();
        self.emote
            .addAnimation_forKey(&bounce(), Some(ns_string!("bounce")));
    }

    pub fn hide_badge(&self) {
        self.emote.removeAllAnimations();
        without_animation(|| self.emote.setHidden(true));
        self.restack_bubble();
    }

    /// Centres the emote over the head, above the timer pill when it shows.
    fn place_emote(&self) {
        let size = self.emote.bounds().size;
        let bottom = if self.pill.isHidden() {
            PET_HEIGHT - EMOTE_OVERLAP
        } else {
            let pill = self.pill.frame();
            pill.origin.y + pill.size.height + EMOTE_GAP
        };
        let origin = NSPoint::new((WINDOW_WIDTH - size.width) / 2.0, bottom);
        without_animation(|| self.emote.setFrame(NSRect::new(origin, size)));
        self.restack_bubble();
    }

    /// Re-stacks a showing bubble directly above the pill and emote, which
    /// can appear or go after it (an approval sends its line before its badge).
    fn restack_bubble(&self) {
        if self.bubble.isHidden() {
            return;
        }
        let mut frame = self.bubble.frame();
        frame.origin.y = self.bubble_bottom();
        self.bubble.setFrame(frame);
    }

    /// Shows `text` (the time left) in the pill above the pet's head.
    pub fn show_countdown(&self, text: &str) {
        self.pill_label.setStringValue(&NSString::from_str(text));
        let text_size = self.pill_label.fittingSize();
        let width = text_size.width + PILL_PADDING_X * 2.0;
        self.pill_label.setFrame(NSRect::new(
            NSPoint::new(PILL_PADDING_X, (PILL_HEIGHT - text_size.height) / 2.0),
            text_size,
        ));
        let origin = NSPoint::new((WINDOW_WIDTH - width) / 2.0, PET_HEIGHT - PILL_OVERLAP);
        self.pill
            .setFrame(NSRect::new(origin, NSSize::new(width, PILL_HEIGHT)));
        self.pill.setHidden(false);
        self.place_emote();
    }

    pub fn hide_countdown(&self) {
        self.pill.setHidden(true);
        self.place_emote();
    }
}

/// Where the pet sits inside the panel's content view.
pub fn pet_frame() -> NSRect {
    NSRect::new(
        NSPoint::new(PET_INSET_X, 0.0),
        NSSize::new(PET_WIDTH, PET_HEIGHT),
    )
}

fn create_panel(mtm: MainThreadMarker) -> Retained<NSPanel> {
    let frame = NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(WINDOW_WIDTH, WINDOW_HEIGHT),
    );
    let style = NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel;
    let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
        NSPanel::alloc(mtm),
        frame,
        style,
        NSBackingStoreType::Buffered,
        false,
    );

    // SAFETY: the panel is owned by `PetWindow` and never closed, so it must
    // not release itself.
    unsafe { panel.setReleasedWhenClosed(false) };
    panel.setOpaque(false);
    panel.setBackgroundColor(Some(&NSColor::clearColor()));
    panel.setHasShadow(false);
    panel.setLevel(NSStatusWindowLevel);
    panel.setFloatingPanel(true);
    panel.setHidesOnDeactivate(false);
    panel.setIgnoresMouseEvents(true);
    panel.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::Stationary
            | NSWindowCollectionBehavior::FullScreenAuxiliary
            | NSWindowCollectionBehavior::IgnoresCycle,
    );
    panel
}

fn create_bubble(mtm: MainThreadMarker) -> (Retained<NSBox>, Retained<NSTextField>) {
    let bubble = NSBox::new(mtm);
    bubble.setBoxType(NSBoxType::Custom);
    bubble.setTitlePosition(NSTitlePosition::NoTitle);
    bubble.setContentViewMargins(NSSize::new(0.0, 0.0));
    bubble.setFillColor(&NSColor::whiteColor());
    bubble.setBorderColor(&NSColor::colorWithWhite_alpha(0.0, 0.15));
    bubble.setBorderWidth(1.0);
    bubble.setCornerRadius(10.0);
    bubble.setHidden(true);

    let label = NSTextField::wrappingLabelWithString(&NSString::new(), mtm);
    label.setSelectable(false);
    label.setPreferredMaxLayoutWidth(BUBBLE_TEXT_WIDTH);
    label.setFont(Some(&NSFont::systemFontOfSize(12.0)));
    label.setTextColor(Some(&NSColor::blackColor()));

    let content: Retained<NSView> = bubble.contentView().expect("box has a content view");
    content.addSubview(&label);
    (bubble, label)
}

/// A hop of a few points, then a rest, forever.
fn bounce() -> Retained<CAKeyframeAnimation> {
    let animation =
        CAKeyframeAnimation::animationWithKeyPath(Some(ns_string!("transform.translation.y")));
    let numbers = |values: &[f64]| {
        let numbers: Vec<Retained<NSNumber>> = values
            .iter()
            .map(|value| NSNumber::new_f64(*value))
            .collect();
        NSArray::from_retained_slice(&numbers)
    };
    let heights = numbers(&[0.0, 5.0, 0.0, 2.0, 0.0, 0.0]);
    // SAFETY: an array of NSNumber is an array of objects (NSArray is
    // covariant), and a translation animation takes NSNumber values.
    unsafe {
        let heights: Retained<NSArray> = Retained::cast_unchecked(heights);
        animation.setValues(Some(&heights));
    }
    animation.setKeyTimes(Some(&numbers(&[0.0, 0.08, 0.16, 0.22, 0.28, 1.0])));
    animation.setDuration(EMOTE_BOUNCE_SECONDS);
    animation.setRepeatCount(f32::INFINITY);
    animation
}

/// A dark, rounded label for the countdown.
fn create_pill(mtm: MainThreadMarker) -> (Retained<NSBox>, Retained<NSTextField>) {
    let pill = NSBox::new(mtm);
    pill.setBoxType(NSBoxType::Custom);
    pill.setTitlePosition(NSTitlePosition::NoTitle);
    pill.setContentViewMargins(NSSize::new(0.0, 0.0));
    pill.setFillColor(&NSColor::colorWithWhite_alpha(0.0, 0.6));
    pill.setBorderWidth(0.0);
    pill.setCornerRadius(PILL_HEIGHT / 2.0);
    pill.setHidden(true);

    let label = NSTextField::labelWithString(&NSString::new(), mtm);
    label.setSelectable(false);
    // SAFETY: AppKit's font weight constants are immutable statics.
    let weight = unsafe { NSFontWeightSemibold };
    // Fixed-width digits keep the pill from jittering every second.
    label.setFont(Some(&NSFont::monospacedDigitSystemFontOfSize_weight(
        10.0, weight,
    )));
    label.setTextColor(Some(&NSColor::whiteColor()));

    let content: Retained<NSView> = pill.contentView().expect("box has a content view");
    content.addSubview(&label);
    (pill, label)
}

/// Standalone layers animate property changes by default; sprite frames and
/// contents must switch instantly.
fn without_animation(change: impl FnOnce()) {
    CATransaction::begin();
    CATransaction::setDisableActions(true);
    change();
    CATransaction::commit();
}
