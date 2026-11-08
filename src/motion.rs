//! Pure pet behaviour: roaming, reactions, dragging and falling.
//!
//! Coordinates are AppKit screen points (origin bottom-left, y grows upward)
//! and describe the bottom-left corner of the pet. Nothing here touches
//! AppKit, so the behaviour is unit-tested directly.

use crate::reaction::Reaction;

/// One row of the spritesheet, as the pet format defines it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anim {
    Idle,
    RunRight,
    RunLeft,
    Waving,
    Jumping,
    Failed,
    Waiting,
    Running,
    Review,
}

impl Anim {
    /// How long a reaction using this animation holds before roaming resumes.
    fn reaction_hold_secs(self) -> f64 {
        match self {
            // Two plays of the one-shot rows.
            Anim::Waving => 1.4,
            Anim::Jumping => 1.7,
            Anim::Failed => 2.5,
            // Thinking lasts until the agent's next reaction replaces it
            // (an edit, a question, the reply finishing). The cap only
            // matters if that never comes, e.g. the session was closed.
            Anim::Review => THINKING_MAX_SECS,
            // Other looping rows describe ongoing work; hold them a while.
            _ => 8.0,
        }
    }
}

pub fn reaction_anim(reaction: Reaction) -> Anim {
    match reaction {
        Reaction::Idle => Anim::Idle,
        Reaction::Thinking => Anim::Review,
        Reaction::Working | Reaction::Editing | Reaction::Running => Anim::Running,
        Reaction::Testing | Reaction::Waiting => Anim::Waiting,
        Reaction::Waving => Anim::Waving,
        Reaction::Success | Reaction::Celebrating => Anim::Jumping,
        Reaction::Error => Anim::Failed,
    }
}

/// The area the pet may occupy on its current display.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub min_x: f64,
    /// Largest x for the pet's left edge, so the whole pet stays visible.
    pub max_x: f64,
    pub floor_y: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Activity {
    Idle { remaining: f64 },
    Walk { direction: f64, remaining: f64 },
    React { anim: Anim, remaining: f64 },
    Drag { anim: Anim },
    Fall { velocity_y: f64 },
}

const WALK_SPEED: f64 = 70.0;
const GRAVITY: f64 = 2400.0;
/// How fast the pet steps up when the floor rises (the Dock sliding in);
/// about as fast as the Dock itself.
const LIFT_SPEED: f64 = 400.0;
const THINKING_MAX_SECS: f64 = 10.0 * 60.0;

pub struct Motion {
    pub x: f64,
    pub y: f64,
    pub paused: bool,
    activity: Activity,
    rng: u64,
}

impl Motion {
    pub fn new(x: f64, y: f64, seed: u64) -> Motion {
        Motion {
            x,
            y,
            paused: false,
            activity: Activity::Idle { remaining: 1.5 },
            // xorshift must never be seeded with zero.
            rng: seed | 1,
        }
    }

    pub fn is_dragging(&self) -> bool {
        matches!(self.activity, Activity::Drag { .. })
    }

    pub fn react(&mut self, reaction: Reaction) {
        if self.is_dragging() {
            return;
        }
        let anim = reaction_anim(reaction);
        let remaining = anim.reaction_hold_secs();
        self.activity = Activity::React { anim, remaining };
    }

    pub fn start_drag(&mut self) {
        self.activity = Activity::Drag { anim: Anim::Idle };
    }

    pub fn drag_to(&mut self, x: f64, y: f64) {
        let Activity::Drag { anim } = self.activity else {
            return;
        };
        let dx = x - self.x;
        let anim = if dx > 0.5 {
            Anim::RunRight
        } else if dx < -0.5 {
            Anim::RunLeft
        } else {
            anim
        };
        self.x = x;
        self.y = y;
        self.activity = Activity::Drag { anim };
    }

    pub fn end_drag(&mut self) {
        if self.is_dragging() {
            self.activity = Activity::Fall { velocity_y: 0.0 };
        }
    }

    /// Advances the pet by `dt` seconds and returns the animation to show.
    pub fn tick(&mut self, dt: f64, bounds: Bounds) -> Anim {
        self.follow_floor(dt, bounds);
        let anim = match self.activity {
            Activity::Drag { anim } => return anim,
            Activity::Fall { velocity_y } => self.fall(dt, velocity_y, bounds),
            Activity::React { anim, remaining } => {
                let remaining = remaining - dt;
                if remaining <= 0.0 {
                    self.activity = self.next_roam();
                } else {
                    self.activity = Activity::React { anim, remaining };
                }
                anim
            }
            Activity::Idle { remaining } => {
                let remaining = remaining - dt;
                if remaining <= 0.0 {
                    self.activity = self.next_roam();
                } else {
                    self.activity = Activity::Idle { remaining };
                }
                Anim::Idle
            }
            Activity::Walk {
                direction,
                remaining,
            } => self.walk(dt, direction, remaining, bounds),
        };

        self.x = self.x.clamp(bounds.min_x, bounds.max_x.max(bounds.min_x));
        anim
    }

    /// Keeps a standing pet on the floor when the floor moves: it steps up
    /// smoothly when the floor rises and falls when the floor drops away.
    fn follow_floor(&mut self, dt: f64, bounds: Bounds) {
        if matches!(self.activity, Activity::Drag { .. } | Activity::Fall { .. }) {
            return;
        }
        if self.y > bounds.floor_y {
            self.activity = Activity::Fall { velocity_y: 0.0 };
        } else if self.y < bounds.floor_y {
            self.y = (self.y + LIFT_SPEED * dt).min(bounds.floor_y);
        }
    }

    fn walk(&mut self, dt: f64, direction: f64, remaining: f64, bounds: Bounds) -> Anim {
        if self.paused {
            self.activity = Activity::Idle { remaining: 1.0 };
            return Anim::Idle;
        }

        let mut direction = direction;
        self.x += direction * WALK_SPEED * dt;
        if self.x <= bounds.min_x {
            self.x = bounds.min_x;
            direction = 1.0;
        } else if self.x >= bounds.max_x {
            self.x = bounds.max_x;
            direction = -1.0;
        }

        let remaining = remaining - dt;
        if remaining <= 0.0 {
            self.activity = Activity::Idle {
                remaining: self.random_between(2.0, 6.0),
            };
        } else {
            self.activity = Activity::Walk {
                direction,
                remaining,
            };
        }

        if direction > 0.0 {
            Anim::RunRight
        } else {
            Anim::RunLeft
        }
    }

    fn fall(&mut self, dt: f64, velocity_y: f64, bounds: Bounds) -> Anim {
        let velocity_y = velocity_y - GRAVITY * dt;
        self.y += velocity_y * dt;
        if self.y <= bounds.floor_y {
            self.y = bounds.floor_y;
            self.activity = Activity::Idle { remaining: 1.0 };
            return Anim::Idle;
        }
        self.activity = Activity::Fall { velocity_y };
        Anim::Jumping
    }

    fn next_roam(&mut self) -> Activity {
        if self.paused || self.random_between(0.0, 1.0) < 0.35 {
            return Activity::Idle {
                remaining: self.random_between(2.0, 6.0),
            };
        }
        let direction = if self.random_between(0.0, 1.0) < 0.5 {
            -1.0
        } else {
            1.0
        };
        Activity::Walk {
            direction,
            remaining: self.random_between(2.0, 5.0),
        }
    }

    fn random_between(&mut self, low: f64, high: f64) -> f64 {
        // xorshift64: plenty for picking idle and walk durations.
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        let unit = (self.rng >> 11) as f64 / (1u64 << 53) as f64;
        low + unit * (high - low)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOUNDS: Bounds = Bounds {
        min_x: 0.0,
        max_x: 300.0,
        floor_y: 40.0,
    };

    fn run(motion: &mut Motion, seconds: f64) -> Vec<Anim> {
        let steps = (seconds * 60.0) as usize;
        (0..steps)
            .map(|_| motion.tick(1.0 / 60.0, BOUNDS))
            .collect()
    }

    #[test]
    fn roaming_pet_walks_and_stays_on_screen() {
        let mut motion = Motion::new(150.0, 40.0, 7);
        let anims = run(&mut motion, 120.0);

        assert!(
            anims
                .iter()
                .any(|anim| matches!(anim, Anim::RunLeft | Anim::RunRight))
        );
        assert!((BOUNDS.min_x..=BOUNDS.max_x).contains(&motion.x));
        assert_eq!(motion.y, BOUNDS.floor_y);
    }

    #[test]
    fn paused_pet_does_not_move() {
        let mut motion = Motion::new(150.0, 40.0, 7);
        motion.paused = true;
        run(&mut motion, 60.0);

        assert_eq!(motion.x, 150.0);
    }

    #[test]
    fn reaction_holds_the_pet_in_place_then_roaming_resumes() {
        let mut motion = Motion::new(150.0, 40.0, 7);
        motion.react(Reaction::Error);

        let during = run(&mut motion, 2.0);
        assert!(during.iter().all(|anim| *anim == Anim::Failed));
        assert_eq!(motion.x, 150.0);

        let after = run(&mut motion, 1.0);
        assert_ne!(after.last(), Some(&Anim::Failed));
    }

    #[test]
    fn thinking_holds_until_the_next_reaction() {
        let mut motion = Motion::new(150.0, 40.0, 7);
        motion.react(Reaction::Thinking);

        let minute = run(&mut motion, 60.0);
        assert!(minute.iter().all(|anim| *anim == Anim::Review));

        motion.react(Reaction::Editing);
        assert_ne!(motion.tick(1.0 / 60.0, BOUNDS), Anim::Review);
    }

    #[test]
    fn released_pet_falls_back_to_the_floor() {
        let mut motion = Motion::new(150.0, 40.0, 7);
        motion.start_drag();
        motion.drag_to(80.0, 400.0);
        motion.end_drag();
        run(&mut motion, 2.0);

        assert_eq!(motion.y, BOUNDS.floor_y);
    }

    #[test]
    fn pet_steps_up_when_the_floor_rises_and_falls_when_it_drops() {
        let mut motion = Motion::new(150.0, 40.0, 7);
        motion.paused = true;
        let raised = Bounds {
            floor_y: 124.0,
            ..BOUNDS
        };

        motion.tick(1.0 / 60.0, raised);
        assert!(
            motion.y > 40.0 && motion.y < 124.0,
            "no teleport: {}",
            motion.y
        );
        for _ in 0..60 {
            motion.tick(1.0 / 60.0, raised);
        }
        assert_eq!(motion.y, 124.0);

        run(&mut motion, 1.0);
        assert_eq!(motion.y, BOUNDS.floor_y);
    }

    #[test]
    fn reactions_do_not_interrupt_a_drag() {
        let mut motion = Motion::new(150.0, 40.0, 7);
        motion.start_drag();
        motion.react(Reaction::Success);

        assert!(motion.is_dragging());
    }
}
