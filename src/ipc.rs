//! Local socket protocol between `openpets-rs mcp` and the running app.
//!
//! One newline-terminated JSON request per connection, answered by one
//! newline-terminated JSON response. The socket lives in the user's private
//! data directory, so only the user's own processes can reach it.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::mpsc::{self, Sender};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::reaction::{Reaction, validate_message};

const IO_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_REQUEST_BYTES: u64 = 4096;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    Status,
    React {
        reaction: Reaction,
    },
    Say {
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reaction: Option<Reaction>,
    },
}

/// The pet the app is showing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PetInfo {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pet: Option<PetInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Response {
    pub fn ok(pet: PetInfo) -> Response {
        Response {
            ok: true,
            pet: Some(pet),
            error: None,
        }
    }

    pub fn error(message: impl Into<String>) -> Response {
        Response {
            ok: false,
            pet: None,
            error: Some(message.into()),
        }
    }
}

/// A validated request waiting for the app to answer it on the main thread.
pub struct Incoming {
    pub request: Request,
    pub reply: Sender<Response>,
}

/// Binds the socket and answers connections on a background thread.
///
/// Requests are validated here, then handed to the main thread through
/// `deliver`, which answers on each request's `reply` channel.
pub fn serve(path: &Path, deliver: Sender<Incoming>) -> std::io::Result<()> {
    let listener = UnixListener::bind(path)?;
    std::thread::Builder::new()
        .name("ipc".into())
        .spawn(move || {
            for stream in listener.incoming() {
                match stream {
                    Ok(stream) => {
                        if let Err(error) = handle_connection(stream, &deliver) {
                            eprintln!("[ipc] connection failed: {error}");
                        }
                    }
                    Err(error) => eprintln!("[ipc] accept failed: {error}"),
                }
            }
        })?;
    Ok(())
}

fn handle_connection(stream: UnixStream, deliver: &Sender<Incoming>) -> std::io::Result<()> {
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;

    let mut line = String::new();
    BufReader::new((&stream).take(MAX_REQUEST_BYTES)).read_line(&mut line)?;

    let response = match parse_request(&line) {
        Ok(request) => dispatch(request, deliver),
        Err(message) => Response::error(message),
    };

    let mut stream = stream;
    serde_json::to_writer(&mut stream, &response)?;
    stream.write_all(b"\n")
}

fn parse_request(line: &str) -> Result<Request, String> {
    let request: Request =
        serde_json::from_str(line.trim()).map_err(|_| "Invalid request.".to_owned())?;

    match request {
        Request::Say { message, reaction } => {
            let message = validate_message(&message)?;
            Ok(Request::Say { message, reaction })
        }
        other => Ok(other),
    }
}

fn dispatch(request: Request, deliver: &Sender<Incoming>) -> Response {
    let (reply, answer) = mpsc::channel();
    if deliver.send(Incoming { request, reply }).is_err() {
        return Response::error("OpenPets is shutting down.");
    }
    answer
        .recv_timeout(IO_TIMEOUT)
        .unwrap_or_else(|_| Response::error("OpenPets did not answer in time."))
}

/// Sends one request to the running app.
pub fn send(path: &Path, request: &Request) -> std::io::Result<Response> {
    let mut stream = UnixStream::connect(path)?;
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;

    let mut payload = serde_json::to_vec(request)?;
    payload.push(b'\n');
    stream.write_all(&payload)?;

    let mut line = String::new();
    BufReader::new(stream.take(MAX_REQUEST_BYTES)).read_line(&mut line)?;
    serde_json::from_str(line.trim()).map_err(std::io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_socket(name: &str) -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("openpets-rs-{name}-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        path
    }

    fn glitchcat(name: &str) -> PetInfo {
        PetInfo {
            id: "glitchcat".into(),
            name: name.into(),
        }
    }

    #[test]
    fn requests_reach_the_app_and_answers_come_back() {
        let path = temp_socket("roundtrip");
        let (deliver, inbox) = mpsc::channel::<Incoming>();
        serve(&path, deliver).unwrap();

        let app = std::thread::spawn(move || {
            let incoming = inbox.recv().unwrap();
            let name = match incoming.request {
                Request::React {
                    reaction: Reaction::Waving,
                } => "Glitchcat",
                _ => "unexpected",
            };
            incoming.reply.send(Response::ok(glitchcat(name))).unwrap();
        });

        let response = send(
            &path,
            &Request::React {
                reaction: Reaction::Waving,
            },
        )
        .unwrap();
        app.join().unwrap();
        assert_eq!(response, Response::ok(glitchcat("Glitchcat")));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn unsafe_speech_is_rejected_before_reaching_the_app() {
        let path = temp_socket("unsafe");
        let (deliver, inbox) = mpsc::channel::<Incoming>();
        serve(&path, deliver).unwrap();

        let request = Request::Say {
            message: "token = abc".into(),
            reaction: None,
        };
        let response = send(&path, &request).unwrap();

        assert!(!response.ok);
        assert!(inbox.try_recv().is_err());
        let _ = std::fs::remove_file(&path);
    }
}
