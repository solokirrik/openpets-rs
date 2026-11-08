//! The menu bar app: one pet window, the status menu and the IPC socket.
//!
//! Everything runs on the main thread. A 60 Hz timer animates the pet and
//! answers agent requests, which the IPC thread hands over a channel; mouse
//! and menu events arrive as AppKit callbacks through `with_app`.

mod menu;
mod pet_icon;
mod pet_view;
mod pet_window;

use std::cell::RefCell;
use std::ptr::NonNull;
use std::rc::{Rc, Weak};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::NSWorkspace;
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSEvent, NSMenu, NSScreen, NSSound,
};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSTimer};
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString, NSUserDefaults, ns_string};

use crate::dock::{DockSettings, DockWatch, Revealed};
use crate::ipc::{self, Incoming, PetInfo, Request, Response};
use crate::motion::{Anim, Bounds, Motion};
use crate::paths;
use crate::pet::{self, PetEntry};
use crate::reaction::Reaction;
use crate::timer::{self, Timer};
use menu::StatusMenu;
use pet_icon::Theme;
use pet_view::PetView;
use pet_window::{PET_HEIGHT, PET_WIDTH, PetWindow};

const TICK_SECONDS: f64 = 1.0 / 60.0;
/// A press that moves less than this is a click, not a drag.
const DRAG_THRESHOLD: f64 = 3.0;
/// How often to check whether the system switched between light and dark.
const THEME_CHECK_INTERVAL: Duration = Duration::from_secs(1);
const TIMER_DONE_MESSAGE: &str = "Time's up!";
const TIMER_DONE_BUBBLE: Duration = Duration::from_secs(5);

pub fn run() -> Result<(), String> {
    let mtm = MainThreadMarker::new().ok_or("the app must start on the main thread")?;
    let socket = paths::socket_path();

    if ipc::send(&socket, &Request::Status).is_ok() {
        eprintln!("[app] OpenPets is already running");
        return Ok(());
    }
    prepare_data_dir()?;
    let _ = std::fs::remove_file(&socket);

    let (deliver, inbox) = mpsc::channel();
    ipc::serve(&socket, deliver)
        .map_err(|error| format!("cannot listen on the socket: {error}"))?;
    eprintln!("[app] listening for agents");

    let application = NSApplication::sharedApplication(mtm);
    application.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    let app = Rc::new_cyclic(|weak| RefCell::new(App::new(mtm, weak.clone(), inbox)));
    let tick = RcBlock::new(move |_timer: NonNull<NSTimer>| {
        // A panic here would abort the app, so skip a frame instead.
        if let Ok(mut app) = app.try_borrow_mut() {
            app.tick();
        }
    });
    // SAFETY: the block only touches main-thread state, and the timer is
    // scheduled on the main run loop.
    let _timer =
        unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(TICK_SECONDS, true, &tick) };

    application.run();
    Ok(())
}

fn prepare_data_dir() -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    let dir = paths::data_dir();
    std::fs::create_dir_all(&dir)
        .map_err(|error| format!("cannot create {}: {error}", dir.display()))?;
    // Only this user may reach the socket inside.
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("cannot restrict {}: {error}", dir.display()))
}

/// A mouse press that started on the pet.
struct Press {
    /// Cursor position relative to the pet's bottom-left corner.
    grab_offset: NSPoint,
    start: NSPoint,
    dragging: bool,
}

pub struct App {
    mtm: MainThreadMarker,
    window: PetWindow,
    status_menu: StatusMenu,
    inbox: Receiver<Incoming>,
    motion: Motion,
    /// Pets offered in the menu, refreshed each time it opens.
    pets: Vec<PetEntry>,
    /// The pet on screen.
    current: PetEntry,
    anim: Anim,
    anim_started: Instant,
    last_tick: Instant,
    bubble_until: Option<Instant>,
    press: Option<Press>,
    /// The theme the menu bar icon was drawn for.
    theme: Theme,
    theme_checked: Instant,
    timer: Option<Timer>,
    /// The countdown text on screen, so the pill is redrawn once a second.
    countdown: String,
    dock: DockWatch,
    /// Where an auto-hiding Dock is showing right now, if anywhere.
    dock_revealed: Option<Revealed>,
}

impl App {
    fn new(mtm: MainThreadMarker, weak: Weak<RefCell<App>>, inbox: Receiver<Incoming>) -> App {
        let start = initial_position(mtm);
        let now = Instant::now();
        let pet_view = PetView::new(mtm, pet_window::pet_frame(), weak.clone());
        let mut app = App {
            mtm,
            window: PetWindow::new(mtm, &pet_view),
            status_menu: StatusMenu::new(mtm, weak),
            inbox,
            motion: Motion::new(start.x, start.y, random_seed()),
            pets: pet::list_pets(&paths::pets_dir()),
            current: PetEntry::built_in(),
            anim: Anim::Idle,
            anim_started: now,
            last_tick: now,
            bubble_until: None,
            press: None,
            theme: Theme::current(mtm),
            theme_checked: now,
            timer: None,
            countdown: String::new(),
            dock: DockWatch::default(),
            dock_revealed: None,
        };

        let saved = std::fs::read_to_string(paths::selected_pet_file()).unwrap_or_default();
        let saved_pet = app.pets.iter().find(|pet| pet.id == saved.trim()).cloned();
        app.show_pet(saved_pet.unwrap_or_else(PetEntry::built_in));
        app.window.move_pet_to(start.x, start.y);
        app
    }

    fn tick(&mut self) {
        let now = Instant::now();
        // Clamp so a stalled run loop (sleep, menu tracking) does not teleport the pet.
        let dt = now.duration_since(self.last_tick).as_secs_f64().min(0.1);
        self.last_tick = now;

        self.answer_requests();
        self.update_click_through();
        self.watch_dock();

        let bounds = self.current_bounds();
        let anim = self.motion.tick(dt, bounds);
        if anim != self.anim {
            self.anim = anim;
            self.anim_started = now;
        }
        let elapsed = now.duration_since(self.anim_started).as_secs_f64();
        let (row, column) = pet::frame_at(self.anim, elapsed);
        self.window.show_frame(row, column);
        self.window.move_pet_to(self.motion.x, self.motion.y);

        if self.bubble_until.is_some_and(|until| now >= until) {
            self.bubble_until = None;
            self.window.hide_bubble();
        }

        self.update_timer();

        if now.duration_since(self.theme_checked) >= THEME_CHECK_INTERVAL {
            self.theme_checked = now;
            self.follow_theme();
        }
    }

    /// Redraws the menu bar icon when the system switches light/dark.
    fn follow_theme(&mut self) {
        let theme = Theme::current(self.mtm);
        if theme != self.theme {
            eprintln!("[icon] system theme is now {theme:?}");
            self.theme = theme;
            self.update_menu_icon();
        }
    }

    fn answer_requests(&mut self) {
        while let Ok(incoming) = self.inbox.try_recv() {
            let response = self.handle(incoming.request);
            // The requester may have timed out and gone away; nothing to do then.
            let _ = incoming.reply.send(response);
        }
    }

    fn handle(&mut self, request: Request) -> Response {
        match request {
            Request::Status => {}
            Request::React { reaction } => {
                eprintln!("[app] reaction {}", reaction.name());
                self.motion.react(reaction);
            }
            Request::Say { message, reaction } => {
                eprintln!("[app] speech bubble ({} chars)", message.chars().count());
                if let Some(reaction) = reaction {
                    self.motion.react(reaction);
                }
                let visible_secs = 3.0 + 0.05 * message.chars().count() as f64;
                self.window.show_bubble(&message);
                self.bubble_until = Some(Instant::now() + Duration::from_secs_f64(visible_secs));
            }
        }
        Response::ok(PetInfo {
            id: self.current.id.clone(),
            name: self.current.name.clone(),
        })
    }

    /// The panel accepts the mouse only while the cursor is over the pet
    /// (or a press is in progress), so the rest of it never blocks clicks.
    fn update_click_through(&mut self) {
        let over_pet = self.pet_rect().contains(NSEvent::mouseLocation());
        self.window
            .set_ignores_mouse(!over_pet && self.press.is_none());
    }

    fn mouse_down(&mut self, point: NSPoint) {
        self.press = Some(Press {
            grab_offset: NSPoint::new(point.x - self.motion.x, point.y - self.motion.y),
            start: point,
            dragging: false,
        });
    }

    fn mouse_dragged(&mut self, point: NSPoint) {
        let Some(press) = &mut self.press else {
            return;
        };
        let moved = (point.x - press.start.x).hypot(point.y - press.start.y);
        if !press.dragging && moved > DRAG_THRESHOLD {
            press.dragging = true;
            self.motion.start_drag();
        }
        if press.dragging {
            let x = point.x - press.grab_offset.x;
            let y = point.y - press.grab_offset.y;
            self.motion.drag_to(x, y);
            // Follow the cursor now rather than on the next tick.
            self.window.move_pet_to(self.motion.x, self.motion.y);
        }
    }

    fn mouse_up(&mut self) {
        let Some(press) = self.press.take() else {
            return;
        };
        if press.dragging {
            self.motion.end_drag();
        } else {
            self.motion.react(Reaction::Waving);
        }
    }

    fn context_menu(&self) -> Retained<NSMenu> {
        self.status_menu.context_menu()
    }

    fn pet_rect(&self) -> PetRect {
        PetRect {
            x: self.motion.x,
            y: self.motion.y,
        }
    }

    /// The visible area of the display under the pet's centre.
    fn current_bounds(&self) -> Bounds {
        let centre = NSPoint::new(
            self.motion.x + PET_WIDTH / 2.0,
            self.motion.y + PET_HEIGHT / 2.0,
        );
        let visible = screen_containing(self.mtm, centre);
        let mut floor_y = visible.origin.y;
        // Step up onto a revealed Dock as soon as any part of the pet is over it.
        if let Some(dock) = self.dock_revealed
            && dock.overlaps(self.motion.x, self.motion.x + PET_WIDTH)
        {
            floor_y = floor_y.max(dock.bottom + dock.height);
        }
        Bounds {
            min_x: visible.origin.x,
            max_x: visible.origin.x + visible.size.width - PET_WIDTH,
            floor_y,
        }
    }

    /// Follows the auto-hiding Dock from the cursor position (see `crate::dock`).
    fn watch_dock(&mut self) {
        let cursor = NSEvent::mouseLocation();
        let display = display_frame(self.mtm, cursor);
        let display = (
            display.origin.x,
            display.origin.y,
            display.origin.x + display.size.width,
        );
        let revealed = self.dock.update((cursor.x, cursor.y), display, || {
            dock_settings().reveal_size()
        });
        match (self.dock_revealed, revealed) {
            (None, Some(dock)) => eprintln!(
                "[dock] shown: x {:.0}..{:.0}, {:.0} pt tall",
                dock.left, dock.right, dock.height
            ),
            (Some(_), None) => eprintln!("[dock] hidden"),
            _ => {}
        }
        self.dock_revealed = revealed;
    }

    /// Shows `pet`, falling back to the bundled pet if it cannot be loaded.
    /// If even that fails, the current spritesheet stays on screen.
    fn show_pet(&mut self, pet: PetEntry) {
        match self.load_spritesheet(&pet) {
            Ok(()) => {
                eprintln!("[app] showing pet {}", pet.id);
                self.current = pet;
                self.update_icons();
            }
            Err(reason) => {
                eprintln!("[app] cannot show pet {}: {reason}", pet.id);
                let fallback = PetEntry::built_in();
                if pet.id == fallback.id {
                    return;
                }
                match self.load_spritesheet(&fallback) {
                    Ok(()) => {
                        self.current = fallback;
                        self.update_icons();
                    }
                    Err(reason) => eprintln!("[app] cannot show the bundled pet either: {reason}"),
                }
            }
        }
    }

    /// Redraws the menu bar and Finder icons from the pet on screen.
    fn update_icons(&self) {
        self.update_menu_icon();
        if let Some((spritesheet, rows)) = self.window.spritesheet() {
            pet_icon::set_app_icon(spritesheet, rows);
        }
    }

    fn update_menu_icon(&self) {
        let Some((spritesheet, rows)) = self.window.spritesheet() else {
            return;
        };
        let menu_icon = pet_icon::menu_bar_icon(spritesheet, rows, self.theme);
        if menu_icon.is_none() {
            eprintln!(
                "[icon] no menu bar icon for {}; using the paw",
                self.current.id
            );
        }
        self.status_menu.set_icon(menu_icon.as_deref());
    }

    fn load_spritesheet(&mut self, pet: &PetEntry) -> Result<(), String> {
        let bytes = pet.spritesheet_bytes().map_err(|error| error.to_string())?;
        self.window.set_spritesheet(&bytes)
    }

    fn select_pet(&mut self, index: usize) {
        let Some(pet) = self.pets.get(index).cloned() else {
            return;
        };
        self.show_pet(pet);
        if let Err(error) = std::fs::write(paths::selected_pet_file(), &self.current.id) {
            eprintln!("[app] cannot remember the selected pet: {error}");
        }
    }

    fn start_timer(&mut self, minutes: u64) {
        eprintln!("[timer] started for {minutes} min");
        self.timer = Some(Timer::start(minutes, SystemTime::now()));
        self.update_timer();
    }

    fn cancel_timer(&mut self) {
        eprintln!("[timer] cancelled");
        self.stop_timer();
    }

    fn stop_timer(&mut self) {
        self.timer = None;
        self.countdown.clear();
        self.window.hide_countdown();
    }

    /// Refreshes the countdown, and celebrates when it reaches zero.
    fn update_timer(&mut self) {
        let Some(timer) = self.timer else {
            return;
        };
        let Some(left) = timer.remaining(SystemTime::now()) else {
            eprintln!("[timer] finished");
            self.stop_timer();
            self.motion.react(Reaction::Success);
            self.window.show_bubble(TIMER_DONE_MESSAGE);
            self.bubble_until = Some(Instant::now() + TIMER_DONE_BUBBLE);
            if let Some(sound) = NSSound::soundNamed(ns_string!("Glass")) {
                sound.play();
            }
            return;
        };
        let text = timer::format_remaining(left);
        if text != self.countdown {
            self.window.show_countdown(&text);
            self.countdown = text;
        }
    }

    fn toggle_pause(&mut self) {
        self.motion.paused = !self.motion.paused;
    }

    fn open_pets_folder(&self) {
        let dir = paths::pets_dir();
        if let Err(error) = std::fs::create_dir_all(&dir) {
            eprintln!("[app] cannot create the pets folder: {error}");
            return;
        }
        if let Err(error) = std::process::Command::new("open").arg(&dir).status() {
            eprintln!("[app] cannot open the pets folder: {error}");
        }
    }

    fn fill_menu(&mut self, menu: &NSMenu) {
        self.pets = pet::list_pets(&paths::pets_dir());
        let selected = self.pets.iter().position(|pet| pet.id == self.current.id);
        let names: Vec<String> = self.pets.iter().map(|pet| pet.name.clone()).collect();
        self.status_menu.fill(
            menu,
            &names,
            selected,
            self.motion.paused,
            self.timer.is_some(),
        );
    }
}

/// Runs `action` on the app from an AppKit callback (menu or mouse event).
///
/// Callbacks never nest inside `App::tick`, so the borrow normally succeeds;
/// if it does not, the event is dropped rather than panicking.
fn with_app(app: &Weak<RefCell<App>>, action: impl FnOnce(&mut App)) {
    let Some(app) = app.upgrade() else {
        return;
    };
    match app.try_borrow_mut() {
        Ok(mut app) => action(&mut app),
        Err(_) => eprintln!("[app] busy, ignoring an event"),
    };
}

struct PetRect {
    x: f64,
    y: f64,
}

impl PetRect {
    fn contains(&self, point: NSPoint) -> bool {
        let inside_x = point.x >= self.x && point.x <= self.x + PET_WIDTH;
        let inside_y = point.y >= self.y && point.y <= self.y + PET_HEIGHT;
        inside_x && inside_y
    }
}

/// The usable area (without menu bar and a pinned Dock) of the display
/// under `point`.
fn screen_containing(mtm: MainThreadMarker, point: NSPoint) -> NSRect {
    screen_at(mtm, point)
        .map(|screen| screen.visibleFrame())
        .unwrap_or_default()
}

/// The full frame of the display under `point`.
fn display_frame(mtm: MainThreadMarker, point: NSPoint) -> NSRect {
    screen_at(mtm, point)
        .map(|screen| screen.frame())
        .unwrap_or_default()
}

/// The display under `point`, or the main display when it is off-screen.
fn screen_at(mtm: MainThreadMarker, point: NSPoint) -> Option<Retained<NSScreen>> {
    let screens = NSScreen::screens(mtm);
    let under = screens.iter().find(|screen| {
        let frame = screen.frame();
        let inside_x = point.x >= frame.origin.x && point.x < frame.origin.x + frame.size.width;
        // The cursor can sit exactly on the bottom edge (y == origin.y).
        let inside_y = point.y >= frame.origin.y && point.y <= frame.origin.y + frame.size.height;
        inside_x && inside_y
    });
    under.or_else(|| NSScreen::mainScreen(mtm))
}

/// The user's Dock settings and contents. Missing keys mean the macOS
/// defaults: always shown, at the bottom, 48-point icons, recent apps shown.
fn dock_settings() -> DockSettings {
    let defaults = NSUserDefaults::standardUserDefaults();
    let dock = defaults.persistentDomainForName(ns_string!("com.apple.dock"));
    let value = |key: &str| dock.as_ref()?.objectForKey(&NSString::from_str(key));
    let number = |key: &str| {
        value(key).and_then(|object| object.downcast_ref::<NSNumber>().map(NSNumber::doubleValue))
    };
    let orientation = value("orientation")
        .and_then(|object| object.downcast_ref::<NSString>().map(NSString::to_string));
    let list = |key: &str| {
        value(key)
            .and_then(|object| object.downcast::<NSArray>().ok())
            .map(|array| array.to_vec())
            .unwrap_or_default()
    };

    let pinned = list("persistent-apps");
    let pinned_ids: Vec<String> = pinned
        .iter()
        .filter_map(|tile| tile_bundle_id(tile))
        .collect();
    let show_recents = number("show-recents").is_none_or(|flag| flag != 0.0);
    let recents = if show_recents {
        list("recent-apps").len()
    } else {
        0
    };
    let others = list("persistent-others").len();

    // Finder, pinned apps, open apps that are not pinned, recents, the
    // stacks, and the Trash.
    let tiles = 1 + pinned.len() + open_unpinned_apps(&pinned_ids) + recents + others + 1;
    let dividers = 1 + usize::from(recents > 0);
    DockSettings {
        autohide: number("autohide").is_some_and(|flag| flag != 0.0),
        at_bottom: orientation.is_none_or(|side| side == "bottom"),
        tile_size: number("tilesize").unwrap_or(48.0),
        tiles,
        dividers,
    }
}

/// `tile-data.bundle-identifier` of a Dock tile entry.
fn tile_bundle_id(tile: &AnyObject) -> Option<String> {
    let tile = tile.downcast_ref::<NSDictionary>()?;
    let data = tile.objectForKey(ns_string!("tile-data"))?;
    let data = data.downcast_ref::<NSDictionary>()?;
    let id = data.objectForKey(ns_string!("bundle-identifier"))?;
    Some(id.downcast_ref::<NSString>()?.to_string())
}

/// Apps with a Dock icon that are running but not pinned. Finder is left
/// out because it is always counted.
fn open_unpinned_apps(pinned: &[String]) -> usize {
    NSWorkspace::sharedWorkspace()
        .runningApplications()
        .iter()
        .filter(|app| app.activationPolicy() == NSApplicationActivationPolicy::Regular)
        .filter_map(|app| app.bundleIdentifier().map(|id| id.to_string()))
        .filter(|id| id != "com.apple.finder" && !pinned.contains(id))
        .count()
}

/// The pet starts near the right side of the main display's bottom edge.
fn initial_position(mtm: MainThreadMarker) -> NSPoint {
    let visible = NSScreen::mainScreen(mtm)
        .map(|screen| screen.visibleFrame())
        .unwrap_or_default();
    NSPoint::new(
        visible.origin.x + visible.size.width * 0.75,
        visible.origin.y,
    )
}

fn random_seed() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos() as u64)
        .unwrap_or(0x5eed)
}
