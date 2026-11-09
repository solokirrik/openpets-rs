//! `openpets-rs install-hooks` / `uninstall-hooks`: manage the Claude Code
//! hook entries that run `openpets-rs hook`.
//!
//! Only entries whose command is this binary's `hook` command are touched;
//! every other setting and hook is preserved. A settings file that is not
//! valid JSON is left alone and reported.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

/// The events `openpets-rs hook` reacts to. `Notification` maps to nothing,
/// so it is not installed.
const HOOK_EVENTS: [&str; 5] = [
    "UserPromptSubmit",
    "PreToolUse",
    "PermissionRequest",
    "Stop",
    "StopFailure",
];
const HOOK_SUFFIX: &str = " hook";
const HOOK_MARKER: &str = "openpets-rs";

/// `$CLAUDE_CONFIG_DIR/settings.json`, or `~/.claude/settings.json`.
pub fn settings_path() -> PathBuf {
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR") {
        return PathBuf::from(dir).join("settings.json");
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join(".claude/settings.json")
}

/// The shell command Claude Code should run, quoting the binary path.
pub fn hook_command(executable: &str) -> String {
    format!("{}{HOOK_SUFFIX}", shell_quote(executable))
}

/// Single-quotes `text` for `sh`.
pub fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// This binary's path, for hook commands.
pub fn current_executable() -> Result<String, String> {
    let executable =
        std::env::current_exe().map_err(|error| format!("cannot locate this binary: {error}"))?;
    executable
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| "binary path is not valid UTF-8".to_owned())
}

fn is_our_command(command: &str) -> bool {
    command.contains(HOOK_MARKER) && command.ends_with(HOOK_SUFFIX)
}

/// Adds one hook entry per event, replacing any previous `openpets-rs hook`
/// entry (for example one pointing at an older build).
pub fn add_hooks(settings: Value, command: &str) -> Result<Value, String> {
    let mut settings = remove_hooks(settings)?;
    let root = settings
        .as_object_mut()
        .ok_or("settings must be a JSON object")?;
    let hooks = root
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or("`hooks` must be a JSON object")?;

    for event in HOOK_EVENTS {
        let groups = hooks
            .entry(event)
            .or_insert_with(|| Value::Array(Vec::new()))
            .as_array_mut()
            .ok_or_else(|| format!("`hooks.{event}` must be an array"))?;
        groups.push(json!({
            "hooks": [{ "type": "command", "command": command, "timeout": 3, "async": true }]
        }));
    }
    Ok(settings)
}

/// Removes every `openpets-rs hook` entry, dropping groups and events that
/// become empty because of it.
pub fn remove_hooks(mut settings: Value) -> Result<Value, String> {
    let root = settings
        .as_object_mut()
        .ok_or("settings must be a JSON object")?;
    let Some(hooks) = root.get_mut("hooks") else {
        return Ok(settings);
    };
    let hooks = hooks
        .as_object_mut()
        .ok_or("`hooks` must be a JSON object")?;

    for groups in hooks.values_mut() {
        let Some(groups) = groups.as_array_mut() else {
            continue;
        };
        for group in groups.iter_mut() {
            if let Some(entries) = group.get_mut("hooks").and_then(Value::as_array_mut) {
                entries.retain(|entry| {
                    let command = entry
                        .get("command")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    !is_our_command(command)
                });
            }
        }
        groups.retain(|group| {
            let entries = group.get("hooks").and_then(Value::as_array);
            entries.is_none_or(|entries| !entries.is_empty())
        });
    }
    hooks.retain(|_, groups| groups.as_array().is_none_or(|groups| !groups.is_empty()));
    if hooks.is_empty() {
        root.remove("hooks");
    }
    Ok(settings)
}

pub fn install() -> Result<(), String> {
    let command = hook_command(&current_executable()?);
    update_json_file(&settings_path(), |settings| add_hooks(settings, &command))?;
    println!("Installed Claude Code hooks running: {command}");
    println!("Restart Claude Code sessions to pick them up.");
    Ok(())
}

pub fn uninstall() -> Result<(), String> {
    update_json_file(&settings_path(), remove_hooks)?;
    println!("Removed openpets-rs hooks from Claude Code settings.");
    Ok(())
}

/// Rewrites the JSON object at `path` (a missing file reads as `{}`).
pub fn update_json_file(
    path: &Path,
    change: impl FnOnce(Value) -> Result<Value, String>,
) -> Result<(), String> {
    let settings = match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).map_err(|error| {
            format!(
                "{} is not valid JSON ({error}); left unchanged",
                path.display()
            )
        })?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => json!({}),
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    };

    let updated = change(settings)?;
    let mut text = serde_json::to_string_pretty(&updated).map_err(|error| error.to_string())?;
    text.push('\n');

    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|error| format!("cannot create {}: {error}", dir.display()))?;
    }
    // Write beside the target and rename, so the agent never reads a half-written file.
    let temporary = path.with_extension("json.openpets-rs.tmp");
    std::fs::write(&temporary, text)
        .map_err(|error| format!("cannot write {}: {error}", temporary.display()))?;
    std::fs::rename(&temporary, path)
        .map_err(|error| format!("cannot replace {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const COMMAND: &str = "'/Applications/OpenPets.app/Contents/MacOS/openpets-rs' hook";

    fn our_entry_count(settings: &Value) -> usize {
        settings["hooks"]
            .as_object()
            .into_iter()
            .flat_map(|hooks| hooks.values())
            .flat_map(|groups| groups.as_array().cloned().unwrap_or_default())
            .flat_map(|group| group["hooks"].as_array().cloned().unwrap_or_default())
            .filter(|entry| entry["command"].as_str().is_some_and(is_our_command))
            .count()
    }

    #[test]
    fn installing_twice_leaves_one_entry_per_event_and_keeps_other_hooks() {
        let settings = json!({
            "theme": "dark",
            "hooks": { "PreToolUse": [{ "matcher": "Bash", "hooks": [{ "type": "command", "command": "echo safe" }] }] }
        });

        let once = add_hooks(settings, COMMAND).unwrap();
        let twice = add_hooks(once, COMMAND).unwrap();

        assert_eq!(our_entry_count(&twice), HOOK_EVENTS.len());
        assert_eq!(twice["theme"], "dark");
        assert_eq!(
            twice["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
            "echo safe"
        );
    }

    #[test]
    fn uninstalling_removes_only_openpets_rs_entries() {
        let settings = json!({
            "hooks": { "Stop": [{ "hooks": [{ "type": "command", "command": "say done" }] }] }
        });
        let installed = add_hooks(settings, COMMAND).unwrap();

        let removed = remove_hooks(installed).unwrap();

        assert_eq!(
            removed,
            json!({ "hooks": { "Stop": [{ "hooks": [{ "type": "command", "command": "say done" }] }] } })
        );
    }

    #[test]
    fn settings_with_an_unexpected_shape_are_refused() {
        assert!(add_hooks(json!({ "hooks": { "Stop": { "bad": true } } }), COMMAND).is_err());
        assert!(add_hooks(json!([]), COMMAND).is_err());
    }
}
