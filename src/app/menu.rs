//! The menu bar item: pet picker, timer, pause, pets folder and quit.
//!
//! `MenuTarget` receives AppKit menu actions and forwards them to the `App`.
//! The menu is rebuilt each time it opens so newly installed pets show up.

use std::cell::RefCell;
use std::rc::Weak;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject, Sel};
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSControlStateValueOff, NSControlStateValueOn, NSImage, NSMenu, NSMenuDelegate,
    NSMenuItem, NSStatusBar, NSStatusItem, NSVariableStatusItemLength,
};
use objc2_foundation::{MainThreadMarker, NSObject, NSObjectProtocol, NSString, ns_string};

use super::{App, with_app};
use crate::timer::PRESET_MINUTES;

pub struct MenuTargetIvars {
    app: Weak<RefCell<App>>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and MenuTarget does
    // not implement Drop.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = MenuTargetIvars]
    pub struct MenuTarget;

    unsafe impl NSObjectProtocol for MenuTarget {}

    unsafe impl NSMenuDelegate for MenuTarget {
        #[unsafe(method(menuNeedsUpdate:))]
        fn menu_needs_update(&self, menu: &NSMenu) {
            self.with_app(|app| app.fill_menu(menu));
        }
    }

    impl MenuTarget {
        #[unsafe(method(selectPet:))]
        fn select_pet(&self, sender: &NSMenuItem) {
            let index = sender.tag();
            self.with_app(|app| app.select_pet(index as usize));
        }

        #[unsafe(method(togglePause:))]
        fn toggle_pause(&self, _sender: &NSMenuItem) {
            self.with_app(|app| app.toggle_pause());
        }

        #[unsafe(method(startTimer:))]
        fn start_timer(&self, sender: &NSMenuItem) {
            let minutes = sender.tag();
            self.with_app(|app| app.start_timer(minutes as u64));
        }

        #[unsafe(method(cancelTimer:))]
        fn cancel_timer(&self, _sender: &NSMenuItem) {
            self.with_app(|app| app.cancel_timer());
        }

        #[unsafe(method(openPetsFolder:))]
        fn open_pets_folder(&self, _sender: &NSMenuItem) {
            self.with_app(|app| app.open_pets_folder());
        }
    }
);

impl MenuTarget {
    fn new(mtm: MainThreadMarker, app: Weak<RefCell<App>>) -> Retained<MenuTarget> {
        let this = MenuTarget::alloc(mtm).set_ivars(MenuTargetIvars { app });
        // SAFETY: NSObject's `init` has this signature.
        unsafe { msg_send![super(this), init] }
    }

    fn with_app(&self, action: impl FnOnce(&mut App)) {
        with_app(&self.ivars().app, action);
    }
}

/// Owns the status item, its menu and the action target.
pub struct StatusMenu {
    item: Retained<NSStatusItem>,
    _menu: Retained<NSMenu>,
    target: Retained<MenuTarget>,
}

impl StatusMenu {
    pub fn new(mtm: MainThreadMarker, app: Weak<RefCell<App>>) -> StatusMenu {
        let target = MenuTarget::new(mtm, app);

        let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
        if let Some(button) = item.button(mtm) {
            button.setTitle(ns_string!("🐾"));
        }

        let menu = new_menu(mtm, &target);
        item.setMenu(Some(&menu));

        StatusMenu {
            item,
            _menu: menu,
            target,
        }
    }

    /// Shows `icon` in the menu bar, or the paw emoji when there is none.
    pub fn set_icon(&self, icon: Option<&NSImage>) {
        let Some(button) = self.item.button(self.target.mtm()) else {
            return;
        };
        button.setImage(icon);
        let title = if icon.is_some() {
            ns_string!("")
        } else {
            ns_string!("🐾")
        };
        button.setTitle(title);
    }

    /// A fresh copy of the status menu, for right-clicking the pet.
    pub fn context_menu(&self) -> Retained<NSMenu> {
        new_menu(self.target.mtm(), &self.target)
    }

    /// Replaces the menu's items. `pets` are display names; `selected` and
    /// `paused` set the check marks; `timing` offers Cancel Timer.
    pub fn fill(
        &self,
        menu: &NSMenu,
        pets: &[String],
        selected: Option<usize>,
        paused: bool,
        timing: bool,
    ) {
        let mtm = self.target.mtm();
        menu.removeAllItems();

        let pet_menu = NSMenu::new(mtm);
        for (index, name) in pets.iter().enumerate() {
            let item = self.action_item(&NSString::from_str(name), sel!(selectPet:));
            item.setTag(index as isize);
            item.setState(check_state(Some(index) == selected));
            pet_menu.addItem(&item);
        }
        pet_menu.addItem(&NSMenuItem::separatorItem(mtm));
        pet_menu.addItem(&self.action_item(ns_string!("Open Pets Folder…"), sel!(openPetsFolder:)));

        let pet_item = NSMenuItem::new(mtm);
        pet_item.setTitle(ns_string!("Pet"));
        pet_item.setSubmenu(Some(&pet_menu));
        menu.addItem(&pet_item);
        menu.addItem(&self.timer_item(timing));

        let pause = self.action_item(ns_string!("Pause Walking"), sel!(togglePause:));
        pause.setState(check_state(paused));
        menu.addItem(&pause);

        menu.addItem(&NSMenuItem::separatorItem(mtm));
        let quit = new_item(
            mtm,
            ns_string!("Quit OpenPets"),
            sel!(terminate:),
            ns_string!("q"),
        );
        let application = NSApplication::sharedApplication(mtm);
        let application: &AnyObject = &application;
        // SAFETY: NSApplication responds to `terminate:`.
        unsafe { quit.setTarget(Some(application)) };
        menu.addItem(&quit);
    }

    fn timer_item(&self, timing: bool) -> Retained<NSMenuItem> {
        let mtm = self.target.mtm();
        let timer_menu = NSMenu::new(mtm);
        for minutes in PRESET_MINUTES {
            let title = if minutes % 60 == 0 {
                format!("{} hour", minutes / 60)
            } else {
                format!("{minutes} min")
            };
            let item = self.action_item(&NSString::from_str(&title), sel!(startTimer:));
            item.setTag(minutes as isize);
            timer_menu.addItem(&item);
        }
        if timing {
            timer_menu.addItem(&NSMenuItem::separatorItem(mtm));
            timer_menu.addItem(&self.action_item(ns_string!("Cancel Timer"), sel!(cancelTimer:)));
        }

        let item = NSMenuItem::new(mtm);
        item.setTitle(ns_string!("Timer"));
        item.setSubmenu(Some(&timer_menu));
        item
    }

    fn action_item(&self, title: &NSString, action: Sel) -> Retained<NSMenuItem> {
        let item = new_item(self.target.mtm(), title, action, ns_string!(""));
        let target: &AnyObject = &self.target;
        // SAFETY: MenuTarget implements every selector used with `action_item`.
        unsafe { item.setTarget(Some(target)) };
        item
    }
}

/// An empty menu that `MenuTarget` fills each time it opens.
fn new_menu(mtm: MainThreadMarker, target: &MenuTarget) -> Retained<NSMenu> {
    let menu = NSMenu::new(mtm);
    menu.setDelegate(Some(ProtocolObject::from_ref(target)));
    menu
}

fn new_item(
    mtm: MainThreadMarker,
    title: &NSString,
    action: Sel,
    key: &NSString,
) -> Retained<NSMenuItem> {
    // SAFETY: the action is a valid selector for the item's target.
    unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            title,
            Some(action),
            key,
        )
    }
}

fn check_state(on: bool) -> isize {
    if on {
        NSControlStateValueOn
    } else {
        NSControlStateValueOff
    }
}
