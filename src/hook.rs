//! `openpets-rs hook`: turns Claude Code hook events into pet reactions.
//!
//! Claude Code runs this command for each configured hook event and pipes
//! the event JSON to stdin. The mapping matches the OpenPets Claude
//! integration, so the pet reacts the same way while you work. A hook must
//! never disturb the agent: every failure is swallowed and the exit code is
//! always 0. Set `OPENPETS_DEBUG=1` to see why an event was dropped.
//!
//! A permission request also puts a badge over the pet (`?` for a question,
//! `!` for an approval), and a finished answer puts up `i`. A badge stays
//! until that session's next event: Claude Code has no "answered" or "read"
//! event, but the session always moves on once the user responds. Badges
//! are tracked per session, so another session carrying on does not clear
//! one still waiting on the user, and `?`/`!` outrank `i`.
//!
//! `openpets-rs hook agy <Event>` does the same for Antigravity (`agy`),
//! whose payloads carry no event name (hence the argument) and which runs
//! hooks synchronously, waiting for a JSON reply on stdout. That reply is
//! written first, on every path, and never changes what the agent does.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::LazyLock;
use std::time::{SystemTime, UNIX_EPOCH};

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ipc::{self, Request};
use crate::paths;
use crate::reaction::{Badge, Reaction};

const MAX_PAYLOAD_BYTES: u64 = 64 * 1024;
const REACTION_COOLDOWN_MS: u64 = 10_000;
const SPEECH_COOLDOWN_MS: u64 = 20_000;
const PROMPT_SPEECH_COOLDOWN_MS: u64 = 3_000;
/// A badge from a session that never moved on (closed while waiting) is
/// dropped after this long.
const BADGE_EXPIRY_MS: u64 = 2 * 60 * 60 * 1000;

/// Canned, pre-vetted lines; hooks never forward agent text to the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Speech {
    Error,
    Permission,
    Question,
}

impl Speech {
    fn key(self) -> &'static str {
        match self {
            Speech::Error => "error",
            Speech::Permission => "permission",
            Speech::Question => "question",
        }
    }

    fn lines(self) -> &'static [&'static str] {
        match self {
            Speech::Error => &[
                "Something failed",
                "Needs another look",
                "Hit a snag",
                "Not quite there",
            ],
            Speech::Permission => &["Approval needed"],
            Speech::Question => &["Question for you", "Quick question", "Need your input"],
        }
    }

    fn cooldown_ms(self) -> u64 {
        match self {
            // Prompts that wait on the user should resurface quickly.
            Speech::Permission | Speech::Question => PROMPT_SPEECH_COOLDOWN_MS,
            Speech::Error => SPEECH_COOLDOWN_MS,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Decision {
    pub reaction: Reaction,
    pub speech: Option<Speech>,
    /// Set while the session waits on the user.
    pub badge: Option<Badge>,
}

static TEST_COMMAND: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(test|vitest|jest|pytest|npm\s+test|pnpm\s+test|yarn\s+test|cargo\s+test|go\s+test)\b")
        .expect("valid test command pattern")
});

/// Maps one Claude Code hook payload to what the pet should do, if anything.
pub fn map_event(payload: &Value) -> Option<Decision> {
    let event = payload.get("hook_event_name")?.as_str()?;
    let tool = payload
        .get("tool_name")
        .and_then(Value::as_str)
        .unwrap_or_default();

    let (reaction, speech) = match event {
        "UserPromptSubmit" => (Reaction::Thinking, None),
        "Stop" => (Reaction::Success, None),
        "StopFailure" => (Reaction::Error, Some(Speech::Error)),
        // AskUserQuestion also raises PermissionRequest, but it asks a question.
        "PermissionRequest" if tool == "AskUserQuestion" => {
            (Reaction::Waiting, Some(Speech::Question))
        }
        "PermissionRequest" => (Reaction::Waiting, Some(Speech::Permission)),
        "PreToolUse" => (tool_reaction(tool, payload)?, None),
        _ => return None,
    };
    let badge = match (event, tool) {
        ("PermissionRequest", "AskUserQuestion") => Some(Badge::Question),
        ("PermissionRequest", _) => Some(Badge::Alert),
        ("Stop" | "StopFailure", _) => Some(Badge::Info),
        _ => None,
    };
    Some(Decision {
        reaction,
        speech,
        badge,
    })
}

fn tool_reaction(tool: &str, payload: &Value) -> Option<Reaction> {
    match tool {
        "Edit" | "Write" | "MultiEdit" => Some(Reaction::Editing),
        "Bash" => test_reaction(payload.pointer("/tool_input/command")),
        _ => None,
    }
}

fn test_reaction(command: Option<&Value>) -> Option<Reaction> {
    let command = command.and_then(Value::as_str).unwrap_or_default();
    let command: String = command.chars().take(300).collect();
    TEST_COMMAND.is_match(&command).then_some(Reaction::Testing)
}

/// Antigravity tools that change files. The CLI uses the first three; the
/// documented IDE names follow.
pub const AGY_EDIT_TOOLS: [&str; 6] = [
    "edit_file",
    "write_file",
    "multi_edit",
    "write_to_file",
    "replace_file_content",
    "multi_replace_file_content",
];
pub const AGY_COMMAND_TOOL: &str = "run_command";
pub const AGY_QUESTION_TOOL: &str = "ask_question";

/// Maps one Antigravity hook payload to what the pet should do. `event` is
/// the name `install-hooks agy` put on the command line.
pub fn map_agy_event(event: &str, payload: &Value) -> Option<Decision> {
    let tool = payload
        .pointer("/toolCall/name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let quiet = |reaction| Decision {
        reaction,
        speech: None,
        badge: None,
    };

    match event {
        // PreInvocation fires before every model step; only the first
        // (0-based) starts a turn, later ones would undo editing/testing.
        "PreInvocation" => {
            let first = payload.get("invocationNum").and_then(Value::as_u64) == Some(0);
            first.then(|| quiet(Reaction::Thinking))
        }
        "PreToolUse" if tool == AGY_QUESTION_TOOL => Some(Decision {
            reaction: Reaction::Waiting,
            speech: Some(Speech::Question),
            badge: Some(Badge::Question),
        }),
        "PostToolUse" if AGY_EDIT_TOOLS.contains(&tool) => Some(quiet(Reaction::Editing)),
        "PostToolUse" if tool == AGY_COMMAND_TOOL => {
            test_reaction(payload.pointer("/toolCall/args/CommandLine")).map(quiet)
        }
        "Stop" => {
            let reason = payload
                .get("terminationReason")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_ascii_uppercase();
            let failed = payload
                .get("error")
                .and_then(Value::as_str)
                .is_some_and(|error| !error.is_empty());
            if failed || reason.contains("ERROR") {
                Some(Decision {
                    reaction: Reaction::Error,
                    speech: Some(Speech::Error),
                    badge: Some(Badge::Info),
                })
            } else if reason.contains("CANCEL") {
                None
            } else {
                Some(Decision {
                    badge: Some(Badge::Info),
                    ..quiet(Reaction::Success)
                })
            }
        }
        _ => None,
    }
}

/// The JSON Antigravity requires on stdout. `"allow"` keeps agy's own
/// permission rules in charge (unlike `"auto_approve"`), and any `Stop`
/// decision other than `"continue"` lets the agent stop.
pub fn agy_reply(event: &str) -> &'static str {
    match event {
        "PreToolUse" => r#"{"decision":"allow"}"#,
        "Stop" => r#"{"decision":"stop"}"#,
        _ => "{}",
    }
}

/// What hook runs share through a file: each run is a separate process.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct HookState {
    throttle: Throttle,
    badges: BadgeBoard,
}

impl HookState {
    /// A missing or unreadable file starts fresh; at worst one reaction
    /// repeats early.
    fn load(path: &Path) -> HookState {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    fn save(&self, path: &Path) -> std::io::Result<()> {
        let temporary = path.with_extension("tmp");
        std::fs::write(&temporary, serde_json::to_vec(self)?)?;
        std::fs::rename(temporary, path)
    }
}

/// Last-sent times per key, so rapid events (one per tool call) do not
/// restart the same animation.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Throttle {
    sent_at_ms: BTreeMap<String, u64>,
}

impl Throttle {
    /// Records `key` as sent and returns true, unless it was sent within `cooldown_ms`.
    pub fn allow(&mut self, key: &str, cooldown_ms: u64, now_ms: u64) -> bool {
        let previous = self.sent_at_ms.get(key).copied().unwrap_or(0);
        if now_ms.saturating_sub(previous) < cooldown_ms {
            return false;
        }
        self.sent_at_ms.insert(key.to_owned(), now_ms);
        true
    }
}

/// The badges sessions are waiting on, and the one the pet was last told to show.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BadgeBoard {
    pending: BTreeMap<String, PendingBadge>,
    shown: Option<Badge>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct PendingBadge {
    badge: Badge,
    since_ms: u64,
}

impl BadgeBoard {
    /// Records `session`'s latest event: `badge` while it waits on the user,
    /// `None` once it moves on. Returns the badge the pet should now show
    /// (`Some(None)` clears it), or `None` when nothing needs sending.
    pub fn update(
        &mut self,
        session: &str,
        badge: Option<Badge>,
        now_ms: u64,
    ) -> Option<Option<Badge>> {
        self.pending
            .retain(|_, pending| now_ms.saturating_sub(pending.since_ms) < BADGE_EXPIRY_MS);
        match badge {
            Some(badge) => {
                let pending = PendingBadge {
                    badge,
                    since_ms: now_ms,
                };
                self.pending.insert(session.to_owned(), pending);
            }
            None => {
                self.pending.remove(session);
            }
        }

        // Anything blocked on the user beats a finished answer; then the newest.
        let wanted = self
            .pending
            .values()
            .max_by_key(|pending| (pending.badge.needs_user(), pending.since_ms))
            .map(|pending| pending.badge);
        // A new request is always sent: the user may have clicked the last
        // badge away, so "already shown" cannot be trusted for it.
        (badge.is_some() || wanted != self.shown).then_some(wanted)
    }

    /// Notes what the pet shows after a successful send.
    pub fn mark_shown(&mut self, badge: Option<Badge>) {
        self.shown = badge;
    }
}

/// Picks what to send for `decision`, or `None` when everything is throttled.
pub fn plan_request(decision: Decision, throttle: &mut Throttle, now_ms: u64) -> Option<Request> {
    let speak = decision
        .speech
        .filter(|speech| throttle.allow(speech.key(), speech.cooldown_ms(), now_ms));
    let reaction_key = format!("reaction:{}", decision.reaction.name());
    let react = throttle.allow(&reaction_key, REACTION_COOLDOWN_MS, now_ms);

    match speak {
        Some(speech) => {
            let lines = speech.lines();
            let message = lines[(now_ms as usize) % lines.len()].to_owned();
            Some(Request::Say {
                message,
                reaction: Some(decision.reaction),
            })
        }
        None if react => Some(Request::React {
            reaction: decision.reaction,
        }),
        None => None,
    }
}

/// Handles one Claude Code hook event.
pub fn run() {
    report(handle_stdin(None));
}

/// Handles one Antigravity hook event, replying on stdout before anything
/// else can fail or take time.
pub fn run_agy(event: &str) {
    let mut stdout = std::io::stdout().lock();
    let _ = writeln!(stdout, "{}", agy_reply(event));
    let _ = stdout.flush();
    drop(stdout);
    report(handle_stdin(Some(event)));
}

fn report(result: Result<(), String>) {
    let debug = std::env::var_os("OPENPETS_DEBUG").is_some_and(|value| value == "1");
    if let Err(reason) = result
        && debug
    {
        eprintln!("[hook] ignored: {reason}");
    }
}

fn handle_stdin(agy_event: Option<&str>) -> Result<(), String> {
    let mut raw = String::new();
    std::io::stdin()
        .take(MAX_PAYLOAD_BYTES)
        .read_to_string(&mut raw)
        .map_err(|error| format!("cannot read the payload: {error}"))?;
    let payload: Value = serde_json::from_str(&raw).map_err(|_| "payload is not JSON")?;

    let decision = match agy_event {
        Some(event) => map_agy_event(event, &payload),
        None => map_event(&payload),
    };
    let session = payload
        .get("session_id")
        .or_else(|| payload.get("conversationId"))
        .and_then(Value::as_str)
        .unwrap_or_default();

    let state_path = paths::hook_state_file();
    let mut state = HookState::load(&state_path);
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0);
    // Every event counts for badges, even ones that leave the pet alone:
    // any activity means the session is no longer waiting on the user.
    let badge = state.badges.update(
        session,
        decision.and_then(|decision| decision.badge),
        now_ms,
    );
    let request = decision.and_then(|decision| plan_request(decision, &mut state.throttle, now_ms));

    let mut result = match (&request, badge) {
        (None, None) if decision.is_none() => Err("event does not change the pet".to_owned()),
        (None, None) => Err("throttled".to_owned()),
        _ => Ok(()),
    };
    if let Some(request) = &request {
        result = result.and(send(request));
    }
    if let Some(badge) = badge {
        let sent = send(&Request::SetBadge { badge });
        if sent.is_ok() {
            state.badges.mark_shown(badge);
        }
        result = result.and(sent);
    }

    state
        .save(&state_path)
        .map_err(|error| format!("cannot save hook state: {error}"))?;
    result
}

/// Hooks never launch the app: no pet running means nothing to show.
fn send(request: &Request) -> Result<(), String> {
    let response = ipc::send(&paths::socket_path(), request)
        .map_err(|_| "OpenPets is not running".to_owned())?;
    if response.ok {
        Ok(())
    } else {
        Err(response.error.unwrap_or_else(|| "request rejected".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn reaction_for(payload: Value) -> Option<Reaction> {
        map_event(&payload).map(|decision| decision.reaction)
    }

    #[test]
    fn maps_claude_code_events_to_reactions() {
        assert_eq!(
            reaction_for(json!({ "hook_event_name": "UserPromptSubmit" })),
            Some(Reaction::Thinking)
        );
        assert_eq!(
            reaction_for(json!({ "hook_event_name": "Stop" })),
            Some(Reaction::Success)
        );
        assert_eq!(
            reaction_for(json!({ "hook_event_name": "StopFailure" })),
            Some(Reaction::Error)
        );
        assert_eq!(
            reaction_for(json!({ "hook_event_name": "PreToolUse", "tool_name": "Edit" })),
            Some(Reaction::Editing)
        );
        assert_eq!(
            reaction_for(json!({
                "hook_event_name": "PreToolUse",
                "tool_name": "Bash",
                "tool_input": { "command": "cargo test --all" }
            })),
            Some(Reaction::Testing)
        );
    }

    #[test]
    fn routine_tool_use_leaves_the_pet_alone() {
        assert_eq!(
            reaction_for(json!({ "hook_event_name": "PreToolUse", "tool_name": "Read" })),
            None
        );
        assert_eq!(
            reaction_for(json!({
                "hook_event_name": "PreToolUse",
                "tool_name": "Bash",
                "tool_input": { "command": "ls -la" }
            })),
            None
        );
        assert_eq!(
            reaction_for(json!({ "hook_event_name": "Notification" })),
            None
        );
    }

    #[test]
    fn a_question_is_announced_as_a_question_not_an_approval() {
        let decision = map_event(&json!({
            "hook_event_name": "PermissionRequest",
            "tool_name": "AskUserQuestion"
        }))
        .unwrap();

        assert_eq!(decision.speech, Some(Speech::Question));
        assert_eq!(decision.badge, Some(Badge::Question));
    }

    #[test]
    fn approval_badge_stays_until_that_session_moves_on() {
        let approval = map_event(&json!({
            "hook_event_name": "PermissionRequest",
            "tool_name": "Bash"
        }))
        .unwrap();
        let mut board = BadgeBoard::default();

        let shown = board.update("a", approval.badge, 1_000).unwrap();
        assert_eq!(shown, Some(Badge::Alert));
        board.mark_shown(shown);

        // Another session working does not clear a badge still waiting on the user.
        assert_eq!(board.update("b", None, 2_000), None);
        // The waiting session's next event (any event) clears it.
        assert_eq!(board.update("a", None, 3_000), Some(None));
    }

    #[test]
    fn a_finished_answer_shows_info_until_the_next_prompt() {
        for event in ["Stop", "StopFailure"] {
            let decision = map_event(&json!({ "hook_event_name": event })).unwrap();
            assert_eq!(decision.badge, Some(Badge::Info), "{event}");
        }
        let finished = map_agy_event(
            "Stop",
            &json!({ "terminationReason": "NO_TOOL_CALL", "error": "" }),
        )
        .unwrap();
        assert_eq!(finished.badge, Some(Badge::Info));

        let mut board = BadgeBoard::default();
        board.update("a", Some(Badge::Info), 1_000);
        board.mark_shown(Some(Badge::Info));
        let prompt = map_event(&json!({ "hook_event_name": "UserPromptSubmit" })).unwrap();
        assert_eq!(board.update("a", prompt.badge, 2_000), Some(None));
    }

    #[test]
    fn a_finished_answer_does_not_hide_a_session_waiting_on_the_user() {
        let mut board = BadgeBoard::default();
        board.update("waiting", Some(Badge::Question), 1_000);
        board.mark_shown(Some(Badge::Question));

        // A newer `i` from another session is recorded but not shown.
        assert_eq!(
            board.update("done", Some(Badge::Info), 2_000),
            Some(Some(Badge::Question))
        );
        board.mark_shown(Some(Badge::Question));
        // Once the question is answered, the `i` surfaces.
        assert_eq!(
            board.update("waiting", None, 3_000),
            Some(Some(Badge::Info))
        );
    }

    #[test]
    fn a_new_request_is_shown_even_after_the_badge_was_clicked_away() {
        let mut board = BadgeBoard::default();
        board.update("a", Some(Badge::Alert), 1_000);
        board.mark_shown(Some(Badge::Alert));

        // The user clicked the pet, then the same session asks again.
        assert_eq!(
            board.update("a", Some(Badge::Alert), 2_000),
            Some(Some(Badge::Alert))
        );
    }

    #[test]
    fn a_badge_left_by_a_closed_session_expires() {
        let mut board = BadgeBoard::default();
        board.update("closed", Some(Badge::Question), 0);
        board.mark_shown(Some(Badge::Question));

        assert_eq!(board.update("other", None, BADGE_EXPIRY_MS), Some(None));
    }

    #[test]
    fn repeated_events_within_the_cooldown_are_dropped() {
        let decision = Decision {
            reaction: Reaction::Editing,
            speech: None,
            badge: None,
        };
        let mut throttle = Throttle::default();

        assert!(plan_request(decision, &mut throttle, 100_000).is_some());
        assert!(plan_request(decision, &mut throttle, 105_000).is_none());
        assert!(plan_request(decision, &mut throttle, 111_000).is_some());
    }

    fn agy_reaction(event: &str, payload: Value) -> Option<Reaction> {
        map_agy_event(event, &payload).map(|decision| decision.reaction)
    }

    // Payload shapes below were captured from agy 1.5.3.
    #[test]
    fn maps_antigravity_events_to_reactions() {
        assert_eq!(
            agy_reaction(
                "PreInvocation",
                json!({ "invocationNum": 0, "initialNumSteps": 1 })
            ),
            Some(Reaction::Thinking)
        );
        assert_eq!(
            agy_reaction(
                "PostToolUse",
                json!({ "error": "", "toolCall": { "name": "edit_file", "args": { "Target": "hi" } } })
            ),
            Some(Reaction::Editing)
        );
        assert_eq!(
            agy_reaction(
                "PostToolUse",
                json!({ "toolCall": { "name": "run_command", "args": { "CommandLine": "cargo test" } } })
            ),
            Some(Reaction::Testing)
        );
        assert_eq!(
            agy_reaction(
                "Stop",
                json!({ "error": "", "executionNum": 0, "fullyIdle": true, "terminationReason": "NO_TOOL_CALL" })
            ),
            Some(Reaction::Success)
        );
        assert_eq!(
            agy_reaction("Stop", json!({ "terminationReason": "error" })),
            Some(Reaction::Error)
        );
        assert_eq!(
            agy_reaction("Stop", json!({ "terminationReason": "USER_CANCELED" })),
            None
        );
    }

    #[test]
    fn later_antigravity_steps_do_not_restart_thinking() {
        assert_eq!(
            agy_reaction(
                "PreInvocation",
                json!({ "invocationNum": 1, "initialNumSteps": 3 })
            ),
            None
        );
    }

    #[test]
    fn routine_antigravity_tools_leave_the_pet_alone() {
        assert_eq!(
            agy_reaction(
                "PostToolUse",
                json!({ "toolCall": { "name": "run_command", "args": { "CommandLine": "ls" } } })
            ),
            None
        );
        assert_eq!(
            agy_reaction(
                "PostToolUse",
                json!({ "toolCall": { "name": "read_file" } })
            ),
            None
        );
        // A tool about to run is never an edit: only PostToolUse counts.
        assert_eq!(
            agy_reaction("PreToolUse", json!({ "toolCall": { "name": "edit_file" } })),
            None
        );
    }

    #[test]
    fn an_antigravity_question_shows_a_badge_that_its_answer_clears() {
        let asked = map_agy_event(
            "PreToolUse",
            &json!({ "toolCall": { "name": "ask_question" } }),
        )
        .unwrap();
        assert_eq!(asked.speech, Some(Speech::Question));
        assert_eq!(asked.badge, Some(Badge::Question));

        let answered = map_agy_event(
            "PostToolUse",
            &json!({ "toolCall": { "name": "ask_question" } }),
        );
        let mut board = BadgeBoard::default();
        board.update("c", asked.badge, 1_000);
        board.mark_shown(asked.badge);
        assert_eq!(
            board.update("c", answered.and_then(|decision| decision.badge), 2_000),
            Some(None)
        );
    }

    #[test]
    fn antigravity_always_gets_a_reply_that_leaves_the_agent_in_charge() {
        for event in [
            "PreInvocation",
            "PostToolUse",
            "PreToolUse",
            "Stop",
            "Unknown",
        ] {
            let reply: Value = serde_json::from_str(agy_reply(event)).unwrap();
            assert!(reply.is_object(), "{event}");
        }
        assert_eq!(agy_reply("PreToolUse"), r#"{"decision":"allow"}"#);
        let stop: Value = serde_json::from_str(agy_reply("Stop")).unwrap();
        assert_ne!(stop["decision"], "continue");
    }

    #[test]
    fn speech_carries_the_reaction_with_it() {
        let decision = map_event(&json!({ "hook_event_name": "StopFailure" })).unwrap();
        let request = plan_request(decision, &mut Throttle::default(), 100_000).unwrap();

        let Request::Say { message, reaction } = request else {
            panic!("expected speech, got {request:?}");
        };
        assert!(crate::reaction::validate_message(&message).is_ok());
        assert_eq!(reaction, Some(Reaction::Error));
    }
}
