use super::*;
use std::io::Write;
use std::net::TcpListener;

// Optional local evidence uses the real project's saved session without writing it.
#[test]
fn desktop_editor_initial_http_payload_uses_the_real_dispatch_path() {
    let project = std::env::var_os("STASIS_EDITOR_PAYLOAD_PROJECT").map(PathBuf::from);
    let root = project
        .clone()
        .unwrap_or_else(|| super::super::tests::desktop_editor_fixture("initial_http_payload"));
    let mut session = if project.is_some() {
        let saved: Value = serde_json::from_slice(
            &std::fs::read(root.join(".stasis/editor/session.json")).unwrap(),
        )
        .unwrap();
        serde_json::from_value::<TaskSession>(saved["snapshot"]["session"].clone()).unwrap()
    } else {
        let mut session = TaskSession::new();
        session.new_task("one", "Inspect value", "Project").unwrap();
        session
    };
    session.active_task_mut().unwrap().connection = ConnectionState::Connected;
    let sources = super::super::desktop_source_context(&root).unwrap();
    let symbol_count = sources.len();
    let expected_catalog = ProposalTools {
        sources,
        ..ProposalTools::default()
    }
    .source_catalog()
    .unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let mut config = if project.is_some() {
        stasis_ai::OpenRouterConfig::from_workspace(&root).unwrap()
    } else {
        stasis_ai::OpenRouterConfig {
            api_key: String::new(),
            base_url: String::new(),
            model: stasis_ai::DEFAULT_OPENROUTER_MODEL.into(),
            approved_models: vec![stasis_ai::DEFAULT_OPENROUTER_MODEL.into()].into_boxed_slice(),
            routing: stasis_ai::RoutingConfig::default(),
            timeout: Duration::from_secs(10),
        }
    };
    config.base_url = endpoint;
    config.api_key = "local-capture-only".into();
    let server = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "no desktop HTTP request arrived");
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("capture listener: {error}"),
            }
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut reader = std::io::BufReader::new(&mut stream);
        let mut length = None;
        loop {
            let mut line = String::new();
            assert!(std::io::BufRead::read_line(&mut reader, &mut line).unwrap() > 0);
            if line == "\r\n" {
                break;
            }
            if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                length = Some(value.trim().parse::<usize>().unwrap());
            }
        }
        let length = length.expect("HTTP body length");
        assert!(length <= 2 * 1024 * 1024);
        let mut body = vec![0; length];
        reader.read_exact(&mut body).unwrap();
        drop(reader);
        let reply = json!({"mode":"done", "working_notes":"Captured locally.", "summary":"Capture complete."});
        let event = json!({"choices":[{"delta":{"content":reply.to_string()}}]});
        let response = format!("data: {event}\n\ndata: [DONE]\n\n");
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}", response.len()).unwrap();
        body
    });
    let provider_root = root.clone();
    let (sent, received) = mpsc::channel();
    let controller = TaskController::new(move |request, canceled| {
        let result = run_reply_provider_with_config(
            request,
            canceled,
            provider_root.clone(),
            None,
            ProviderConfig::OpenRouter(config.clone()),
            |_| {},
        );
        sent.send(result.clone()).unwrap();
        result
    });
    controller.send_active(&mut session).unwrap();
    let reply = received
        .recv_timeout(Duration::from_secs(30))
        .unwrap()
        .unwrap();
    assert_eq!(reply.text, "Capture complete.");
    let bytes = server.join().unwrap();
    assert!(bytes.len() <= 10_000, "initial HTTP body exceeds 10 KB");
    if let Ok(limit) = std::env::var("STASIS_EDITOR_PAYLOAD_MAX_BYTES") {
        assert!(
            bytes.len() < limit.parse::<usize>().unwrap(),
            "HTTP body exceeds payload budget"
        );
    }
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    let content = body["messages"][0]["content"].as_str().unwrap();
    assert!(content.starts_with("request:1\nrole:Stasis desktop task assistant\n"));
    assert!(content.contains("\ntools:\n"));
    assert!(content.contains("\tinspect_source\tselector\t"));
    assert!(content.ends_with(expected_catalog.as_str().unwrap()));
    assert!(expected_catalog.as_str().unwrap().len() <= 5_000);
    assert_eq!(body["response_format"]["type"], "json_schema");
    if let Some(output) = std::env::var_os("STASIS_EDITOR_PAYLOAD_OUTPUT") {
        let output = PathBuf::from(output);
        std::fs::create_dir_all(&output).unwrap();
        std::fs::write(output.join("http-body.exact.json"), &bytes).unwrap();
        std::fs::write(output.join("model-message.exact.jsonl"), content).unwrap();
        std::fs::write(
            output.join("catalog.txt"),
            expected_catalog.as_str().unwrap(),
        )
        .unwrap();
    }
    eprintln!(
        "desktop HTTP body={} bytes; model message={} bytes; symbols={}",
        bytes.len(),
        content.len(),
        symbol_count
    );
    if project.is_none() {
        std::fs::remove_dir_all(root).unwrap();
    }
}
