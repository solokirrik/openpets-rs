//! `openpets-rs mcp`: a stdio MCP server that forwards tool calls to the app.
//!
//! MCP over stdio is newline-delimited JSON-RPC 2.0. The surface is a few
//! tools, so the protocol is handled directly with `serde_json` instead of
//! pulling in an async runtime. Stdout carries protocol messages only; all
//! diagnostics go to stderr.

use std::io::{BufRead, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::ipc::{self, Request, Response};
use crate::paths;
use crate::reaction::Reaction;
use crate::timer::{MAX_MINUTES, validate_minutes};

const SUPPORTED_PROTOCOL_VERSIONS: [&str; 4] =
    ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"];
const DEFAULT_PROTOCOL_VERSION: &str = "2025-06-18";
const APP_LAUNCH_TIMEOUT: Duration = Duration::from_secs(5);

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;

/// Sends a request to the app. `Err` means the app could not be reached.
pub type Forward<'a> = dyn FnMut(&Request) -> Result<Response, String> + 'a;

pub fn run() -> std::io::Result<()> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();

    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(reply) = handle_line(&line, &mut forward_to_app) {
            serde_json::to_writer(&mut stdout, &reply)?;
            stdout.write_all(b"\n")?;
            stdout.flush()?;
        }
    }
    Ok(())
}

/// Answers one line of input: a single message, or a batch (an array of
/// messages, allowed by protocol 2025-03-26). Returns `None` when nothing
/// needs a reply, such as a notification.
pub fn handle_line(line: &str, forward: &mut Forward) -> Option<Value> {
    let Ok(message) = serde_json::from_str::<Value>(line) else {
        return Some(error_response(Value::Null, PARSE_ERROR, "Parse error"));
    };

    let Value::Array(batch) = message else {
        return handle_message(&message, forward);
    };
    if batch.is_empty() {
        return Some(error_response(Value::Null, INVALID_REQUEST, "Empty batch"));
    }
    let replies: Vec<Value> = batch
        .iter()
        .filter_map(|message| handle_message(message, forward))
        .collect();
    if replies.is_empty() {
        None
    } else {
        Some(Value::Array(replies))
    }
}

fn handle_message(message: &Value, forward: &mut Forward) -> Option<Value> {
    if !message.is_object() {
        return Some(error_response(
            Value::Null,
            INVALID_REQUEST,
            "Invalid Request",
        ));
    }
    // Messages without an id are notifications (or responses) and get no reply.
    let id = message.get("id").cloned()?;
    let method = message
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let params = message.get("params").cloned().unwrap_or(Value::Null);

    let result = match method {
        "initialize" => initialize(&params),
        "ping" => json!({}),
        "tools/list" => json!({ "tools": tool_definitions() }),
        "tools/call" => call_tool(&params, forward),
        _ => return Some(error_response(id, METHOD_NOT_FOUND, "Method not found")),
    };
    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

fn initialize(params: &Value) -> Value {
    let requested = params.get("protocolVersion").and_then(Value::as_str);
    let version = requested
        .filter(|version| SUPPORTED_PROTOCOL_VERSIONS.contains(version))
        .unwrap_or(DEFAULT_PROTOCOL_VERSION);

    json!({
        "protocolVersion": version,
        "capabilities": { "tools": {} },
        "serverInfo": { "name": "openpets", "version": env!("CARGO_PKG_VERSION") },
        "instructions": "Interact with the user's OpenPets desktop pet. Use openpets_status first. Use openpets_say only for short status/personality messages, never code, logs, secrets, URLs, or file paths."
    })
}

fn tool_definitions() -> Value {
    let reactions: Vec<&str> = Reaction::ALL
        .iter()
        .map(|reaction| reaction.name())
        .collect();
    json!([
        {
            "name": "openpets_status",
            "title": "OpenPets Status",
            "description": "Check whether OpenPets is running and which pet it shows.",
            "inputSchema": { "type": "object", "properties": {} },
            "annotations": { "readOnlyHint": true, "idempotentHint": true }
        },
        {
            "name": "openpets_react",
            "title": "OpenPets React",
            "description": "Set a short coding-oriented reaction on the OpenPets desktop pet.",
            "inputSchema": {
                "type": "object",
                "properties": { "reaction": { "type": "string", "enum": reactions } },
                "required": ["reaction"]
            },
            "annotations": { "readOnlyHint": false, "idempotentHint": false }
        },
        {
            "name": "openpets_say",
            "title": "OpenPets Say",
            "description": "Show a short safe message (max 140 characters) on the OpenPets desktop pet. Do not send code, logs, secrets, URLs, or file paths.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "message": { "type": "string", "minLength": 1, "maxLength": 140 },
                    "reaction": { "type": "string", "enum": reactions }
                },
                "required": ["message"]
            },
            "annotations": { "readOnlyHint": false, "idempotentHint": false }
        },
        {
            "name": "openpets_start_timer",
            "title": "OpenPets Start Timer",
            "description": "Start a countdown shown above the OpenPets desktop pet; the pet celebrates with a chime when it ends. Replaces a running timer. Use only when the user asks for a timer.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "minutes": { "type": "integer", "minimum": 1, "maximum": MAX_MINUTES }
                },
                "required": ["minutes"]
            },
            "annotations": { "readOnlyHint": false, "idempotentHint": false }
        }
    ])
}

fn call_tool(params: &Value, forward: &mut Forward) -> Value {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let arguments = params.get("arguments").cloned().unwrap_or(Value::Null);

    let request = match build_request(name, &arguments) {
        Ok(request) => request,
        Err(message) => return tool_error(&message),
    };

    match forward(&request) {
        Ok(response) if response.ok => tool_success(&request, &response),
        Ok(response) => {
            let message = response
                .error
                .unwrap_or_else(|| "OpenPets rejected the request.".into());
            tool_error(&message)
        }
        // Not running is a normal answer to a status check, not a failure.
        Err(reason) if request == Request::Status => json!({
            "content": [{ "type": "text", "text": format!("OpenPets is not running. {reason}") }],
            "structuredContent": { "appRunning": false, "unavailableReason": reason },
            "isError": false
        }),
        Err(reason) => tool_error(&reason),
    }
}

fn build_request(name: &str, arguments: &Value) -> Result<Request, String> {
    match name {
        "openpets_status" => Ok(Request::Status),
        "openpets_react" => {
            let reaction = arguments.get("reaction").unwrap_or(&Value::Null);
            Ok(Request::React {
                reaction: parse_reaction(reaction)?,
            })
        }
        "openpets_say" => {
            let message = arguments
                .get("message")
                .and_then(Value::as_str)
                .ok_or("A message is required.")?
                .to_owned();
            let reaction = match arguments.get("reaction") {
                None | Some(Value::Null) => None,
                Some(value) => Some(parse_reaction(value)?),
            };
            Ok(Request::Say { message, reaction })
        }
        "openpets_start_timer" => {
            let minutes = arguments
                .get("minutes")
                .and_then(Value::as_u64)
                .ok_or(format!(
                    "minutes must be a whole number from 1 to {MAX_MINUTES}."
                ))?;
            Ok(Request::StartTimer {
                minutes: validate_minutes(minutes)?,
            })
        }
        _ => Err(format!("Unknown tool: {name}")),
    }
}

fn parse_reaction(value: &Value) -> Result<Reaction, String> {
    if let Some(reaction) = value.as_str().and_then(Reaction::parse) {
        return Ok(reaction);
    }
    let names: Vec<&str> = Reaction::ALL
        .iter()
        .map(|reaction| reaction.name())
        .collect();
    Err(format!(
        "Invalid reaction. Use one of: {}",
        names.join(", ")
    ))
}

fn tool_success(request: &Request, response: &Response) -> Value {
    let (text, structured) = match request {
        Request::Status => {
            let pet_name = response
                .pet
                .as_ref()
                .map(|pet| pet.name.as_str())
                .unwrap_or("a pet");
            let structured = json!({
                "appRunning": true,
                "petId": response.pet.as_ref().map(|pet| &pet.id),
                "petName": response.pet.as_ref().map(|pet| &pet.name),
            });
            (
                format!("OpenPets is running and showing {pet_name}."),
                structured,
            )
        }
        Request::React { reaction } => (
            format!("OpenPets reaction sent: {}", reaction.name()),
            json!({ "ok": true, "reaction": reaction.name() }),
        ),
        Request::Say { reaction, .. } => (
            "OpenPets message shown.".to_owned(),
            json!({ "ok": true, "reaction": reaction.map(Reaction::name) }),
        ),
        Request::StartTimer { minutes } => (
            format!("OpenPets timer started for {minutes} min."),
            json!({ "ok": true, "minutes": minutes }),
        ),
        Request::SetBadge { badge } => (
            "OpenPets badge updated.".to_owned(),
            json!({ "ok": true, "badge": badge }),
        ),
    };
    json!({
        "content": [{ "type": "text", "text": text }],
        "structuredContent": structured,
        "isError": false
    })
}

fn tool_error(text: &str) -> Value {
    json!({
        "content": [{ "type": "text", "text": text }],
        "isError": true
    })
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// Sends a request to the app, launching it first when it is not running.
/// A status check never launches the app: it reports what is.
fn forward_to_app(request: &Request) -> Result<Response, String> {
    let socket = paths::socket_path();
    if let Ok(response) = ipc::send(&socket, request) {
        return Ok(response);
    }
    if *request == Request::Status {
        return Err("Start it from the menu bar or send a reaction to launch it.".into());
    }

    launch_app()?;
    let deadline = Instant::now() + APP_LAUNCH_TIMEOUT;
    loop {
        std::thread::sleep(Duration::from_millis(150));
        match ipc::send(&socket, request) {
            Ok(response) => return Ok(response),
            Err(_) if Instant::now() < deadline => continue,
            Err(_) => return Err("OpenPets did not start in time.".into()),
        }
    }
}

fn launch_app() -> Result<(), String> {
    let executable = std::env::current_exe().map_err(|_| "Cannot locate the OpenPets app.")?;
    eprintln!("[mcp] starting the OpenPets app");
    Command::new(executable)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|_| "Could not start the OpenPets app.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::PetInfo;

    fn glitchcat() -> Response {
        Response::ok(PetInfo {
            id: "glitchcat".into(),
            name: "Glitchcat".into(),
        })
    }

    fn call(name: &str, arguments: Value, forward: &mut Forward) -> Value {
        let message = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": name, "arguments": arguments }
        });
        handle_line(&message.to_string(), forward).unwrap()["result"].clone()
    }

    #[test]
    fn lists_the_openpets_tools() {
        let message = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" });
        let reply = handle_line(&message.to_string(), &mut |_| unreachable!()).unwrap();
        let names: Vec<&str> = reply["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();

        assert_eq!(
            names,
            [
                "openpets_status",
                "openpets_react",
                "openpets_say",
                "openpets_start_timer"
            ]
        );
    }

    #[test]
    fn status_reports_the_shown_pet() {
        let result = call("openpets_status", json!({}), &mut |_| Ok(glitchcat()));

        assert_eq!(result["isError"], false);
        assert_eq!(result["structuredContent"]["appRunning"], true);
        assert_eq!(result["structuredContent"]["petId"], "glitchcat");
    }

    #[test]
    fn status_of_a_stopped_app_is_an_answer_not_an_error() {
        let result = call("openpets_status", json!({}), &mut |_| {
            Err("not running".into())
        });

        assert_eq!(result["isError"], false);
        assert_eq!(result["structuredContent"]["appRunning"], false);
    }

    #[test]
    fn valid_reaction_is_forwarded_to_the_app() {
        let mut sent = Vec::new();
        let result = call(
            "openpets_react",
            json!({ "reaction": "success" }),
            &mut |request| {
                sent.push(request.clone());
                Ok(glitchcat())
            },
        );

        assert_eq!(result["isError"], false);
        assert_eq!(
            sent,
            [Request::React {
                reaction: Reaction::Success
            }]
        );
    }

    #[test]
    fn invalid_reaction_is_a_tool_error_and_is_not_forwarded() {
        let result = call(
            "openpets_react",
            json!({ "reaction": "bad" }),
            &mut |_| unreachable!(),
        );

        assert_eq!(result["isError"], true);
    }

    #[test]
    fn timer_is_started_in_the_app() {
        let mut sent = Vec::new();
        let result = call(
            "openpets_start_timer",
            json!({ "minutes": 25 }),
            &mut |request| {
                sent.push(request.clone());
                Ok(glitchcat())
            },
        );

        assert_eq!(result["isError"], false);
        assert_eq!(sent, [Request::StartTimer { minutes: 25 }]);
    }

    #[test]
    fn timer_outside_one_minute_to_one_day_is_refused() {
        for minutes in [json!(0), json!(1441), json!(-5), json!(2.5), json!("10")] {
            let result = call(
                "openpets_start_timer",
                json!({ "minutes": minutes }),
                &mut |_| unreachable!(),
            );

            assert_eq!(result["isError"], true, "minutes {minutes}");
        }
    }

    #[test]
    fn app_rejection_is_reported_as_a_tool_error() {
        let result = call(
            "openpets_say",
            json!({ "message": "const x = 1" }),
            &mut |_| Ok(Response::error("Message must not contain code.")),
        );

        assert_eq!(result["isError"], true);
        assert_eq!(
            result["content"][0]["text"],
            "Message must not contain code."
        );
    }

    #[test]
    fn unreachable_app_fails_a_reaction() {
        let result = call(
            "openpets_react",
            json!({ "reaction": "waving" }),
            &mut |_| Err("OpenPets did not start in time.".into()),
        );

        assert_eq!(result["isError"], true);
    }

    #[test]
    fn notifications_get_no_reply() {
        let message = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });

        assert!(handle_line(&message.to_string(), &mut |_| unreachable!()).is_none());
    }

    #[test]
    fn batch_requests_get_one_reply_per_request() {
        let batch = json!([
            { "jsonrpc": "2.0", "method": "notifications/initialized" },
            { "jsonrpc": "2.0", "id": 1, "method": "ping" },
            { "jsonrpc": "2.0", "id": 2, "method": "tools/list" }
        ]);
        let reply = handle_line(&batch.to_string(), &mut |_| unreachable!()).unwrap();
        let ids: Vec<i64> = reply
            .as_array()
            .unwrap()
            .iter()
            .map(|reply| reply["id"].as_i64().unwrap())
            .collect();

        assert_eq!(ids, [1, 2]);
    }

    #[test]
    fn malformed_input_gets_a_parse_error() {
        let reply = handle_line("{ not json", &mut |_| unreachable!()).unwrap();

        assert_eq!(reply["error"]["code"], PARSE_ERROR);
    }
}
