//! Reactions agents can request, badges, and the safety rules for speech text.
//!
//! The reaction names and the message rules match the OpenPets MCP contract so
//! existing agent instructions keep working unchanged.

use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Reaction {
    Idle,
    Thinking,
    Working,
    Editing,
    Running,
    Testing,
    Waiting,
    Waving,
    Success,
    Error,
    Celebrating,
}

/// A mark above the pet's head that stays until the situation changes,
/// for things that need the user: `?` for a question, `!` for an approval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Badge {
    Question,
    Alert,
}

impl Reaction {
    pub const ALL: [Reaction; 11] = [
        Reaction::Idle,
        Reaction::Thinking,
        Reaction::Working,
        Reaction::Editing,
        Reaction::Running,
        Reaction::Testing,
        Reaction::Waiting,
        Reaction::Waving,
        Reaction::Success,
        Reaction::Error,
        Reaction::Celebrating,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Reaction::Idle => "idle",
            Reaction::Thinking => "thinking",
            Reaction::Working => "working",
            Reaction::Editing => "editing",
            Reaction::Running => "running",
            Reaction::Testing => "testing",
            Reaction::Waiting => "waiting",
            Reaction::Waving => "waving",
            Reaction::Success => "success",
            Reaction::Error => "error",
            Reaction::Celebrating => "celebrating",
        }
    }

    pub fn parse(value: &str) -> Option<Reaction> {
        Reaction::ALL
            .into_iter()
            .find(|reaction| reaction.name() == value)
    }
}

pub const MAX_MESSAGE_CHARS: usize = 140;

static CODE_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"```|<script|function\s+\w+|=>|\b(class|import|export|const|let|var)\b")
        .expect("valid code pattern")
});

static LOCATION_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"https?://|www\.|/[\w.-]+/[\w./-]+|[A-Za-z]:\\").expect("valid location pattern")
});

static SECRET_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(api[_-]?key|secret|token|password|passwd|BEGIN [A-Z ]+PRIVATE KEY)")
        .expect("valid secret pattern")
});

/// Returns the trimmed message when it is short, single-line, plain prose.
///
/// Speech bubbles are visible to anyone looking at the screen, so code, URLs,
/// file paths and anything that looks like a credential are rejected.
pub fn validate_message(value: &str) -> Result<String, &'static str> {
    let message = value.trim();
    let char_count = message.chars().count();

    if char_count == 0 {
        return Err("Message is empty.");
    }
    if char_count > MAX_MESSAGE_CHARS {
        return Err("Message is longer than 140 characters.");
    }
    if message.contains(['\0', '\r', '\n']) {
        return Err("Message must be a single line.");
    }
    if CODE_PATTERN.is_match(message) {
        return Err("Message must not contain code.");
    }
    if LOCATION_PATTERN.is_match(message) {
        return Err("Message must not contain URLs or file paths.");
    }
    if SECRET_PATTERN.is_match(message) {
        return Err("Message must not mention secrets.");
    }

    Ok(message.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_short_status_text() {
        assert_eq!(
            validate_message("  Tests are green!  ").unwrap(),
            "Tests are green!"
        );
    }

    #[test]
    fn rejects_text_that_would_leak_onto_the_screen() {
        let unsafe_messages = [
            "const secret = 1",
            "see https://example.com",
            "edited /Users/me/project/main.rs",
            "my api_key is here",
            "line one\nline two",
            &"a".repeat(141),
        ];
        for message in unsafe_messages {
            assert!(validate_message(message).is_err(), "accepted: {message}");
        }
    }
}
