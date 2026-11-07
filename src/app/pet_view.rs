//! The view covering the pet sprite. It receives the mouse events for the
//! pet (press, drag, release, right-click) and forwards them to the `App`.

use std::cell::RefCell;
use std::rc::Weak;

use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{NSEvent, NSMenu, NSView};
use objc2_foundation::{MainThreadMarker, NSRect};

use super::{App, with_app};

pub struct PetViewIvars {
    app: Weak<RefCell<App>>,
}

define_class!(
    // SAFETY: NSView has no extra subclassing requirements for event
    // handling, and PetView does not implement Drop.
    #[unsafe(super = NSView)]
    #[thread_kind = MainThreadOnly]
    #[ivars = PetViewIvars]
    pub struct PetView;

    impl PetView {
        /// The pet's panel never becomes key, so the first click must count.
        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, _event: &NSEvent) {
            let point = NSEvent::mouseLocation();
            with_app(&self.ivars().app, |app| app.mouse_down(point));
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, _event: &NSEvent) {
            let point = NSEvent::mouseLocation();
            with_app(&self.ivars().app, |app| app.mouse_dragged(point));
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, _event: &NSEvent) {
            with_app(&self.ivars().app, |app| app.mouse_up());
        }

        #[unsafe(method(rightMouseDown:))]
        fn right_mouse_down(&self, event: &NSEvent) {
            // Build the menu inside the borrow, but show it after: the menu
            // runs its own event loop and its actions need to borrow the app.
            let mut menu: Option<Retained<NSMenu>> = None;
            with_app(&self.ivars().app, |app| menu = Some(app.context_menu()));
            if let Some(menu) = menu {
                NSMenu::popUpContextMenu_withEvent_forView(&menu, event, self);
            }
        }
    }
);

impl PetView {
    pub fn new(mtm: MainThreadMarker, frame: NSRect, app: Weak<RefCell<App>>) -> Retained<PetView> {
        let this = PetView::alloc(mtm).set_ivars(PetViewIvars { app });
        // SAFETY: NSView's `initWithFrame:` has this signature.
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }
}
