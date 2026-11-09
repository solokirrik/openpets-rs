//! `openpets-rs install-hooks agy` / `uninstall-hooks agy`: manage the
//! Antigravity hooks that run `openpets-rs hook agy <Event>`.
//!
//! Antigravity's `hooks.json` maps hook names to events, so everything lives
//! under one `openpets` key; other hooks are preserved.

use std::path::PathBuf;

use serde_json::{Map, Value, json};

use crate::claude_settings::{current_executable, shell_quote, update_json_file};
use crate::hook::{AGY_COMMAND_TOOL, AGY_EDIT_TOOLS, AGY_QUESTION_TOOL};

const HOOK_NAME: &str = "openpets";
/// Antigravity waits for each hook; the pet must never hold the agent up.
const TIMEOUT_SECONDS: u64 = 3;

/// `~/.gemini/config/hooks.json`, the global hooks file.
pub fn hooks_path() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join(".gemini/config/hooks.json")
}

/// Sets the `openpets` hook, replacing any previous one (for example one
/// pointing at an older build).
pub fn add_hooks(hooks_file: Value, executable: &str) -> Result<Value, String> {
    let mut hooks_file = remove_hooks(hooks_file)?;
    let root = hooks_file
        .as_object_mut()
        .ok_or("hooks.json must be a JSON object")?;
    let handler = |event: &str| {
        let command = format!("{} hook agy {event}", shell_quote(executable));
        json!({ "type": "command", "command": command, "timeout": TIMEOUT_SECONDS })
    };
    let mut post_tools: Vec<&str> = AGY_EDIT_TOOLS.to_vec();
    post_tools.extend([AGY_COMMAND_TOOL, AGY_QUESTION_TOOL]);

    let mut events = Map::new();
    events.insert("PreInvocation".into(), json!([handler("PreInvocation")]));
    // Only the question tool: a PreToolUse reply carries a permission
    // decision, so the pet stays out of every other tool's approval.
    events.insert(
        "PreToolUse".into(),
        json!([{ "matcher": format!("^{AGY_QUESTION_TOOL}$"), "hooks": [handler("PreToolUse")] }]),
    );
    events.insert(
        "PostToolUse".into(),
        json!([{ "matcher": format!("^({})$", post_tools.join("|")), "hooks": [handler("PostToolUse")] }]),
    );
    events.insert("Stop".into(), json!([handler("Stop")]));
    root.insert(HOOK_NAME.into(), Value::Object(events));
    Ok(hooks_file)
}

/// Removes the `openpets` hook.
pub fn remove_hooks(mut hooks_file: Value) -> Result<Value, String> {
    hooks_file
        .as_object_mut()
        .ok_or("hooks.json must be a JSON object")?
        .remove(HOOK_NAME);
    Ok(hooks_file)
}

pub fn install() -> Result<(), String> {
    let executable = current_executable()?;
    let path = hooks_path();
    update_json_file(&path, |hooks_file| add_hooks(hooks_file, &executable))?;
    println!("Installed Antigravity hooks in {}", path.display());
    println!("Restart agy sessions to pick them up (check with /hooks).");
    Ok(())
}

pub fn uninstall() -> Result<(), String> {
    let path = hooks_path();
    if !path.exists() {
        println!("No Antigravity hooks file; nothing to remove.");
        return Ok(());
    }
    update_json_file(&path, remove_hooks)?;
    println!("Removed openpets-rs hooks from {}", path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXECUTABLE: &str = "/Users/me/Applications/OpenPets.app/Contents/MacOS/openpets-rs";

    #[test]
    fn installing_twice_keeps_one_openpets_hook_and_other_hooks() {
        let other = json!({ "PostToolUse": [{ "matcher": "run_command", "hooks": [{ "command": "./lint.sh" }] }] });
        let hooks_file = json!({ "linter": other });

        let once = add_hooks(hooks_file, EXECUTABLE).unwrap();
        let twice = add_hooks(once.clone(), EXECUTABLE).unwrap();

        assert_eq!(once, twice);
        assert_eq!(twice["linter"], other);
        assert_eq!(
            twice["openpets"]["Stop"][0]["command"],
            format!("'{EXECUTABLE}' hook agy Stop")
        );
    }

    #[test]
    fn tool_hooks_use_matcher_groups_and_others_flat_handlers() {
        let installed = add_hooks(json!({}), EXECUTABLE).unwrap();
        let events = &installed["openpets"];

        for event in ["PreInvocation", "Stop"] {
            assert!(events[event][0]["command"].is_string(), "{event}");
        }
        for event in ["PreToolUse", "PostToolUse"] {
            assert!(events[event][0]["matcher"].is_string(), "{event}");
            assert!(
                events[event][0]["hooks"][0]["command"].is_string(),
                "{event}"
            );
        }
    }

    #[test]
    fn pre_tool_use_only_matches_the_question_tool() {
        let installed = add_hooks(json!({}), EXECUTABLE).unwrap();
        let matcher = installed["openpets"]["PreToolUse"][0]["matcher"]
            .as_str()
            .unwrap();
        let matcher = regex::Regex::new(matcher).unwrap();

        assert!(matcher.is_match("ask_question"));
        for tool in ["run_command", "edit_file", "write_file", "ask_permission"] {
            assert!(!matcher.is_match(tool), "{tool}");
        }
    }

    #[test]
    fn uninstalling_removes_only_the_openpets_hook() {
        let hooks_file = json!({ "linter": { "Stop": [{ "command": "./lint.sh" }] } });
        let installed = add_hooks(hooks_file.clone(), EXECUTABLE).unwrap();

        assert_eq!(remove_hooks(installed).unwrap(), hooks_file);
    }

    #[test]
    fn a_hooks_file_that_is_not_an_object_is_refused() {
        assert!(add_hooks(json!([]), EXECUTABLE).is_err());
        assert!(remove_hooks(json!("x")).is_err());
    }
}
