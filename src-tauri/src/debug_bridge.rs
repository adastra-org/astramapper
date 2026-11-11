//! A debug bridge: lets a developer (or a coding agent) watch the running app
//! and drive its camera from outside, over a loopback HTTP port.
//!
//! # Why this exists
//!
//! Most of what World does cannot be checked by a unit test: whether streaming
//! keeps up while flying, what the console says, what F3 reads at the city. The
//! only way to know was a person flying the camera and reading the overlay. The
//! bridge makes the same things reachable from a shell: read the console and the
//! perf numbers as they happen, move the camera, take a screenshot.
//!
//! # Shape
//!
//! ```text
//!  shell (curl)  --HTTP-->  this module  <--invoke(debug_bridge_sync)--  WebView
//!                           logs, state,                                 console capture,
//!                           command queue                                state, commands
//! ```
//!
//! The WebView calls `debug_bridge_sync` four times a second. One call carries
//! the console lines and, now and then, a state snapshot up, and brings the
//! queued commands back down. The WebView never listens on a port; the HTTP
//! side only ever talks to the queues in this module.
//!
//! # Off unless asked for
//!
//! It runs in debug builds (`tauri dev`) and in a release build only when
//! `ASTRAMAPPER_DEBUG_BRIDGE=1` is set; `ASTRAMAPPER_DEBUG_BRIDGE=0` turns it off
//! in a debug build. A shipped app has no listening port.
//!
//! # Authentication
//!
//! It binds `127.0.0.1` only, and every request needs the token the app wrote to
//! `astramapper-debug-bridge.json` in the temp directory (`Authorization: Bearer`
//! header or `?token=`). A web page cannot guess the token, which is what stops a
//! page in the user's browser from driving the app through a `fetch` to
//! localhost. The `Host` header must also be a loopback name, against DNS
//! rebinding. No dependency is added: the server is a hand-written HTTP/1.0
//! subset, since it speaks to `curl` and nothing else.
//!
//! # Routes
//!
//! | Route | |
//! | --- | --- |
//! | `GET /health` | `{ "ok": true }` |
//! | `GET /state` | the WebView's last snapshot, plus the backend's perf report |
//! | `GET /logs?since=N&limit=M` | console and backend lines with `seq > N` |
//! | `POST /cmd` | queue one command (a JSON object); returns its `id` |
//! | `GET /screenshot` | queue a screenshot and wait for the PNG |

use std::collections::{HashMap, VecDeque};
use std::hash::{BuildHasher, Hasher};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// How many log lines are kept. Older ones fall off the front.
const LOG_CAP: usize = 4000;
/// The longest line kept, in bytes. A console dump of a big object is cut here.
const LINE_MAX: usize = 4000;
/// Commands waiting for the WebView beyond this are dropped oldest first.
const COMMAND_CAP: usize = 256;
/// Largest request body accepted.
const BODY_MAX: usize = 64 * 1024;
/// Largest request head (request line and headers) accepted.
const HEAD_MAX: usize = 16 * 1024;
/// How long `GET /screenshot` waits for the WebView to deliver.
const SCREENSHOT_WAIT: Duration = Duration::from_secs(6);
/// The port tried first; any free one is used if it is taken.
const PREFERRED_PORT: u16 = 17420;

/// Whether the bridge should run in this process.
pub fn enabled() -> bool {
    enabled_with(std::env::var("ASTRAMAPPER_DEBUG_BRIDGE").ok().as_deref())
}

fn enabled_with(flag: Option<&str>) -> bool {
    match flag {
        Some("0") => false,
        Some(value) if !value.is_empty() => true,
        _ => cfg!(debug_assertions),
    }
}

#[derive(Debug, Clone, Serialize)]
struct LogLine {
    seq: u64,
    /// Milliseconds since the bridge started.
    t: u64,
    /// `console` for the WebView, `rust` for the backend.
    source: String,
    level: String,
    text: String,
}

/// A line sent up by the WebView.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IncomingLog {
    level: String,
    text: String,
}

#[derive(Debug, Clone, Serialize)]
struct Command {
    id: u64,
    #[serde(flatten)]
    body: Value,
}

struct Bridge {
    started: Instant,
    logs: VecDeque<LogLine>,
    next_seq: u64,
    state: Option<Value>,
    state_at: Option<Instant>,
    commands: VecDeque<Command>,
    next_command: u64,
    screenshot: Option<Vec<u8>>,
    screenshot_count: u64,
}

impl Bridge {
    fn new() -> Self {
        Self {
            started: Instant::now(),
            logs: VecDeque::new(),
            next_seq: 1,
            state: None,
            state_at: None,
            commands: VecDeque::new(),
            next_command: 1,
            screenshot: None,
            screenshot_count: 0,
        }
    }

    fn push_log(&mut self, source: &str, level: &str, text: &str) {
        let seq = self.next_seq;
        self.next_seq += 1;
        if self.logs.len() == LOG_CAP {
            self.logs.pop_front();
        }
        self.logs.push_back(LogLine {
            seq,
            t: u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX),
            source: source.to_string(),
            level: level.to_string(),
            text: clip(text),
        });
    }

    fn push_command(&mut self, body: Value) -> u64 {
        let id = self.next_command;
        self.next_command += 1;
        if self.commands.len() == COMMAND_CAP {
            self.commands.pop_front();
        }
        self.commands.push_back(Command { id, body });
        id
    }

    fn logs_since(&self, since: u64, limit: usize) -> (Vec<LogLine>, u64) {
        let lines: Vec<LogLine> = self
            .logs
            .iter()
            .filter(|line| line.seq > since)
            .take(limit)
            .cloned()
            .collect();
        let next = lines.last().map_or(since, |line| line.seq);
        (lines, next)
    }
}

/// `text` cut to `LINE_MAX` bytes on a character boundary.
fn clip(text: &str) -> String {
    if text.len() <= LINE_MAX {
        return text.to_string();
    }
    let mut end = LINE_MAX;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}... [{} bytes cut]", &text[..end], text.len() - end)
}

struct Shared {
    bridge: Mutex<Bridge>,
    /// Signalled when a screenshot arrives.
    screenshot_ready: Condvar,
    token: OnceLock<String>,
}

fn shared() -> &'static Shared {
    static SHARED: OnceLock<Shared> = OnceLock::new();
    SHARED.get_or_init(|| Shared {
        bridge: Mutex::new(Bridge::new()),
        screenshot_ready: Condvar::new(),
        token: OnceLock::new(),
    })
}

fn lock() -> std::sync::MutexGuard<'static, Bridge> {
    match shared().bridge.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Record a backend line, when the bridge is running. Cheap when it is not.
pub fn log(level: &str, text: &str) {
    if shared().token.get().is_none() {
        return;
    }
    lock().push_log("rust", level, text);
}

// ---------------------------------------------------------------- HTTP

#[derive(Debug, PartialEq, Eq)]
struct Request {
    method: String,
    path: String,
    query: HashMap<String, String>,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq)]
enum ParseError {
    Malformed,
    TooLarge,
}

fn decode_query(raw: &str) -> HashMap<String, String> {
    raw.split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (key.to_string(), value.to_string())
        })
        .collect()
}

/// Read one request from `reader`.
fn read_request(reader: &mut impl BufRead) -> Result<Request, ParseError> {
    let mut head = 0usize;
    let mut line = String::new();
    let mut read_line = |reader: &mut dyn BufRead, line: &mut String| -> Result<(), ParseError> {
        line.clear();
        let n = reader.read_line(line).map_err(|_| ParseError::Malformed)?;
        if n == 0 {
            return Err(ParseError::Malformed);
        }
        head += n;
        if head > HEAD_MAX {
            return Err(ParseError::TooLarge);
        }
        Ok(())
    };
    read_line(reader, &mut line)?;
    let mut parts = line.split_whitespace();
    let method = parts.next().ok_or(ParseError::Malformed)?.to_string();
    let target = parts.next().ok_or(ParseError::Malformed)?.to_string();
    let (path, query) = match target.split_once('?') {
        Some((path, query)) => (path.to_string(), decode_query(query)),
        None => (target, HashMap::new()),
    };
    let mut headers = HashMap::new();
    loop {
        read_line(reader, &mut line)?;
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            break;
        }
        if let Some((name, value)) = trimmed.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }
    let length: usize = headers
        .get("content-length")
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    if length > BODY_MAX {
        return Err(ParseError::TooLarge);
    }
    let mut body = vec![0u8; length];
    reader
        .read_exact(&mut body)
        .map_err(|_| ParseError::Malformed)?;
    Ok(Request {
        method,
        path,
        query,
        headers,
        body,
    })
}

struct Response {
    status: u16,
    content_type: &'static str,
    body: Vec<u8>,
}

impl Response {
    fn json(status: u16, value: &Value) -> Self {
        Self {
            status,
            content_type: "application/json",
            body: value.to_string().into_bytes(),
        }
    }

    fn error(status: u16, message: &str) -> Self {
        Self::json(status, &json!({ "error": message }))
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        413 => "Payload Too Large",
        504 => "Gateway Timeout",
        _ => "Error",
    }
}

fn write_response(stream: &mut TcpStream, response: &Response) {
    let head = format!(
        "HTTP/1.0 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        response.status,
        reason(response.status),
        response.content_type,
        response.body.len()
    );
    // The client may have gone; nothing useful to do about it.
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(&response.body);
}

/// Whether `host` (a `Host` header value) names the loopback interface.
fn loopback_host(host: &str) -> bool {
    let name = host.rsplit_once(':').map_or(host, |(name, _)| name);
    matches!(name, "127.0.0.1" | "localhost" | "[::1]")
}

fn authorized(request: &Request, token: &str) -> bool {
    let bearer = request
        .headers
        .get("authorization")
        .and_then(|value| value.strip_prefix("Bearer "));
    let given = bearer.or_else(|| request.query.get("token").map(String::as_str));
    given == Some(token)
}

fn handle(request: &Request, token: &str) -> Response {
    if !request
        .headers
        .get("host")
        .is_some_and(|host| loopback_host(host))
    {
        return Response::error(403, "host must be a loopback name");
    }
    if !authorized(request, token) {
        return Response::error(401, "missing or wrong token");
    }
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/health") => Response::json(200, &json!({ "ok": true })),
        ("GET", "/state") => route_state(),
        ("GET", "/logs") => route_logs(request),
        ("POST", "/cmd") => route_command(request),
        ("GET", "/screenshot") => route_screenshot(),
        _ => Response::error(404, "unknown route"),
    }
}

fn route_state() -> Response {
    let (state, age_ms) = {
        let bridge = lock();
        (
            bridge.state.clone(),
            bridge
                .state_at
                .map(|at| u64::try_from(at.elapsed().as_millis()).unwrap_or(u64::MAX)),
        )
    };
    let backend = serde_json::to_value(crate::perf::report()).unwrap_or(Value::Null);
    Response::json(
        200,
        &json!({ "state": state, "ageMs": age_ms, "backend": backend }),
    )
}

fn route_logs(request: &Request) -> Response {
    let since = request
        .query
        .get("since")
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    let limit = request
        .query
        .get("limit")
        .and_then(|value| value.parse().ok())
        .unwrap_or(500)
        .min(LOG_CAP);
    let (lines, next) = lock().logs_since(since, limit);
    Response::json(200, &json!({ "next": next, "logs": lines }))
}

fn route_command(request: &Request) -> Response {
    let Ok(body) = serde_json::from_slice::<Value>(&request.body) else {
        return Response::error(400, "body must be JSON");
    };
    if !body.get("type").is_some_and(Value::is_string) {
        return Response::error(400, "a command needs a string \"type\"");
    }
    let id = lock().push_command(body);
    Response::json(200, &json!({ "id": id }))
}

fn route_screenshot() -> Response {
    let wanted = {
        let mut bridge = lock();
        bridge.push_command(json!({ "type": "screenshot" }));
        bridge.screenshot_count + 1
    };
    let deadline = Instant::now() + SCREENSHOT_WAIT;
    let mut bridge = lock();
    while bridge.screenshot_count < wanted {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Response::error(504, "the WebView did not deliver a screenshot");
        }
        bridge = match shared().screenshot_ready.wait_timeout(bridge, left) {
            Ok((guard, _)) => guard,
            Err(poisoned) => poisoned.into_inner().0,
        };
    }
    match bridge.screenshot.clone() {
        Some(png) => Response {
            status: 200,
            content_type: "image/png",
            body: png,
        },
        None => Response::error(504, "the WebView reported no canvas to capture"),
    }
}

fn serve_connection(mut stream: TcpStream, token: &str) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let Ok(clone) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(clone);
    let response = match read_request(&mut reader) {
        Ok(request) => handle(&request, token),
        Err(ParseError::TooLarge) => Response::error(413, "request too large"),
        Err(ParseError::Malformed) => Response::error(400, "malformed request"),
    };
    write_response(&mut stream, &response);
}

/// 128 random bits as hex, from the OS-seeded keys of `RandomState`.
fn new_token() -> String {
    let mut out = String::with_capacity(32);
    for salt in 0u64..2 {
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u64(salt);
        out.push_str(&format!("{:016x}", hasher.finish()));
    }
    out
}

fn info_path() -> std::path::PathBuf {
    std::env::temp_dir().join("astramapper-debug-bridge.json")
}

/// Start the listener on its own thread. Returns the port, or `None` when the
/// bridge is off or nothing could be bound.
pub fn start() -> Option<u16> {
    if !enabled() {
        return None;
    }
    let listener = TcpListener::bind(("127.0.0.1", PREFERRED_PORT))
        .or_else(|_| TcpListener::bind(("127.0.0.1", 0)))
        .ok()?;
    let port = listener.local_addr().ok()?.port();
    let token = new_token();
    shared().token.set(token.clone()).ok()?;
    let info = json!({
        "port": port,
        "token": token,
        "pid": std::process::id(),
        "url": format!("http://127.0.0.1:{port}"),
    });
    if std::fs::write(info_path(), info.to_string()).is_err() {
        eprintln!("[debug-bridge] could not write {}", info_path().display());
    }
    eprintln!(
        "[debug-bridge] http://127.0.0.1:{port} (token in {})",
        info_path().display()
    );
    lock().push_log("rust", "info", &format!("debug bridge listening on {port}"));
    std::thread::Builder::new()
        .name("debug-bridge".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                let token = token.clone();
                // A slow or stuck client must not hold the others up; screenshot
                // waits are the long ones.
                let _ = std::thread::Builder::new()
                    .name("debug-bridge-conn".into())
                    .spawn(move || serve_connection(stream, &token));
            }
        })
        .ok()?;
    Some(port)
}

// ---------------------------------------------------------------- IPC

/// What the WebView learns at startup: whether to run its half, and where.
#[derive(Serialize)]
pub struct BridgeInfo {
    port: u16,
}

#[tauri::command]
pub fn debug_bridge_info() -> Option<BridgeInfo> {
    shared().token.get()?;
    let port = std::fs::read_to_string(info_path())
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| value.get("port")?.as_u64())
        .and_then(|port| u16::try_from(port).ok())?;
    Some(BridgeInfo { port })
}

/// One round trip: lines and snapshot up, queued commands back.
///
/// `screenshot` is `Some("")` when a capture was asked for and the canvas could
/// not deliver, so the waiting HTTP request fails fast instead of timing out.
#[tauri::command]
pub fn debug_bridge_sync(
    logs: Vec<IncomingLog>,
    state: Option<Value>,
    screenshot: Option<String>,
) -> Vec<Value> {
    if shared().token.get().is_none() {
        return Vec::new();
    }
    let mut bridge = lock();
    for line in &logs {
        bridge.push_log("console", &line.level, &line.text);
    }
    if let Some(state) = state {
        bridge.state = Some(state);
        bridge.state_at = Some(Instant::now());
    }
    if let Some(encoded) = screenshot {
        // Accept a data URL as `canvas.toDataURL` makes it.
        let payload = encoded
            .split_once("base64,")
            .map_or(encoded.as_str(), |(_, data)| data);
        bridge.screenshot = base64::engine::general_purpose::STANDARD
            .decode(payload)
            .ok()
            .filter(|bytes| !bytes.is_empty());
        bridge.screenshot_count += 1;
        shared().screenshot_ready.notify_all();
    }
    bridge
        .commands
        .drain(..)
        .filter_map(|command| serde_json::to_value(command).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn parse(raw: &str) -> Result<Request, ParseError> {
        read_request(&mut BufReader::new(Cursor::new(raw.as_bytes().to_vec())))
    }

    fn get(path: &str, headers: &[(&str, &str)]) -> Request {
        Request {
            method: "GET".into(),
            path: path.into(),
            query: HashMap::new(),
            headers: headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            body: Vec::new(),
        }
    }

    #[test]
    fn the_flag_decides_and_a_debug_build_defaults_on() {
        assert!(!enabled_with(Some("0")));
        assert!(enabled_with(Some("1")));
        assert_eq!(enabled_with(None), cfg!(debug_assertions));
        assert_eq!(enabled_with(Some("")), cfg!(debug_assertions));
    }

    #[test]
    fn a_request_is_split_into_path_query_headers_and_body() {
        let request = parse(
            "POST /cmd?x=1&y=two HTTP/1.1\r\nHost: 127.0.0.1:1\r\nContent-Length: 5\r\n\r\nhello",
        )
        .unwrap();
        assert_eq!(request.method, "POST");
        assert_eq!(request.path, "/cmd");
        assert_eq!(request.query["y"], "two");
        assert_eq!(request.headers["host"], "127.0.0.1:1");
        assert_eq!(request.body, b"hello");
    }

    #[test]
    fn a_huge_body_is_refused_before_it_is_read() {
        let raw = format!(
            "POST /cmd HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
            BODY_MAX + 1
        );
        assert_eq!(parse(&raw), Err(ParseError::TooLarge));
        assert_eq!(parse("\r\n"), Err(ParseError::Malformed));
        assert_eq!(parse(""), Err(ParseError::Malformed));
    }

    #[test]
    fn only_loopback_hosts_pass_and_the_token_is_required() {
        assert!(loopback_host("127.0.0.1:17420"));
        assert!(loopback_host("localhost:9"));
        assert!(!loopback_host("evil.example:80"));
        assert!(!loopback_host("127.0.0.1.evil.example"));

        let ok = get(
            "/health",
            &[("host", "127.0.0.1:1"), ("authorization", "Bearer t")],
        );
        assert_eq!(handle(&ok, "t").status, 200);
        let wrong = get(
            "/health",
            &[("host", "127.0.0.1:1"), ("authorization", "Bearer x")],
        );
        assert_eq!(handle(&wrong, "t").status, 401);
        let none = get("/health", &[("host", "127.0.0.1:1")]);
        assert_eq!(handle(&none, "t").status, 401);
        let rebound = get(
            "/health",
            &[("host", "evil.example"), ("authorization", "Bearer t")],
        );
        assert_eq!(handle(&rebound, "t").status, 403);
        let mut query = get("/health", &[("host", "localhost")]);
        query.query.insert("token".into(), "t".into());
        assert_eq!(handle(&query, "t").status, 200);
    }

    #[test]
    fn logs_keep_a_cap_and_are_read_by_sequence() {
        let mut bridge = Bridge::new();
        for i in 0..(LOG_CAP + 10) {
            bridge.push_log("console", "log", &format!("line {i}"));
        }
        assert_eq!(bridge.logs.len(), LOG_CAP);
        let (lines, next) = bridge.logs_since(0, 3);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].text, "line 10");
        let (rest, _) = bridge.logs_since(next, 10);
        assert_eq!(rest[0].seq, next + 1);
        let (none, same) = bridge.logs_since(u64::MAX - 1, 10);
        assert!(none.is_empty());
        assert_eq!(same, u64::MAX - 1);
    }

    #[test]
    fn a_long_line_is_cut_on_a_character_boundary() {
        let text = "é".repeat(LINE_MAX);
        let clipped = clip(&text);
        assert!(clipped.contains("bytes cut"));
        assert!(clipped.len() < text.len());
        assert_eq!(clip("short"), "short");
    }

    #[test]
    fn commands_get_increasing_ids_and_the_queue_is_bounded() {
        let mut bridge = Bridge::new();
        let first = bridge.push_command(json!({ "type": "a" }));
        let second = bridge.push_command(json!({ "type": "b" }));
        assert_eq!(second, first + 1);
        for _ in 0..(COMMAND_CAP + 5) {
            bridge.push_command(json!({ "type": "c" }));
        }
        assert_eq!(bridge.commands.len(), COMMAND_CAP);
    }

    #[test]
    fn a_command_needs_a_type_and_json() {
        let mut request = get("/cmd", &[]);
        request.method = "POST".into();
        request.body = b"not json".to_vec();
        assert_eq!(route_command(&request).status, 400);
        request.body = br#"{"nope":1}"#.to_vec();
        assert_eq!(route_command(&request).status, 400);
    }

    #[test]
    fn tokens_are_32_hex_digits_and_differ() {
        let a = new_token();
        let b = new_token();
        assert_eq!(a.len(), 32);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }
}
