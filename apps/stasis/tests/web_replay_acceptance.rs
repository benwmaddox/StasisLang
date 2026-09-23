use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{connect, Message};

fn stamp() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos()
}

struct TestTree(PathBuf);

impl TestTree {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!(
            "stasis_web_replay_acceptance_{}_{}",
            std::process::id(),
            stamp()
        )))
    }
}

impl Drop for TestTree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct StaticServer {
    address: String,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl StaticServer {
    fn start(root: &Path) -> Self {
        let root = fs::canonicalize(root).expect("canonical package root");
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind Web package server");
        listener
            .set_nonblocking(true)
            .expect("set local server nonblocking");
        let address = format!("http://{}", listener.local_addr().expect("server address"));
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let server_thread = thread::spawn(move || {
            let mut requests = Vec::new();
            while !thread_stop.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let request_root = root.clone();
                        requests.push(thread::spawn(move || serve_one(&request_root, &mut stream)));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
            for request in requests {
                let _ = request.join();
            }
        });
        Self {
            address,
            stop,
            thread: Some(server_thread),
        }
    }

    fn url(&self, replay_name: &str) -> String {
        format!("{}/index.html?stasis-replay={replay_name}", self.address)
    }
}

impl Drop for StaticServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn serve_one(root: &Path, stream: &mut TcpStream) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let mut request = Vec::new();
    let mut chunk = [0_u8; 1024];
    while request.len() < 16 * 1024 {
        let Ok(read) = stream.read(&mut chunk) else {
            break;
        };
        if read == 0 {
            break;
        }
        request.extend_from_slice(&chunk[..read]);
        if request.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    let request_text = String::from_utf8_lossy(&request);
    let requested = request_text
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/");
    let relative = requested
        .split('?')
        .next()
        .unwrap_or("/")
        .trim_start_matches('/');
    let relative = if relative.is_empty() {
        "index.html"
    } else {
        relative
    };
    let candidate = root.join(relative);
    let file = fs::canonicalize(&candidate)
        .ok()
        .filter(|path| path.starts_with(root))
        .and_then(|path| fs::read(path).ok());
    let (status, content_type, body) = match file {
        Some(bytes) => {
            let content_type = match Path::new(relative).extension().and_then(|ext| ext.to_str()) {
                Some("html") => "text/html; charset=utf-8",
                Some("js" | "mjs") => "text/javascript; charset=utf-8",
                Some("wasm") => "application/wasm",
                Some("json") => "application/json; charset=utf-8",
                Some("css") => "text/css; charset=utf-8",
                Some("png") => "image/png",
                Some("svg") => "image/svg+xml",
                _ => "application/octet-stream",
            };
            ("200 OK", content_type, bytes)
        }
        None => (
            "404 Not Found",
            "text/plain; charset=utf-8",
            b"not found".to_vec(),
        ),
    };
    let header = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(&body);
}

fn browser_executable() -> Option<PathBuf> {
    if let Some(configured) = std::env::var_os("STASIS_WEB_BROWSER") {
        let path = PathBuf::from(configured);
        if path.is_file() {
            return Some(path);
        }
    }
    #[cfg(windows)]
    let candidates = [
        PathBuf::from(r"C:\Program Files\Google\Chrome\Application\chrome.exe"),
        PathBuf::from(r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe"),
    ];
    #[cfg(target_os = "macos")]
    let candidates = [
        PathBuf::from("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
        PathBuf::from("/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge"),
    ];
    #[cfg(all(not(windows), not(target_os = "macos")))]
    let candidates = [
        PathBuf::from("/usr/bin/google-chrome"),
        PathBuf::from("/usr/bin/chromium"),
        PathBuf::from("/usr/bin/chromium-browser"),
        PathBuf::from("/usr/bin/microsoft-edge"),
    ];
    candidates.into_iter().find(|path| path.is_file())
}

struct BrowserProcess(Child);

impl Drop for BrowserProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn cdp_command<S: Read + Write>(
    socket: &mut tungstenite::WebSocket<S>,
    id: u64,
    method: &str,
    params: Value,
    session_id: Option<&str>,
) -> Value {
    let mut request = json!({ "id": id, "method": method, "params": params });
    if let Some(session_id) = session_id {
        request["sessionId"] = json!(session_id);
    }
    socket
        .send(Message::Text(request.to_string().into()))
        .unwrap_or_else(|error| panic!("send CDP {method}: {error}"));
    loop {
        let message = socket
            .read()
            .unwrap_or_else(|error| panic!("read CDP {method}: {error}"));
        let Message::Text(text) = message else {
            continue;
        };
        let response: Value = serde_json::from_str(text.as_ref())
            .unwrap_or_else(|error| panic!("parse CDP {method} response: {error}"));
        if response.get("id").and_then(Value::as_u64) == Some(id) {
            assert!(
                response.get("error").is_none(),
                "CDP {method} failed: {}",
                response["error"]
            );
            return response;
        }
    }
}

fn run_browser(browser: &Path, profile: &Path, url: &str) -> String {
    fs::create_dir_all(profile).expect("create isolated headless browser profile");
    let browser_log = profile.join("chrome.stderr.log");
    let child = Command::new(browser)
        .arg("--headless=new")
        .arg("--no-sandbox")
        .arg("--disable-gpu-sandbox")
        .arg("--disable-background-networking")
        .arg("--disable-component-update")
        .arg("--disable-default-apps")
        .arg("--disable-extensions")
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("--enable-webgl")
        .arg("--enable-unsafe-swiftshader")
        .arg("--ignore-gpu-blocklist")
        .arg("--use-gl=angle")
        .arg("--use-angle=swiftshader")
        .arg("--remote-debugging-address=127.0.0.1")
        .arg("--remote-debugging-port=0")
        .arg("--remote-allow-origins=*")
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(
            fs::File::create(&browser_log).expect("create browser startup log"),
        ))
        .spawn()
        .expect("launch headless browser for packaged Web replay");
    let mut child = BrowserProcess(child);
    let devtools_path = profile.join("DevToolsActivePort");
    let startup_deadline = Instant::now() + Duration::from_secs(15);
    let active_port = loop {
        if let Ok(contents) = fs::read_to_string(&devtools_path) {
            break contents;
        }
        if let Some(status) = child.0.try_wait().expect("check headless browser process") {
            let log = fs::read_to_string(&browser_log).unwrap_or_default();
            let tail = log.chars().rev().take(2_000).collect::<String>();
            let tail = tail.chars().rev().collect::<String>();
            panic!("headless browser exited before DevTools startup: {status}; stderr: {tail}");
        }
        assert!(
            Instant::now() < startup_deadline,
            "headless browser did not create {}",
            devtools_path.display()
        );
        thread::sleep(Duration::from_millis(25));
    };
    let mut lines = active_port.lines();
    let port = lines
        .next()
        .and_then(|value| value.parse::<u16>().ok())
        .expect("DevTools active port");
    let websocket_path = lines.next().expect("DevTools browser websocket path");
    let (mut socket, _) = connect(format!("ws://127.0.0.1:{port}{websocket_path}"))
        .expect("connect to headless browser DevTools");
    if let MaybeTlsStream::Plain(stream) = socket.get_mut() {
        stream
            .set_read_timeout(Some(Duration::from_secs(15)))
            .expect("set DevTools response timeout");
    }

    let target_deadline = Instant::now() + Duration::from_secs(15);
    let mut next_id = 1;
    let target_id = loop {
        let response = cdp_command(&mut socket, next_id, "Target.getTargets", json!({}), None);
        next_id += 1;
        let target = response["result"]["targetInfos"]
            .as_array()
            .and_then(|targets| {
                targets.iter().find(|target| {
                    target["type"].as_str() == Some("page") && target["url"].as_str() == Some(url)
                })
            })
            .and_then(|target| target["targetId"].as_str())
            .map(str::to_owned);
        if let Some(target_id) = target {
            break target_id;
        }
        assert!(
            Instant::now() < target_deadline,
            "browser page target did not appear"
        );
        thread::sleep(Duration::from_millis(25));
    };
    let attach = cdp_command(
        &mut socket,
        next_id,
        "Target.attachToTarget",
        json!({ "targetId": target_id, "flatten": true }),
        None,
    );
    next_id += 1;
    let session_id = attach["result"]["sessionId"]
        .as_str()
        .expect("DevTools page session")
        .to_owned();
    cdp_command(
        &mut socket,
        next_id,
        "Runtime.enable",
        json!({}),
        Some(&session_id),
    );
    next_id += 1;

    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let response = cdp_command(
            &mut socket,
            next_id,
            "Runtime.evaluate",
            json!({
                "expression": "({ state: document.body?.dataset?.replayState ?? null, html: document.documentElement?.outerHTML ?? '' })",
                "returnByValue": true,
                "awaitPromise": true,
            }),
            Some(&session_id),
        );
        next_id += 1;
        let value = &response["result"]["result"]["value"];
        let state = value["state"].as_str();
        let dom = value["html"].as_str().unwrap_or_default().to_owned();
        if matches!(state, Some("complete" | "failed")) {
            return dom;
        }
        assert!(
            Instant::now() < deadline,
            "browser replay did not reach a terminal state; state={state:?}
{dom}"
        );
        thread::sleep(Duration::from_millis(50));
    }
}

fn body_contains(dom: &str, expected: &str, label: &str) {
    assert!(
        dom.contains(expected),
        "{label} DOM missing {expected:?}:
{dom}"
    );
}
#[test]
fn packaged_web_runtime_replays_jit_recording_and_reports_divergence_and_corruption() {
    let Some(browser) = browser_executable() else {
        eprintln!(
            "Web replay browser acceptance skipped: set STASIS_WEB_BROWSER to Chrome or Edge"
        );
        return;
    };
    let tree = TestTree::new();
    let workspace = &tree.0;
    fs::create_dir_all(workspace.join("src")).expect("create source directory");
    fs::create_dir_all(workspace.join("tests")).expect("create tests directory");
    fs::write(
        workspace.join("stasis.json"),
        r#"{"manifest_version":1,"name":"web_replay_acceptance","entry":"src/main.stasis","tests":"tests","output":"build","stdlib":"toolchain","web":{"replay":true}}"#,
    )
    .expect("write workspace manifest");
    fs::write(
        workspace.join("src/main.stasis"),
        "import \"/.stasis_cache/toolchain/src/stdlib/host_frame.stasis\";\n\
         global score: i32;\n\
         global host_i32: i32[768];\n\
         global host_f32: f32[64];\n\
         global replay_host_frame: HostFrame;\n\
         function main(): i32 { score = 7; return 0; }\n\
         function tick(): i32 { replay_host_frame.refresh(); score += 1; return 0; }\n\
         function render(): i32 { return 0; }\n\
         function on_code_swap(): void { return; }\n",
    )
    .expect("write JIT and Web fixture source");

    let prepared = Command::new(env!("CARGO_BIN_EXE_stasis"))
        .args(["--json", "prepare"])
        .current_dir(workspace)
        .output()
        .expect("prepare replay test workspace");
    assert!(
        prepared.status.success(),
        "replay workspace prepare failed: stdout={} stderr={}",
        String::from_utf8_lossy(&prepared.stdout),
        String::from_utf8_lossy(&prepared.stderr)
    );

    let replay_path = workspace.join("build/jit-recorded.replay.json");
    fs::create_dir_all(replay_path.parent().expect("recording parent"))
        .expect("create replay output directory");
    let record = Command::new(env!("CARGO_BIN_EXE_stasis"))
        .args([
            "play",
            "src/main.stasis",
            "--watch-dir",
            workspace.to_str().expect("workspace path"),
            "--record-replay",
            replay_path.to_str().expect("replay output path"),
            "--ticks",
            "257",
            "--tick-sleep-us",
            "0",
        ])
        .current_dir(workspace)
        .output()
        .expect("record JIT replay");
    assert!(
        record.status.success(),
        "JIT replay recording failed: stdout={} stderr={}",
        String::from_utf8_lossy(&record.stdout),
        String::from_utf8_lossy(&record.stderr)
    );
    let mut replay: Value =
        serde_json::from_slice(&fs::read(&replay_path).expect("read JIT-produced replay"))
            .expect("parse JIT-produced replay");
    assert_eq!(replay["schema_version"], 3, "producer must emit schema v3");
    assert_eq!(replay["total_ticks"], 257);
    assert_eq!(replay["checkpoints"].as_array().map(Vec::len), Some(1));
    assert!(
        replay["initial_state"]["values"]
            .as_array()
            .is_some_and(|values| !values.is_empty()),
        "fixture must record a non-default initial score"
    );

    let package = workspace.join("build/web");
    let packaged = Command::new(env!("CARGO_BIN_EXE_stasis"))
        .args([
            "package",
            "--workspace",
            workspace.to_str().expect("workspace path"),
            "--target",
            "web",
            "--out",
            "build/web",
            "--json",
        ])
        .current_dir(workspace)
        .output()
        .expect("package replay-enabled Web game");
    assert!(
        packaged.status.success(),
        "Web packaging failed: stdout={} stderr={}",
        String::from_utf8_lossy(&packaged.stdout),
        String::from_utf8_lossy(&packaged.stderr)
    );
    let config = fs::read_to_string(package.join("game.js")).expect("read packaged game runtime");
    let config = config
        .strip_prefix("window.STASIS_GAME = ")
        .and_then(|source| source.split_once(";\n").map(|(json, _)| json))
        .expect("packaged runtime metadata prefix");
    let config: Value = serde_json::from_str(config).expect("parse packaged runtime metadata");
    assert!(config["replayIdentity"]["compatibility"].is_object());
    assert!(config["replayCompatibility"]["state_snapshot"].is_object());
    for field in ["observed_i32", "observed_f32"] {
        assert_eq!(
            replay["identity"]["compatibility"][field],
            config["replayIdentity"]["compatibility"][field],
            "JIT replay and Web package {field} descriptors must match",
        );
    }

    fs::write(
        package.join("jit-recorded.replay.json"),
        fs::read(&replay_path).unwrap(),
    )
    .expect("publish replay beside Web package");

    let divergence_path = package.join("diverged.replay.json");
    let checkpoint_hash = replay["checkpoints"][0]["state_sha256"]
        .as_str()
        .expect("recorded checkpoint hash");
    let replacement_hash = if checkpoint_hash == "0".repeat(64) {
        "f".repeat(64)
    } else {
        "0".repeat(64)
    };
    replay["checkpoints"][0]["state_sha256"] = Value::String(replacement_hash);
    fs::write(
        &divergence_path,
        serde_json::to_vec(&replay).expect("serialize altered checkpoint replay"),
    )
    .expect("write altered checkpoint replay");

    let corrupt_path = package.join("corrupt.replay.json");
    let mut corrupt = serde_json::from_slice::<Value>(
        &fs::read(&replay_path).expect("read baseline replay for corruption"),
    )
    .expect("parse baseline replay for corruption");
    let duplicate = corrupt["initial_state"]["values"][0].clone();
    corrupt["initial_state"]["values"]
        .as_array_mut()
        .expect("initial sparse state values")
        .push(duplicate);
    fs::write(
        &corrupt_path,
        serde_json::to_vec(&corrupt).expect("serialize corrupted initial-state replay"),
    )
    .expect("write corrupted replay");

    let server = StaticServer::start(&package);
    let baseline = run_browser(
        &browser,
        &workspace.join("chrome-baseline"),
        &server.url("jit-recorded.replay.json"),
    );
    body_contains(
        &baseline,
        "data-replay-state=\"complete\"",
        "recorded replay",
    );
    body_contains(
        &baseline,
        "data-replay-tick=\"257\"",
        "recorded replay final tick",
    );
    body_contains(
        &baseline,
        "data-host-tick=\"257\"",
        "recorded replay final host tick",
    );

    let diverged = run_browser(
        &browser,
        &workspace.join("chrome-diverged"),
        &server.url("diverged.replay.json"),
    );
    body_contains(
        &diverged,
        "data-replay-state=\"failed\"",
        "altered checkpoint",
    );
    body_contains(
        &diverged,
        "data-host-tick=\"256\"",
        "altered checkpoint detection tick",
    );
    body_contains(
        &diverged,
        "replay diverged within ticks 1..=256",
        "altered checkpoint divergence",
    );

    let corrupted = run_browser(
        &browser,
        &workspace.join("chrome-corrupt"),
        &server.url("corrupt.replay.json"),
    );
    body_contains(
        &corrupted,
        "data-replay-state=\"failed\"",
        "corrupt sparse state",
    );
    body_contains(
        &corrupted,
        "initial state entries must be sorted and unique",
        "corrupt sparse-state diagnostic",
    );
}
