//! `openpets-rs hook`: turns Claude Code hook events into pet reactions.
//!
//! Claude Code runs this command for each configured hook event and pipes
//! the event JSON to stdin. The mapping matches the OpenPets Claude
//! integration, so the pet reacts the same way while you work. A hook must
//! never disturb the agent: every failure is swallowed and the exit code is
//! always 0. Set `OPENPETS_DEBUG=1` to see why an event was dropped.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;
use std::sync::LazyLock;
use std::time::{SystemTime, UNIX_EPOCH};

use regex::Regex;
use serde_json::Value;

use crate::ipc::{self, Request};
use crate::paths;
use crate::reaction::Reaction;

const MAX_PAYLOAD_BYTES: u64 = 64 * 1024;
const REACTION_COOLDOWN_MS: u64 = 10_000;
const SPEECH_COOLDOWN_MS: u64 = 20_000;
const PROMPT_SPEECH_COOLDOWN_MS: u64 = 3_000;

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
    Some(Decision { reaction, speech })
}

fn tool_reaction(tool: &str, payload: &Value) -> Option<Reaction> {
    match tool {
        "Edit" | "Write" | "MultiEdit" => Some(Reaction::Editing),
        "Bash" => {
            let command = payload
                .pointer("/tool_input/command")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let command: String = command.chars().take(300).collect();
            TEST_COMMAND.is_match(&command).then_some(Reaction::Testing)
        }
        _ => None,
    }
}

/// Last-sent times per key, shared by every hook process through a file so
/// rapid events (one per tool call) do not restart the same animation.
#[derive(Debug, Default)]
pub struct Throttle {
    sent_at_ms: BTreeMap<String, u64>,
}

impl Throttle {
    fn load(path: &Path) -> Throttle {
        let sent_at_ms = std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        Throttle { sent_at_ms }
    }

    fn save(&self, path: &Path) -> std::io::Result<()> {
        let temporary = path.with_extension("tmp");
        std::fs::write(&temporary, serde_json::to_vec(&self.sent_at_ms)?)?;
        std::fs::rename(temporary, path)
    }

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

pub fn run() {
    let debug = std::env::var_os("OPENPETS_DEBUG").is_some_and(|value| value == "1");
    if let Err(reason) = handle_stdin()
        && debug
    {
        eprintln!("[hook] ignored: {reason}");
    }
}

fn handle_stdin() -> Result<(), String> {
    let mut raw = String::new();
    std::io::stdin()
        .take(MAX_PAYLOAD_BYTES)
        .read_to_string(&mut raw)
        .map_err(|error| format!("cannot read the payload: {error}"))?;
    let payload: Value = serde_json::from_str(&raw).map_err(|_| "payload is not JSON")?;

    let decision = map_event(&payload).ok_or("event does not change the pet")?;

    let throttle_path = paths::hook_throttle_file();
    let mut throttle = Throttle::load(&throttle_path);
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0);
    let request = plan_request(decision, &mut throttle, now_ms).ok_or("throttled")?;
    throttle
        .save(&throttle_path)
        .map_err(|error| format!("cannot save throttle state: {error}"))?;

    // Hooks never launch the app: no pet running means nothing to show.
    let response =
        ipc::send(&paths::socket_path(), &request).map_err(|_| "OpenPets is not running")?;
    if !response.ok {
        return Err(response.error.unwrap_or_else(|| "request rejected".into()));
    }
    Ok(())
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
    }

    #[test]
    fn repeated_events_within_the_cooldown_are_dropped() {
        let decision = Decision {
            reaction: Reaction::Editing,
            speech: None,
        };
        let mut throttle = Throttle::default();

        assert!(plan_request(decision, &mut throttle, 100_000).is_some());
        assert!(plan_request(decision, &mut throttle, 105_000).is_none());
        assert!(plan_request(decision, &mut throttle, 111_000).is_some());
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
