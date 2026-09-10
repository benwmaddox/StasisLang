use base64::Engine as _;
use sha2::{Digest, Sha256};
use stasis_ai::task_session::{ActionState, ActivityEntry, ActivityKind, Task, ThreadEntryKind};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const MAX_EMBEDDED_MEDIA_BYTES: usize = 64 * 1024 * 1024;
const MAX_SINGLE_MEDIA_BYTES: usize = 16 * 1024 * 1024;

pub(super) type SemanticDiffs = BTreeMap<(String, usize), String>;

pub(super) struct ExportResult {
    pub html: String,
    pub embedded_media: usize,
    pub omitted_media: usize,
}

pub(super) fn default_file_name(task: &Task) -> String {
    let slug = task
        .objective
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .take(8)
        .collect::<Vec<_>>()
        .join("-");
    format!(
        "{}.html",
        if slug.is_empty() {
            "stasis-chat"
        } else {
            &slug
        }
    )
}

pub(super) fn auto_file_name(task: &Task) -> String {
    let digest = format!("{:x}", Sha256::digest(task.id.as_str().as_bytes()));
    let name = default_file_name(task);
    format!("{}-{}", &digest[..12], name)
}

pub(super) fn content_fingerprint(
    task: &Task,
    diffs: &SemanticDiffs,
    media_hashes: &BTreeMap<String, String>,
    unavailable_media: &BTreeSet<String>,
) -> String {
    let mut hash = Sha256::new();
    hash.update(serde_json::to_vec(task).expect("task serializes"));
    for ((action, revision), diff) in diffs {
        hash.update(action.as_bytes());
        hash.update(revision.to_le_bytes());
        hash.update(diff.as_bytes());
    }
    for source in task
        .screenshots
        .values()
        .map(|image| image.source.as_str())
        .chain(
            task.generated_images
                .values()
                .map(|image| image.source.as_str()),
        )
    {
        hash.update(source.as_bytes());
        if let Some(media_hash) = media_hashes.get(source) {
            hash.update(media_hash.as_bytes());
        }
        hash.update([u8::from(unavailable_media.contains(source))]);
    }
    format!("{:x}", hash.finalize())
}

pub(super) fn write(path: &Path, html: &str) -> Result<(), String> {
    if !path
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("html"))
    {
        return Err("chat export path must end in .html".into());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("could not create {}: {error}", parent.display()))?;
    }
    let mut file = atomic_write_file::AtomicWriteFile::open(path)
        .map_err(|error| format!("could not open {}: {error}", path.display()))?;
    file.write_all(html.as_bytes())
        .map_err(|error| format!("could not write {}: {error}", path.display()))?;
    file.commit()
        .map_err(|error| format!("could not commit {}: {error}", path.display()))
}

pub(super) fn render(
    task: &Task,
    diffs: &SemanticDiffs,
    media_hashes: &BTreeMap<String, String>,
    unavailable_media: &BTreeSet<String>,
) -> ExportResult {
    let activity = task.activity_timeline();
    let latest_actions = latest_sequences(&activity, |kind| match kind {
        ActivityKind::SemanticAction { action_id, .. } => Some(action_id.to_string()),
        _ => None,
    });
    let latest_images = latest_sequences(&activity, |kind| match kind {
        ActivityKind::GeneratedAsset { image_id, .. } => Some(image_id.to_string()),
        _ => None,
    });
    let mut body = String::new();
    let mut remaining_media = MAX_EMBEDDED_MEDIA_BYTES;
    let mut embedded_media = 0;
    let mut omitted_media = 0;

    for entry in &activity {
        let timestamp = timestamp_html(entry);
        let hover = hover_details(entry);
        match &entry.kind {
            ActivityKind::UserMessage { thread_sequence } => {
                if let Some(text) = thread_text(task, *thread_sequence, ThreadEntryKind::Reply) {
                    message(&mut body, "user", "User", timestamp, hover, text, "");
                }
            }
            ActivityKind::AiReply { thread_sequence } => {
                if let Some(text) = thread_text(task, *thread_sequence, ThreadEntryKind::Result) {
                    message(&mut body, "agent", "Agent", timestamp, hover, text, "");
                }
            }
            ActivityKind::HostResult { thread_sequence } => {
                if let Some(text) = thread_text(task, *thread_sequence, ThreadEntryKind::HostResult)
                {
                    message(&mut body, "host", "Stasis", timestamp, hover, text, "");
                }
            }
            ActivityKind::Attachment {
                screenshot_id,
                upload,
                analysis,
            } => {
                let Some(screenshot) = task.screenshots.get(screenshot_id) else {
                    continue;
                };
                let expected = screenshot.content_sha256.as_deref();
                let media = media_html(
                    &screenshot.source,
                    expected,
                    unavailable_media,
                    &mut remaining_media,
                );
                if media.embedded {
                    embedded_media += 1
                } else {
                    omitted_media += 1
                }
                let extra = format!(
                    "<div class=\"artifact-status\">Upload: {} · Analysis: {}</div>{}",
                    escape(&format!("{upload:?}").to_ascii_lowercase()),
                    escape(&format!("{analysis:?}").to_ascii_lowercase()),
                    media.html
                );
                message(
                    &mut body,
                    "artifact",
                    "Picture",
                    timestamp,
                    hover,
                    &screenshot.source,
                    &extra,
                );
            }
            ActivityKind::SemanticAction {
                action_id,
                description,
                state,
                ..
            } => {
                let latest = latest_actions.get(action_id.as_str()) == Some(&entry.sequence);
                let mut extra = String::new();
                if latest {
                    if let Some(action) = task.actions.get(action_id) {
                        for proposal in super::semantic_revisions::proposal_revisions(action) {
                            let _ = write!(
                                extra,
                                "<section class=\"revision\"><h3>Revision {}</h3><p>{}</p><div class=\"artifact-status\">{}</div>",
                                proposal.revision + 1,
                                escape(proposal.description),
                                escape(&action_state(proposal.state))
                            );
                            if let Some(diff) =
                                diffs.get(&(action_id.to_string(), proposal.revision))
                            {
                                let _ = write!(extra, "<pre class=\"diff\">{}</pre>", escape(diff));
                            } else if proposal.payload.is_some() {
                                extra.push_str("<p class=\"unavailable\">Diff unavailable in this export snapshot.</p>");
                            }
                            extra.push_str("</section>");
                        }
                    }
                }
                let summary = format!("{} · {}", action_id, action_state(state));
                message(
                    &mut body,
                    "artifact",
                    "Proposed change",
                    timestamp,
                    hover,
                    &format!("{description}\n{summary}"),
                    &extra,
                );
            }
            ActivityKind::GeneratedAsset {
                image_id,
                review,
                handoff,
            } => {
                if latest_images.get(image_id.as_str()) != Some(&entry.sequence) {
                    continue;
                }
                let Some(image) = task.generated_images.get(image_id) else {
                    continue;
                };
                let media = media_html(
                    &image.source,
                    media_hashes.get(&image.source).map(String::as_str),
                    unavailable_media,
                    &mut remaining_media,
                );
                if media.embedded {
                    embedded_media += 1
                } else {
                    omitted_media += 1
                }
                let extra = format!(
                    "<div class=\"artifact-status\">Review: {} · Handoff: {} · {} / {}</div>{}",
                    escape(&format!("{review:?}").to_ascii_lowercase()),
                    escape(&format!("{handoff:?}").to_ascii_lowercase()),
                    escape(&image.attribution.provider),
                    escape(
                        image
                            .attribution
                            .model
                            .as_deref()
                            .unwrap_or("model unavailable")
                    ),
                    media.html
                );
                message(
                    &mut body,
                    "artifact",
                    "Generated picture",
                    timestamp,
                    hover,
                    &image.source,
                    &extra,
                );
            }
            ActivityKind::FocusedTest { run_id, status } => {
                message(
                    &mut body,
                    "host",
                    "Focused test",
                    timestamp,
                    hover,
                    &format!("Run {run_id}: {status:?}"),
                    "",
                );
            }
        }
    }

    let total_tokens = task
        .metrics
        .input_tokens
        .saturating_add(task.metrics.output_tokens);
    let html = format!(
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; img-src data:; style-src 'unsafe-inline'; script-src 'unsafe-inline'\"><title>{title} · Stasis chat</title><style>{css}</style></head><body><main><header><div class=\"eyebrow\">Stasis task chat</div><h1>{title}</h1><p>{project}</p><div class=\"summary\"><span>{turns} AI turn{turn_suffix}</span><span>{tokens} tokens</span><span>${cost:.4}</span><span>{elapsed}</span></div><p class=\"privacy\">Local export snapshot. Images are embedded only when their saved integrity hash matches.</p></header><section class=\"timeline\">{body}</section><footer>Exported from Stasis · Hover or focus a message for timing and usage details.</footer></main><script>{script}</script></body></html>\n",
        title = escape(&task.objective),
        project = escape(&task.project_summary),
        turns = task.metrics.turn_count,
        turn_suffix = if task.metrics.turn_count == 1 { "" } else { "s" },
        tokens = total_tokens,
        cost = task.metrics.estimated_cost_micros as f64 / 1_000_000.0,
        elapsed = format_duration(task.metrics.elapsed_ms),
        css = CSS,
        script = SCRIPT,
    );
    ExportResult {
        html,
        embedded_media,
        omitted_media,
    }
}

fn latest_sequences(
    entries: &[ActivityEntry],
    key: impl Fn(&ActivityKind) -> Option<String>,
) -> BTreeMap<String, u64> {
    entries
        .iter()
        .filter_map(|entry| key(&entry.kind).map(|key| (key, entry.sequence)))
        .collect()
}

fn thread_text(task: &Task, sequence: u64, kind: ThreadEntryKind) -> Option<&str> {
    task.thread
        .iter()
        .find(|entry| entry.sequence == sequence && entry.kind == kind)
        .map(|entry| entry.text.as_str())
}

fn message(
    body: &mut String,
    class: &str,
    role: &str,
    timestamp: String,
    hover: String,
    text: &str,
    extra: &str,
) {
    let _ = write!(
        body,
        "<article class=\"message {class}\" tabindex=\"0\"><div class=\"message-head\"><strong>{}</strong>{timestamp}</div><div class=\"text\">{}</div>{extra}<div class=\"hover-card\">{hover}</div></article>",
        escape(role),
        escape(text),
    );
}

fn timestamp_html(entry: &ActivityEntry) -> String {
    entry.recorded_at_unix_ms.map_or_else(
        || "<time>Time unavailable</time>".into(),
        |value| format!("<time data-unix-ms=\"{value}\">{value}</time>"),
    )
}

fn hover_details(entry: &ActivityEntry) -> String {
    let mut lines = vec![format!("Activity #{}", entry.sequence)];
    match entry.recorded_at_unix_ms {
        Some(value) => lines.push(format!("Timestamp: {value} ms since Unix epoch")),
        None => lines.push("Timestamp: unavailable for legacy history".into()),
    }
    if let Some(turn) = &entry.provider_turn {
        lines.push(format!("Time taken: {}", format_duration(turn.elapsed_ms)));
        lines.push(format!(
            "Tokens: {} in / {} out",
            turn.input_tokens, turn.output_tokens
        ));
        lines.push(format!(
            "Cost: ${:.6}",
            turn.estimated_cost_micros as f64 / 1_000_000.0
        ));
        if let Some(provider) = &turn.provider {
            lines.push(format!("Provider: {provider}"));
        }
        if let Some(model) = &turn.model {
            lines.push(format!("Model: {model}"));
        }
        if let Some(route) = &turn.route {
            lines.push(format!("Route: {route}"));
        }
    }
    lines
        .into_iter()
        .map(|line| format!("<div>{}</div>", escape(&line)))
        .collect()
}

struct MediaHtml {
    html: String,
    embedded: bool,
}

fn media_html(
    source: &str,
    expected: Option<&str>,
    unavailable: &BTreeSet<String>,
    remaining: &mut usize,
) -> MediaHtml {
    if unavailable.contains(source) {
        return omitted(
            "Picture unavailable: it failed integrity verification when the task was restored.",
        );
    }
    let Some(expected) = expected else {
        return omitted("Picture omitted: no saved integrity hash is available.");
    };
    let path = PathBuf::from(source);
    let read_limit = MAX_SINGLE_MEDIA_BYTES.min(*remaining);
    if read_limit == 0 {
        return omitted("Picture omitted: the export's 64 MiB embedded-media limit was reached.");
    }
    let bytes = read_bounded(&path, read_limit);
    let Ok(bytes) = bytes else {
        return omitted(&format!("Picture omitted: {}", bytes.unwrap_err()));
    };
    let actual = format!("{:x}", Sha256::digest(&bytes));
    if actual != expected {
        return omitted("Picture omitted: its contents changed after capture.");
    }
    let Some(mime) = image_mime(&bytes) else {
        return omitted("Picture omitted: only verified PNG and JPEG data can be embedded.");
    };
    *remaining = remaining.saturating_sub(bytes.len());
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    MediaHtml {
        html: format!("<figure><img loading=\"lazy\" src=\"data:{mime};base64,{encoded}\" alt=\"Exported task picture\"><figcaption>{}</figcaption></figure>", escape(source)),
        embedded: true,
    }
}

fn omitted(reason: &str) -> MediaHtml {
    MediaHtml {
        html: format!("<p class=\"unavailable\">{}</p>", escape(reason)),
        embedded: false,
    }
}

fn read_bounded(path: &Path, limit: usize) -> Result<Vec<u8>, String> {
    let file =
        File::open(path).map_err(|error| format!("could not read {} ({error})", path.display()))?;
    let mut bytes = Vec::new();
    file.take((limit + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("could not read {} ({error})", path.display()))?;
    if bytes.len() > limit {
        return Err(format!(
            "{} exceeds the remaining media limit",
            path.display()
        ));
    }
    Ok(bytes)
}

fn image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else {
        None
    }
}

fn action_state(state: &ActionState) -> String {
    match state {
        ActionState::Proposed => "proposed".into(),
        ActionState::Accepted => "accepted".into(),
        ActionState::Applied => "applied".into(),
        ActionState::Rejected { reason } => format!("rejected: {reason}"),
        ActionState::NeedsRepair { reason } => format!("needs repair: {reason}"),
    }
}

fn format_duration(milliseconds: u64) -> String {
    if milliseconds < 1_000 {
        format!("{milliseconds} ms")
    } else {
        format!("{:.2} s", milliseconds as f64 / 1_000.0)
    }
}

fn escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

const CSS: &str = r#"
:root{color-scheme:dark;--bg:#12161c;--panel:#1c222b;--line:#343d49;--muted:#9eabb9;--text:#eaf0f6;--accent:#8ed7c5;--user:#183c48;--agent:#202832;--artifact:#252535}*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--text);font:15px/1.55 system-ui,-apple-system,"Segoe UI",sans-serif}main{width:min(940px,calc(100% - 32px));margin:40px auto 80px}header{border:1px solid var(--line);background:var(--panel);padding:28px;border-radius:16px}h1{margin:.15em 0;font-size:clamp(24px,5vw,38px);line-height:1.15}.eyebrow,.privacy,time,footer,.artifact-status{color:var(--muted);font-size:12px}.summary{display:flex;flex-wrap:wrap;gap:8px;margin-top:18px}.summary span{border:1px solid var(--line);border-radius:999px;padding:5px 10px}.timeline{display:flex;flex-direction:column;gap:14px;margin-top:22px}.message{position:relative;width:min(84%,760px);border:1px solid var(--line);border-radius:14px;padding:15px 17px;background:var(--agent);outline:none}.message.user{align-self:flex-end;background:var(--user)}.message.host,.message.artifact{align-self:center;width:min(94%,840px)}.message.artifact{background:var(--artifact)}.message-head{display:flex;justify-content:space-between;gap:20px;margin-bottom:7px}.text{white-space:pre-wrap;overflow-wrap:anywhere}.hover-card{position:absolute;z-index:5;right:12px;top:38px;max-width:min(360px,90%);padding:10px 12px;border:1px solid #607083;border-radius:9px;background:#0c1015;box-shadow:0 8px 24px #0009;color:#dfe8f0;font-size:12px;opacity:0;transform:translateY(-4px);pointer-events:none;transition:.12s}.message:hover .hover-card,.message:focus .hover-card{opacity:1;transform:none}.revision{margin-top:14px;padding-top:12px;border-top:1px solid var(--line)}.revision h3{margin:0}.diff{max-height:520px;overflow:auto;padding:13px;border-radius:9px;background:#0d1117;color:#d9e2ea;font:12px/1.45 ui-monospace,SFMono-Regular,Consolas,monospace;white-space:pre}figure{margin:12px 0 0}img{display:block;max-width:100%;max-height:620px;border-radius:9px;border:1px solid var(--line)}figcaption{margin-top:5px;color:var(--muted);font-size:11px;overflow-wrap:anywhere}.unavailable{padding:9px;border-left:3px solid #d19b61;background:#30271f}footer{text-align:center;margin-top:28px}@media(max-width:620px){main{width:min(100% - 18px,940px);margin-top:9px}.message,.message.host,.message.artifact{width:100%}header{padding:20px}.hover-card{position:static;display:none;margin-top:10px}.message:focus .hover-card{display:block}}
"#;

const SCRIPT: &str = r#"document.querySelectorAll('time[data-unix-ms]').forEach(function(node){var date=new Date(Number(node.dataset.unixMs));node.textContent=date.toLocaleString();node.dateTime=date.toISOString();});"#;

#[cfg(test)]
mod tests {
    use super::*;
    use stasis_ai::task_session::{ProviderState, RoutingState};

    #[test]
    fn export_escapes_conversation_and_includes_turn_hover_details() {
        let mut task = Task::new("task", "Fix <canvas>", "A & B").unwrap();
        task.append_user_message("Use <script>alert('no')</script>")
            .unwrap();
        task.append_result("Done & safe").unwrap();
        task.set_provider_state(ProviderState {
            provider: Some("openrouter".into()),
            model: Some("model/x".into()),
            routing: RoutingState::Assigned {
                route: "price".into(),
            },
            ..ProviderState::default()
        })
        .unwrap();
        task.record_turn(1_840, 2_410, 386, 1_200).unwrap();
        let export = render(
            &task,
            &SemanticDiffs::new(),
            &BTreeMap::new(),
            &BTreeSet::new(),
        );
        assert!(export.html.contains("Fix &lt;canvas&gt;"));
        assert!(export
            .html
            .contains("&lt;script&gt;alert(&#39;no&#39;)&lt;/script&gt;"));
        assert!(!export.html.contains("<script>alert('no')</script>"));
        assert!(export.html.contains("Time taken: 1.84 s"));
        assert!(export.html.contains("Tokens: 2410 in / 386 out"));
        assert!(export.html.contains("Cost: $0.001200"));
        assert!(export.html.contains("Provider: openrouter"));
        assert!(export.html.contains("Model: model/x"));
    }

    #[test]
    fn legacy_activity_reports_unavailable_timestamp() {
        let mut task = Task::new("task", "Legacy", "Fixture").unwrap();
        task.thread.push(stasis_ai::ThreadEntry {
            sequence: 1,
            kind: ThreadEntryKind::Reply,
            text: "old".into(),
        });
        let export = render(
            &task,
            &SemanticDiffs::new(),
            &BTreeMap::new(),
            &BTreeSet::new(),
        );
        assert!(export.html.contains("Time unavailable"));
        assert!(export
            .html
            .contains("Timestamp: unavailable for legacy history"));
    }

    #[test]
    fn verified_png_is_embedded_and_changed_media_is_omitted() {
        let bytes = b"\x89PNG\r\n\x1a\nfixture";
        let path =
            std::env::temp_dir().join(format!("stasis-chat-export-{}.png", std::process::id()));
        std::fs::write(&path, bytes).unwrap();
        let hash = format!("{:x}", Sha256::digest(bytes));
        let mut task = Task::new("task", "Picture", "Fixture").unwrap();
        task.vision_capability = stasis_ai::VisionCapability::Available;
        task.attach_screenshot_with_sha256("shot", path.to_string_lossy(), hash.clone())
            .unwrap();
        let export = render(
            &task,
            &SemanticDiffs::new(),
            &BTreeMap::new(),
            &BTreeSet::new(),
        );
        assert_eq!(export.embedded_media, 1);
        assert!(export.html.contains("data:image/png;base64,"));
        std::fs::write(&path, b"\x89PNG\r\n\x1a\nchanged").unwrap();
        let changed = render(
            &task,
            &SemanticDiffs::new(),
            &BTreeMap::new(),
            &BTreeSet::new(),
        );
        assert_eq!(changed.omitted_media, 1);
        assert!(changed.html.contains("contents changed after capture"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn semantic_diff_is_included_only_as_escaped_text() {
        let mut task = Task::new("task", "Change source", "Fixture").unwrap();
        task.propose_action_with_payload(
            "edit-main",
            stasis_ai::ActionKind::Edit,
            "Change main",
            serde_json::json!({"schema_version": 1, "edits": []}),
        )
        .unwrap();
        let mut diffs = SemanticDiffs::new();
        diffs.insert(
            ("edit-main".into(), 0),
            "diff --git a/main.stasis b/main.stasis\n+<img src=x onerror=alert(1)>\n".into(),
        );
        let export = render(&task, &diffs, &BTreeMap::new(), &BTreeSet::new());
        assert!(export.html.contains("diff --git a/main.stasis"));
        assert!(export.html.contains("+&lt;img src=x onerror=alert(1)&gt;"));
        assert!(!export.html.contains("<img src=x onerror=alert(1)>"));
    }

    #[test]
    fn capture_chat_export_evidence() {
        let Some(output) = std::env::var_os("STASIS_CHAT_EXPORT_EVIDENCE_HTML") else {
            return;
        };
        let output = PathBuf::from(output);
        let image_path = output.with_file_name("chat-export-picture.png");
        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let mut image = image::RgbaImage::new(720, 300);
        for (x, y, pixel) in image.enumerate_pixels_mut() {
            let tile = ((x / 48) + (y / 48)) % 2;
            let glow = u8::try_from((x * 80 / 720).min(80)).unwrap();
            *pixel = if tile == 0 {
                image::Rgba([26, 75 + glow, 98 + glow, 255])
            } else {
                image::Rgba([39, 45 + glow / 2, 70 + glow, 255])
            };
        }
        image.save(&image_path).unwrap();
        let image_bytes = std::fs::read(&image_path).unwrap();
        let image_hash = format!("{:x}", Sha256::digest(&image_bytes));

        let mut task = Task::new(
            "evidence",
            "Make the arena feel faster and easier to read",
            "Brickout Revenge · local editor session",
        )
        .unwrap();
        task.append_user_message(
            "Tighten paddle response and brighten the playfield. Keep the existing score behavior.",
        )
        .unwrap();
        task.append_result("I found the movement and palette owners. I prepared one atomic source change with a focused behavior test.").unwrap();
        task.set_provider_state(ProviderState {
            provider: Some("openrouter".into()),
            model: Some("openai/gpt-oss-120b".into()),
            routing: RoutingState::Assigned {
                route: "price".into(),
            },
            ..ProviderState::default()
        })
        .unwrap();
        task.record_turn(1_840, 2_410, 386, 1_200).unwrap();
        task.set_vision_capability(true).unwrap();
        task.attach_screenshot_with_sha256("arena", image_path.to_string_lossy(), image_hash)
            .unwrap();
        task.propose_action_with_payload(
            "tune-arena",
            stasis_ai::ActionKind::Edit,
            "Tune paddle speed and arena colors",
            serde_json::json!({"schema_version": 1, "edits": []}),
        )
        .unwrap();
        let mut diffs = SemanticDiffs::new();
        diffs.insert(("tune-arena".into(), 0), "diff --git a/src/game.stasis b/src/game.stasis\n--- a/src/game.stasis\n+++ b/src/game.stasis\n@@ -8,2 +8,2 @@\n-global paddle_speed: f32 = 6.0\n+global paddle_speed: f32 = 8.5\n-global arena_glow: Color = rgb(24, 70, 92)\n+global arena_glow: Color = rgb(40, 145, 176)\n".into());
        let export = render(&task, &diffs, &BTreeMap::new(), &BTreeSet::new());
        write(&output, &export.html).unwrap();
    }
}
