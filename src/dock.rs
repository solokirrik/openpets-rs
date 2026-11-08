//! Tracks whether an auto-hiding Dock is showing, from the cursor alone.
//!
//! macOS has no public API or notification for the Dock sliding in. It
//! follows a simple rule though: it appears when the cursor touches the
//! bottom edge of a display and hides once the cursor leaves the Dock area.
//! Following the same rule lets the pet step up before the Dock covers it.
//!
//! The Dock is centred and only as wide as its icons, so its size is
//! estimated from what it holds; the pet only steps up while any part of it
//! is over that span, so the Dock never covers it. Exact geometry would need the Accessibility permission.

/// How close to the bottom edge (points) counts as touching it.
const EDGE: f64 = 2.0;

/// Room the Dock takes around its icons: padding plus the gap macOS leaves
/// under a floating Dock. Estimated, not measured.
const DOCK_PADDING: f64 = 20.0;
/// Width of the divider between apps and the stacks/Trash, as a share of
/// the icon size. Estimated, not measured.
const DIVIDER_SHARE: f64 = 0.3;

/// The Dock settings that matter here, read from `com.apple.dock`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DockSettings {
    pub autohide: bool,
    pub at_bottom: bool,
    /// Icon size in points (`tilesize`).
    pub tile_size: f64,
    /// Icons in the Dock: apps, stacks and the Trash.
    pub tiles: usize,
    /// Dividers between the Dock's sections.
    pub dividers: usize,
}

/// The revealed Dock's size in points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DockSize {
    pub width: f64,
    pub height: f64,
}

impl DockSettings {
    /// The revealed Dock's size, or `None` when it never covers the bottom
    /// edge on its own (always shown, or on the left/right side).
    pub fn reveal_size(self) -> Option<DockSize> {
        if !(self.autohide && self.at_bottom) {
            return None;
        }
        let icons = self.tiles as f64 * self.tile_size;
        let dividers = self.dividers as f64 * self.tile_size * DIVIDER_SHARE;
        Some(DockSize {
            width: icons + dividers + DOCK_PADDING,
            height: self.tile_size + DOCK_PADDING,
        })
    }
}

/// Where the revealed Dock is: centred on the display it showed on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Revealed {
    /// The display's bottom edge, where the Dock rests.
    pub bottom: f64,
    pub height: f64,
    /// The Dock's own horizontal span.
    pub left: f64,
    pub right: f64,
    /// The display's horizontal span; the Dock stays up while the cursor is
    /// anywhere this low on it.
    display_min_x: f64,
    display_max_x: f64,
}

impl Revealed {
    /// Whether anything between `left` and `right` is over the Dock.
    pub fn overlaps(&self, left: f64, right: f64) -> bool {
        left < self.right && right > self.left
    }
}

#[derive(Debug, Default)]
pub struct DockWatch {
    revealed: Option<Revealed>,
}

impl DockWatch {
    /// Updates from the cursor position. `display` is the frame of the
    /// display under the cursor as (min_x, bottom, max_x); `size` is
    /// [`DockSettings::reveal_size`], read only when the cursor touches the
    /// edge because that is rare (and picks up Dock changes each time).
    pub fn update(
        &mut self,
        cursor: (f64, f64),
        display: (f64, f64, f64),
        size: impl FnOnce() -> Option<DockSize>,
    ) -> Option<Revealed> {
        let (x, y) = cursor;
        let (min_x, bottom, max_x) = display;

        if let Some(revealed) = self.revealed {
            let inside_x = x >= revealed.display_min_x && x < revealed.display_max_x;
            let inside = inside_x && y <= revealed.bottom + revealed.height;
            if !inside {
                self.revealed = None;
            }
        } else if y <= bottom + EDGE {
            self.revealed = size().map(|size| {
                let centre = (min_x + max_x) / 2.0;
                Revealed {
                    bottom,
                    height: size.height,
                    left: centre - size.width / 2.0,
                    right: centre + size.width / 2.0,
                    display_min_x: min_x,
                    display_max_x: max_x,
                }
            });
        }
        self.revealed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DISPLAY: (f64, f64, f64) = (0.0, 0.0, 1800.0);

    fn settings() -> DockSettings {
        DockSettings {
            autohide: true,
            at_bottom: true,
            tile_size: 64.0,
            tiles: 10,
            dividers: 1,
        }
    }

    fn hidden_dock() -> Option<DockSize> {
        settings().reveal_size()
    }

    #[test]
    fn dock_shows_at_the_bottom_edge_and_hides_once_the_cursor_leaves_it() {
        let mut watch = DockWatch::default();

        assert!(watch.update((900.0, 400.0), DISPLAY, hidden_dock).is_none());
        let revealed = watch.update((900.0, 0.0), DISPLAY, hidden_dock).unwrap();
        assert_eq!(revealed.height, 84.0);
        // Ten 64-point icons, centred: the screen edges are not under it.
        assert!(revealed.overlaps(850.0, 950.0));
        assert!(!revealed.overlaps(100.0, 200.0) && !revealed.overlaps(1600.0, 1700.0));
        // Partly over the Dock's end already counts, so the Dock never hides the pet.
        assert!(revealed.overlaps(revealed.left - 60.0, revealed.left + 10.0));
        // Moving across the Dock keeps it up.
        assert!(watch.update((300.0, 50.0), DISPLAY, hidden_dock).is_some());
        assert!(watch.update((300.0, 200.0), DISPLAY, hidden_dock).is_none());
    }

    #[test]
    fn bigger_icons_or_more_apps_make_a_bigger_dock() {
        let small = settings().reveal_size().unwrap();
        let bigger = DockSettings {
            tile_size: 96.0,
            ..settings()
        }
        .reveal_size()
        .unwrap();
        let fuller = DockSettings {
            tiles: 15,
            ..settings()
        }
        .reveal_size()
        .unwrap();

        assert!(bigger.height > small.height && bigger.width > small.width);
        assert!(fuller.width > small.width);
    }

    #[test]
    fn a_dock_that_never_hides_or_sits_on_the_side_is_ignored() {
        let mut watch = DockWatch::default();
        let side = || {
            DockSettings {
                at_bottom: false,
                ..settings()
            }
            .reveal_size()
        };

        assert!(watch.update((900.0, 0.0), DISPLAY, side).is_none());
    }
}
