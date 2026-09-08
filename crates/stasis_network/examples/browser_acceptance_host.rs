use std::env;
use std::fs;
use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use stasis_network::{BundleFile, EventKind, HostOptions, NetworkHost, StaticBundle};

const TEST_SECRET: [u8; 32] = [
    0xa1, 0x7c, 0x9e, 0x24, 0x0d, 0x6b, 0x3f, 0x81, 0x52, 0xa4, 0x8c, 0x70, 0xe9, 0x3d, 0xb6, 0xf1,
    0xa1, 0x7c, 0x9e, 0x24, 0x0d, 0x6b, 0x3f, 0x81, 0x52, 0xa4, 0x8c, 0x70, 0xe9, 0x3d, 0xb6, 0xf1,
];
const TIMEOUT: Duration = Duration::from_secs(30);

const INDEX_HTML: &str = r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8"><title>Stasis network acceptance</title>
  <style>
    #acceptance-status { box-sizing: border-box; position: fixed; inset: 0; z-index: 1000; padding: 44px; color: #e8eefc; background: #101827; font: 20px/1.6 system-ui, sans-serif; }
    #acceptance-status h1 { margin-top: 0; font-size: 26px; }
  </style>
</head>
<body>
  <canvas id="stasis-canvas" width="64" height="64"></canvas>
  <div id="stasis-hud"></div>
  <div id="stasis-error"></div>
  <div id="stasis-loading"><span id="stasis-loading-status">Loading</span></div>
  <button id="audio-enable" type="button">Enable sound</button>
  <section id="acceptance-status"><h1>Native host browser acceptance</h1><ol id="acceptance-progress"></ol></section>
  <script>
    globalThis.STASIS_CHARACTERIZATION_TEST = true;
    window.STASIS_GAME = {
      strings: {},
      assets: {},
      memory: { payload: { hash: 7, offset: 0, length: 65536, stride: 1, byte_backed: true, type_id: 5 } }
    };
  </script>
  <script src="game.js"></script>
</body>
</html>
"#;

// A valid Wasm module exporting memory plus no-op main/tick/render functions.
const GAME_WASM: &[u8] = &[
    0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x05, 0x01, 0x60, 0x00, 0x01, 0x7f, 0x03,
    0x04, 0x03, 0x00, 0x00, 0x00, 0x05, 0x03, 0x01, 0x00, 0x01, 0x07, 0x21, 0x04, 0x06, 0x6d, 0x65,
    0x6d, 0x6f, 0x72, 0x79, 0x02, 0x00, 0x04, 0x6d, 0x61, 0x69, 0x6e, 0x00, 0x00, 0x04, 0x74, 0x69,
    0x63, 0x6b, 0x00, 0x01, 0x06, 0x72, 0x65, 0x6e, 0x64, 0x65, 0x72, 0x00, 0x02, 0x0a, 0x10, 0x03,
    0x04, 0x00, 0x41, 0x00, 0x0b, 0x04, 0x00, 0x41, 0x00, 0x0b, 0x04, 0x00, 0x41, 0x00, 0x0b,
];

fn main() -> Result<(), String> {
    let ready_file = required_path("--ready-file")?;
    let runtime_source = include_str!("../../../runtime/web/game.js");
    let runtime = runtime_source
        .replacen(
            "      networkClient,",
            "      networkClient,\n      networkTestMemory: () => instance?.exports.memory || null,",
            1,
        )
        .into_bytes();
    if runtime == runtime_source.as_bytes() {
        return Err("browser runtime characterization seam was not found".into());
    }
    let bundle = StaticBundle::new(vec![
        BundleFile {
            path: "index.html".into(),
            mime: "text/html; charset=utf-8".into(),
            bytes: INDEX_HTML.as_bytes().to_vec(),
        },
        BundleFile {
            path: "game.js".into(),
            mime: "text/javascript; charset=utf-8".into(),
            bytes: runtime,
        },
        BundleFile {
            path: "game.wasm".into(),
            mime: "application/wasm".into(),
            bytes: GAME_WASM.to_vec(),
        },
    ])
    .map_err(|error| error.to_string())?;
    let mut host = NetworkHost::bind_with_options(HostOptions {
        bind_addr: IpAddr::V4(Ipv4Addr::LOCALHOST),
        port: 0,
        bundle,
        expected_origin: None,
        session_secret: TEST_SECRET.to_vec(),
        max_connections: stasis_network::MAX_SEATS,
        max_buffered_bytes: stasis_network::MAX_BUFFERED_PAYLOAD,
    })
    .map_err(|error| format!("host bind failed: {error:?}"))?;
    fs::write(&ready_file, host.address().port().to_string())
        .map_err(|error| format!("ready file failed: {error}"))?;

    let deadline = Instant::now() + TIMEOUT;
    let mut handle = None;
    let mut phase = 0_u8;
    while Instant::now() < deadline {
        let Some(event) = host.poll() else {
            thread::sleep(Duration::from_millis(5));
            continue;
        };
        eprintln!(
            "acceptance event phase={phase} kind={:?} connection={}",
            event.kind, event.connection
        );
        match (phase, event.kind) {
            (0, EventKind::Connected) => {
                handle = Some(event.connection);
                send_json(&host, event.connection, br#"{"kind":"join_ack","seat":0}"#)?;
                phase = 1;
            }
            (1, EventKind::Message)
                if event.payload
                    == br#"{"kind":"guest_command","command":"move","sequence":1}"# =>
            {
                send_json(
                    &host,
                    event.connection,
                    br#"{"kind":"snapshot","tick":42,"world":{"players":1}}"#,
                )?;
                send_json(
                    &host,
                    event.connection,
                    br#"{"kind":"native_command","command":"wave"}"#,
                )?;
                phase = 2;
            }
            (2, EventKind::Message)
                if event.payload == br#"{"kind":"command_ack","command":"wave"}"# =>
            {
                phase = 3;
            }
            (3, EventKind::Disconnected) if handle == Some(event.connection) => {
                phase = 4;
            }
            (4, EventKind::Connected) if handle == Some(event.connection) => {
                send_json(
                    &host,
                    event.connection,
                    br#"{"kind":"reconnect_ack","resumed":true}"#,
                )?;
                phase = 5;
            }
            (5, EventKind::Message) if event.payload == br#"{"kind":"acceptance_complete"}"# => {
                host.stop()
                    .map_err(|error| format!("host stop failed: {error:?}"))?;
                println!("BROWSER_NETWORK_ACCEPTANCE_OK");
                return Ok(());
            }
            (_, EventKind::Overflow) => {
                return Err(format!(
                    "network host rejected acceptance traffic in phase {phase}"
                ));
            }
            (_, EventKind::Rejected) if handle == Some(event.connection) => {
                return Err(format!(
                    "network host rejected acceptance traffic in phase {phase}"
                ));
            }
            _ => {}
        }
    }
    Err(format!("network acceptance timed out in phase {phase}"))
}

fn required_path(flag: &str) -> Result<PathBuf, String> {
    let mut args = env::args_os().skip(1);
    while let Some(argument) = args.next() {
        if argument == flag {
            return args
                .next()
                .map(PathBuf::from)
                .ok_or_else(|| format!("{flag} requires a path"));
        }
    }
    Err(format!("missing {flag}"))
}

fn send_json(host: &NetworkHost, connection: u32, payload: &[u8]) -> Result<(), String> {
    host.send(connection, payload)
        .map_err(|error| format!("host send failed: {error:?}"))
}
