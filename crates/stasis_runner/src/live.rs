use crossbeam_channel::{bounded, Receiver, Sender, TryRecvError, TrySendError};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::time::Duration;

pub const LIVE_SCHEMA_VERSION: u16 = 1;
pub const DEFAULT_LIVE_QUEUE_CAPACITY: usize = 64;
pub const DEFAULT_LIVE_OUTPUT_BYTES: usize = 64 * 1024;
pub const MAX_LIVE_REQUEST_BYTES: usize = 512 * 1024;
pub const MAX_LIVE_MULTILINE_BYTES: usize = 256 * 1024;
pub const MAX_SCRATCH_CELLS: usize = 64;
pub const MAX_LIVE_WATCHES: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveRequest {
    #[serde(default = "live_schema_version")]
    pub schema_version: u16,
    pub request_id: u64,
    #[serde(flatten)]
    pub command: LiveCommand,
}

impl LiveRequest {
    pub fn new(request_id: u64, command: LiveCommand) -> Self {
        Self {
            schema_version: LIVE_SCHEMA_VERSION,
            request_id,
            command,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != LIVE_SCHEMA_VERSION {
            return Err(format!(
                "unsupported live-session schema version {}; expected {}",
                self.schema_version, LIVE_SCHEMA_VERSION
            ));
        }
        if self.request_id == 0 {
            return Err("request_id must be greater than zero".to_string());
        }
        let bytes = serde_json::to_vec(self)
            .map_err(|error| format!("failed serializing live request: {error}"))?
            .len();
        if bytes > MAX_LIVE_REQUEST_BYTES {
            return Err(format!(
                "live request requires {bytes} bytes; limit is {MAX_LIVE_REQUEST_BYTES} bytes"
            ));
        }
        Ok(())
    }
}

const fn live_schema_version() -> u16 {
    LIVE_SCHEMA_VERSION
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LiveCommand {
    Help,
    Status,
    Pause,
    Resume,
    Step {
        #[serde(default = "one_tick")]
        ticks: u32,
    },
    Cancel {
        request_id: u64,
    },
    Quit,
    Symbols {
        #[serde(default)]
        query: Option<String>,
        #[serde(default)]
        kind: Option<String>,
        #[serde(default)]
        files: Vec<String>,
        #[serde(default)]
        owner: Option<String>,
        #[serde(default)]
        page: u32,
        #[serde(default = "default_symbol_page_limit")]
        limit: usize,
    },
    Read {
        name: String,
        #[serde(default)]
        kind: Option<String>,
        #[serde(default)]
        file: Option<String>,
        #[serde(default)]
        owner: Option<String>,
        #[serde(default)]
        signature: Option<String>,
    },
    References {
        symbol: String,
        #[serde(default = "default_reference_limit")]
        limit: usize,
    },
    Validate {
        requirement: LiveValidationRequirement,
        #[serde(default)]
        frames: u32,
    },
    ValidationSnapshot,
    ValidationRestore,
    ValidationClear,
    Complete {
        buffer: String,
        cursor: usize,
        #[serde(default = "default_completion_limit")]
        limit: usize,
        #[serde(default)]
        context: CompletionContext,
    },
    Palette {
        #[serde(default)]
        query: String,
        #[serde(default)]
        page: u32,
        #[serde(default = "default_completion_limit")]
        limit: usize,
        #[serde(default)]
        context: CompletionContext,
    },
    Edit {
        operation: LiveEditOperation,
        target: LiveSymbolTarget,
        #[serde(default)]
        source: Option<String>,
        #[serde(default)]
        expected_source_hash: Option<String>,
        #[serde(default)]
        preview: bool,
        #[serde(default = "default_true")]
        run_tests: bool,
    },
    EditBatch {
        edits: Vec<LiveEdit>,
        #[serde(default)]
        preview: bool,
        #[serde(default = "default_true")]
        run_tests: bool,
    },
    Preview,
    Apply {
        #[serde(default = "default_true")]
        run_tests: bool,
    },
    Changes,
    Undo {
        #[serde(default = "default_true")]
        run_tests: bool,
    },
    Redo {
        #[serde(default = "default_true")]
        run_tests: bool,
    },
    Inspect {
        path: String,
    },
    InspectAll {
        #[serde(default = "default_inspect_limit")]
        limit: usize,
        #[serde(default)]
        concise: bool,
    },
    Watch {
        path: String,
    },
    Unwatch {
        #[serde(default)]
        path: Option<String>,
    },
    Set {
        path: String,
        expression: String,
        #[serde(default)]
        preview: bool,
    },
    Print {
        expression: String,
    },
    Do {
        code: String,
        #[serde(default)]
        preview: bool,
    },
    CellPut {
        name: String,
        code: String,
    },
    CellRun {
        name: String,
        #[serde(default)]
        preview: bool,
    },
    CellList,
    CellClear {
        #[serde(default)]
        name: Option<String>,
    },
    CellPersist {
        name: String,
        target: LiveSymbolTarget,
        #[serde(default)]
        preview: bool,
        #[serde(default = "default_true")]
        run_tests: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveValidationRequirement {
    pub path: String,
    #[serde(default = "default_validation_operator")]
    pub op: String,
    pub value: serde_json::Value,
}

pub fn compare_live_validation_values(
    actual: &serde_json::Value,
    operator: &str,
    expected: &serde_json::Value,
) -> Result<bool, String> {
    if matches!(operator, "eq" | "ne") {
        let equal = actual == expected
            || actual
                .as_f64()
                .zip(expected.as_f64())
                .is_some_and(|(actual, expected)| actual == expected);
        return Ok(if operator == "eq" { equal } else { !equal });
    }
    let actual = actual
        .as_f64()
        .ok_or_else(|| format!("operator '{operator}' requires a numeric actual value"))?;
    let expected = expected
        .as_f64()
        .ok_or_else(|| format!("operator '{operator}' requires a numeric expected value"))?;
    match operator {
        "lt" => Ok(actual < expected),
        "lte" => Ok(actual <= expected),
        "gt" => Ok(actual > expected),
        "gte" => Ok(actual >= expected),
        _ => Err(format!(
            "unsupported validation operator '{operator}'; use eq, ne, lt, lte, gt, or gte"
        )),
    }
}

const fn one_tick() -> u32 {
    1
}

const fn default_completion_limit() -> usize {
    32
}

const fn default_inspect_limit() -> usize {
    32
}

const fn default_symbol_page_limit() -> usize {
    32
}

const fn default_reference_limit() -> usize {
    128
}

fn default_validation_operator() -> String {
    "eq".to_string()
}

const fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveEditOperation {
    Add,
    Update,
    Delete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveEdit {
    pub operation: LiveEditOperation,
    pub target: LiveSymbolTarget,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub expected_source_hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveSymbolTarget {
    pub name: String,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub signature: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LiveResponse {
    pub schema_version: u16,
    pub request_id: u64,
    pub tick: u64,
    pub ok: bool,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub truncated: bool,
}

impl LiveResponse {
    pub fn success(request_id: u64, tick: u64, kind: impl Into<String>, data: Value) -> Self {
        Self {
            schema_version: LIVE_SCHEMA_VERSION,
            request_id,
            tick,
            ok: true,
            kind: kind.into(),
            data: Some(data),
            error: None,
            truncated: false,
        }
    }

    pub fn failure(request_id: u64, tick: u64, error: impl Into<String>) -> Self {
        Self {
            schema_version: LIVE_SCHEMA_VERSION,
            request_id,
            tick,
            ok: false,
            kind: "error".to_string(),
            data: None,
            error: Some(error.into()),
            truncated: false,
        }
    }

    pub fn bounded(mut self, max_bytes: usize) -> Self {
        let Ok(encoded) = serde_json::to_vec(&self) else {
            return Self::failure(
                self.request_id,
                self.tick,
                "failed to serialize live response",
            );
        };
        if encoded.len() <= max_bytes {
            return self;
        }
        self.data = Some(serde_json::json!({
            "message": "live response exceeded the configured output limit",
            "original_bytes": encoded.len(),
            "limit_bytes": max_bytes,
        }));
        self.error = (!self.ok).then(|| "live response exceeded the output limit".to_string());
        self.truncated = true;
        self
    }
}

#[derive(Clone)]
pub struct LiveSessionClient {
    requests: Sender<LiveRequest>,
    responses: Receiver<LiveResponse>,
}

pub struct LiveSessionServer {
    requests: Receiver<LiveRequest>,
    responses: Sender<LiveResponse>,
    output_limit: usize,
}

#[derive(Debug)]
pub enum LiveResponseSendError {
    Full(LiveResponse),
    Disconnected,
}

pub fn live_session(capacity: usize) -> (LiveSessionClient, LiveSessionServer) {
    let capacity = capacity.max(1);
    let (request_tx, request_rx) = bounded(capacity);
    let (response_tx, response_rx) = bounded(capacity);
    (
        LiveSessionClient {
            requests: request_tx,
            responses: response_rx,
        },
        LiveSessionServer {
            requests: request_rx,
            responses: response_tx,
            output_limit: DEFAULT_LIVE_OUTPUT_BYTES,
        },
    )
}

impl LiveSessionClient {
    pub fn submit(&self, request: LiveRequest) -> Result<(), String> {
        request.validate()?;
        self.requests
            .try_send(request)
            .map_err(|error| match error {
                TrySendError::Full(_) => "live-session command queue is full".to_string(),
                TrySendError::Disconnected(_) => "live session has ended".to_string(),
            })
    }

    pub fn receive_timeout(&self, timeout: Duration) -> Result<LiveResponse, String> {
        self.responses
            .recv_timeout(timeout)
            .map_err(|error| format!("live-session response unavailable: {error}"))
    }

    pub fn try_receive(&self) -> Result<Option<LiveResponse>, String> {
        match self.responses.try_recv() {
            Ok(response) => Ok(Some(response)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err("live session has ended".to_string()),
        }
    }
}

impl LiveSessionServer {
    pub fn set_output_limit(&mut self, bytes: usize) {
        self.output_limit = bytes.max(256);
    }

    pub fn drain(&self, limit: usize) -> Vec<LiveRequest> {
        let mut requests = Vec::new();
        while requests.len() < limit {
            match self.requests.try_recv() {
                Ok(request) => requests.push(request),
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        requests
    }

    pub fn respond(&self, response: LiveResponse) -> Result<(), LiveResponseSendError> {
        self.responses
            .try_send(response.bounded(self.output_limit))
            .map_err(|error| match error {
                TrySendError::Full(response) => LiveResponseSendError::Full(response),
                TrySendError::Disconnected(_) => LiveResponseSendError::Disconnected,
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionItem {
    pub text: String,
    pub kind: String,
    pub detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<LiveSymbolTarget>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<CompletionScope>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionContext {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_signature: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_offset: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionScope {
    pub owner: String,
    pub file: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_signature: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_end: Option<usize>,
    pub visible_from: usize,
    pub visible_to: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionQuery {
    pub replacement_start: usize,
    pub replacement_end: usize,
    pub page: u32,
    pub truncated: bool,
    pub items: Vec<CompletionItem>,
}

#[derive(Debug, Clone, Default)]
pub struct CompletionIndex {
    items: Vec<IndexedCompletionItem>,
}

#[derive(Debug, Clone)]
struct IndexedCompletionItem {
    normalized_text: String,
    item: CompletionItem,
}

impl CompletionIndex {
    pub fn replace<I>(&mut self, items: I)
    where
        I: IntoIterator<Item = CompletionItem>,
    {
        let mut items = items.into_iter().collect::<Vec<_>>();
        items.sort_by(|left, right| completion_item_key(left).cmp(&completion_item_key(right)));
        items.dedup();
        self.items = items
            .into_iter()
            .map(|item| IndexedCompletionItem {
                normalized_text: item.text.to_ascii_lowercase(),
                item,
            })
            .collect();
    }

    pub fn extend<I>(&mut self, items: I)
    where
        I: IntoIterator<Item = CompletionItem>,
    {
        let merged = self
            .items
            .iter()
            .map(|indexed| indexed.item.clone())
            .chain(items)
            .collect::<Vec<_>>();
        self.replace(merged);
    }

    pub fn retain(&mut self, mut keep: impl FnMut(&CompletionItem) -> bool) {
        self.items.retain(|indexed| keep(&indexed.item));
    }

    pub fn complete(&self, buffer: &str, cursor: usize, limit: usize) -> Vec<CompletionItem> {
        self.query(buffer, cursor, limit).items
    }

    pub fn query(&self, buffer: &str, cursor: usize, limit: usize) -> CompletionQuery {
        self.query_with_context(buffer, cursor, limit, &CompletionContext::default())
    }

    pub fn query_with_context(
        &self,
        buffer: &str,
        cursor: usize,
        limit: usize,
        context: &CompletionContext,
    ) -> CompletionQuery {
        rank_indexed_completion_items(&self.items, buffer, cursor, limit, context)
    }
}

pub fn rank_completion_items(
    items: &[CompletionItem],
    buffer: &str,
    cursor: usize,
    limit: usize,
) -> CompletionQuery {
    rank_completion_items_with_context(items, buffer, cursor, limit, &CompletionContext::default())
}

pub fn rank_completion_items_with_context(
    items: &[CompletionItem],
    buffer: &str,
    cursor: usize,
    limit: usize,
    context: &CompletionContext,
) -> CompletionQuery {
    let indexed = items
        .iter()
        .cloned()
        .map(|item| IndexedCompletionItem {
            normalized_text: item.text.to_ascii_lowercase(),
            item,
        })
        .collect::<Vec<_>>();
    rank_indexed_completion_items(&indexed, buffer, cursor, limit, context)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct CompletionRank<'a> {
    match_class: u8,
    scope_distance: usize,
    expected_type: u8,
    kind: u8,
    gaps: usize,
    first: usize,
    text_len: usize,
    text: &'a str,
    item_kind: &'a str,
    detail: &'a str,
}

fn rank_indexed_completion_items(
    items: &[IndexedCompletionItem],
    buffer: &str,
    cursor: usize,
    limit: usize,
    context: &CompletionContext,
) -> CompletionQuery {
    let mut cursor = cursor.min(buffer.len());
    while !buffer.is_char_boundary(cursor) {
        cursor -= 1;
    }
    let replacement_start = completion_replacement_start(buffer, cursor);
    let query = &buffer[replacement_start..cursor];
    let normalized_query = query.to_ascii_lowercase();
    let bounded_limit = limit.min(256);
    let mut unscoped_match_count = 0usize;
    let mut best = Vec::<(CompletionRank<'_>, &CompletionItem)>::with_capacity(bounded_limit);
    let mut scoped_matches =
        BTreeMap::<(&str, &str, Option<&str>), (CompletionRank<'_>, &CompletionItem)>::new();
    for indexed in items {
        let Some(scope_distance) = completion_scope_distance(&indexed.item, context) else {
            continue;
        };
        let Some((match_class, gaps, first)) =
            fuzzy_completion_score(&indexed.normalized_text, &normalized_query)
        else {
            continue;
        };
        let rank = CompletionRank {
            match_class,
            scope_distance,
            expected_type: completion_expected_type_priority(&indexed.item, context),
            kind: completion_kind_priority(&indexed.item.kind, query),
            gaps,
            first,
            text_len: indexed.item.text.len(),
            text: &indexed.item.text,
            item_kind: &indexed.item.kind,
            detail: &indexed.item.detail,
        };
        if indexed.item.scope.is_some() {
            let overload_detail =
                (indexed.item.kind == "method").then_some(indexed.item.detail.as_str());
            let key = (
                indexed.item.text.as_str(),
                indexed.item.kind.as_str(),
                overload_detail,
            );
            match scoped_matches.get_mut(&key) {
                Some(best) if rank < best.0 => *best = (rank, &indexed.item),
                Some(_) => {}
                None => {
                    scoped_matches.insert(key, (rank, &indexed.item));
                }
            }
        } else {
            unscoped_match_count = unscoped_match_count.saturating_add(1);
            retain_bounded_completion(&mut best, (rank, &indexed.item), bounded_limit);
        }
    }
    let total_matches = unscoped_match_count.saturating_add(scoped_matches.len());
    for candidate in scoped_matches.into_values() {
        retain_bounded_completion(&mut best, candidate, bounded_limit);
    }
    best.sort_by_key(|(rank, _)| *rank);
    let items = best.into_iter().map(|(_, item)| item.clone()).collect();
    CompletionQuery {
        replacement_start,
        replacement_end: cursor,
        page: 0,
        truncated: total_matches > bounded_limit,
        items,
    }
}

fn retain_bounded_completion<'a>(
    best: &mut Vec<(CompletionRank<'a>, &'a CompletionItem)>,
    candidate: (CompletionRank<'a>, &'a CompletionItem),
    limit: usize,
) {
    if limit == 0 {
        return;
    }
    if best.len() < limit {
        best.push(candidate);
        return;
    }
    let (worst_index, worst) = best
        .iter()
        .enumerate()
        .max_by_key(|(_, (rank, _))| *rank)
        .expect("bounded completion set is not empty");
    if candidate.0 < worst.0 {
        best[worst_index] = candidate;
    }
}

fn completion_item_key(
    item: &CompletionItem,
) -> (
    &str,
    &str,
    &str,
    Option<&str>,
    Option<&str>,
    Option<(&str, &str, Option<&str>, Option<usize>, usize, usize)>,
) {
    (
        &item.text,
        &item.kind,
        &item.detail,
        item.type_name.as_deref(),
        item.source.as_deref(),
        item.scope.as_ref().map(|scope| {
            (
                scope.owner.as_str(),
                scope.file.as_str(),
                scope.owner_signature.as_deref(),
                scope.owner_end,
                scope.visible_from,
                scope.visible_to,
            )
        }),
    )
}

fn completion_replacement_start(buffer: &str, cursor: usize) -> usize {
    buffer[..cursor]
        .char_indices()
        .rev()
        .find(|(index, character)| {
            character.is_whitespace()
                || matches!(
                    character,
                    '(' | ')'
                        | '['
                        | ']'
                        | ','
                        | ';'
                        | '{'
                        | '}'
                        | '='
                        | '+'
                        | '-'
                        | '*'
                        | '/'
                        | '%'
                        | '<'
                        | '>'
                        | '!'
                )
                || (*character == ':' && *index > 0)
        })
        .map_or(0, |(index, character)| index + character.len_utf8())
}

fn fuzzy_completion_score(text: &str, query: &str) -> Option<(u8, usize, usize)> {
    if text == query {
        return Some((0, 0, 0));
    }
    if text.starts_with(&query) {
        return Some((1, 0, text.len().saturating_sub(query.len())));
    }
    if let Some(start) = text.find(&query) {
        return Some((2, start, text.len().saturating_sub(query.len())));
    }
    let mut text_indices = text.char_indices();
    let mut first = None;
    let mut previous = None;
    let mut gaps = 0usize;
    for needle in query.chars() {
        let (index, _) = text_indices.find(|(_, candidate)| *candidate == needle)?;
        first.get_or_insert(index);
        if let Some(previous) = previous {
            gaps += index.saturating_sub(previous + 1);
        }
        previous = Some(index);
    }
    Some((3, gaps, first.unwrap_or(0)))
}

fn completion_scope_distance(item: &CompletionItem, context: &CompletionContext) -> Option<usize> {
    let Some(scope) = item.scope.as_ref() else {
        return Some(usize::MAX);
    };
    if context.owner.as_deref() != Some(scope.owner.as_str()) {
        return None;
    }
    if context
        .file
        .as_deref()
        .is_some_and(|file| file != scope.file)
    {
        return None;
    }
    if context
        .owner_signature
        .as_deref()
        .is_some_and(|signature| scope.owner_signature.as_deref() != Some(signature))
    {
        return None;
    }
    let offset = context.source_offset.or(scope.owner_end)?;
    if offset < scope.visible_from || offset > scope.visible_to {
        return None;
    }
    Some(usize::MAX.saturating_sub(scope.visible_from))
}

fn completion_expected_type_priority(item: &CompletionItem, context: &CompletionContext) -> u8 {
    match context.expected_type.as_deref() {
        Some(expected) if item.type_name.as_deref() == Some(expected) => 0,
        Some(_) => 1,
        None => 0,
    }
}

fn completion_kind_priority(kind: &str, query: &str) -> u8 {
    if query.starts_with(':') {
        return u8::from(kind != "command");
    }
    if query.contains('.') {
        return match kind {
            "field" | "state_path" => 0,
            "method" => 1,
            _ => 2,
        };
    }
    match kind {
        "local" => 0,
        "parameter" => 1,
        "field" => 2,
        "method" => 3,
        "function" => 4,
        "global" | "constant" | "state_path" => 5,
        "struct" | "enum" | "enum_variant" => 6,
        "scratch_cell" => 7,
        "command" => 8,
        _ => 9,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScratchCell {
    pub name: String,
    pub code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_result: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_tick: Option<u64>,
}

#[derive(Debug, Default)]
pub struct ScratchWorkspace {
    cells: BTreeMap<String, ScratchCell>,
}

impl ScratchWorkspace {
    pub fn put(&mut self, name: &str, code: String) -> Result<(), String> {
        validate_cell_name(name)?;
        if code.len() > MAX_LIVE_MULTILINE_BYTES {
            return Err(format!(
                "scratch cell exceeds {MAX_LIVE_MULTILINE_BYTES} bytes"
            ));
        }
        if !self.cells.contains_key(name) && self.cells.len() >= MAX_SCRATCH_CELLS {
            return Err(format!(
                "scratch workspace is limited to {MAX_SCRATCH_CELLS} cells"
            ));
        }
        self.cells.insert(
            name.to_string(),
            ScratchCell {
                name: name.to_string(),
                code,
                last_result: None,
                last_tick: None,
            },
        );
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&ScratchCell> {
        self.cells.get(name)
    }

    pub fn record_result(&mut self, name: &str, tick: u64, result: String) -> Result<(), String> {
        let cell = self
            .cells
            .get_mut(name)
            .ok_or_else(|| format!("scratch cell '{name}' not found"))?;
        cell.last_result = Some(result);
        cell.last_tick = Some(tick);
        Ok(())
    }

    pub fn list(&self) -> Vec<ScratchCell> {
        self.cells.values().cloned().collect()
    }

    pub fn clear(&mut self, name: Option<&str>) -> Result<(), String> {
        if let Some(name) = name {
            if self.cells.remove(name).is_none() {
                return Err(format!("scratch cell '{name}' not found"));
            }
        } else {
            self.cells.clear();
        }
        Ok(())
    }
}

fn validate_cell_name(name: &str) -> Result<(), String> {
    let mut chars = name.chars();
    if !chars
        .next()
        .is_some_and(|character| character == '_' || character.is_ascii_alphabetic())
        || !chars.all(|character| character == '_' || character.is_ascii_alphanumeric())
    {
        return Err(format!("invalid scratch cell name '{name}'"));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalInput {
    Request(LiveRequest),
    Continue { prompt: &'static str },
}

#[derive(Debug, Default)]
pub struct TerminalBuffer {
    next_request_id: u64,
    pending: Option<PendingMultiline>,
}

#[derive(Debug)]
struct PendingMultiline {
    request_id: u64,
    command: PendingCommand,
    lines: Vec<String>,
    bytes: usize,
}

#[derive(Debug)]
enum PendingCommand {
    Edit {
        operation: LiveEditOperation,
        target: LiveSymbolTarget,
        preview: bool,
    },
    Do {
        preview: bool,
    },
    CellPut {
        name: String,
    },
}

impl TerminalBuffer {
    pub fn new() -> Self {
        Self {
            next_request_id: 1,
            pending: None,
        }
    }

    pub fn feed_line(&mut self, line: &str) -> Result<TerminalInput, String> {
        if let Some(pending) = self.pending.as_mut() {
            if line.trim() == ":abort" {
                self.pending = None;
                return Ok(TerminalInput::Continue { prompt: "stasis> " });
            }
            if line.trim() != ":end" {
                if pending.bytes.saturating_add(line.len()).saturating_add(1)
                    > MAX_LIVE_MULTILINE_BYTES
                {
                    self.pending = None;
                    return Err(format!(
                        "multiline input exceeds {MAX_LIVE_MULTILINE_BYTES} bytes"
                    ));
                }
                pending.lines.push(line.to_string());
                pending.bytes = pending.bytes.saturating_add(line.len()).saturating_add(1);
                return Ok(TerminalInput::Continue { prompt: "... " });
            }
            let pending = self.pending.take().expect("pending command");
            let source = pending.lines.join("\n");
            let command = match pending.command {
                PendingCommand::Edit {
                    operation,
                    target,
                    preview,
                } => LiveCommand::Edit {
                    operation,
                    target,
                    source: (operation != LiveEditOperation::Delete).then_some(source),
                    expected_source_hash: None,
                    preview,
                    run_tests: true,
                },
                PendingCommand::Do { preview } => LiveCommand::Do {
                    code: source,
                    preview,
                },
                PendingCommand::CellPut { name } => LiveCommand::CellPut { name, code: source },
            };
            return Ok(TerminalInput::Request(LiveRequest::new(
                pending.request_id,
                command,
            )));
        }

        if line.trim_start().starts_with('{') {
            let request = serde_json::from_str::<LiveRequest>(line)
                .map_err(|error| format!("invalid live request JSON: {error}"))?;
            request.validate()?;
            self.next_request_id = self
                .next_request_id
                .max(request.request_id.saturating_add(1));
            return Ok(TerminalInput::Request(request));
        }

        let request_id = self.next_request_id;
        self.next_request_id = self.next_request_id.saturating_add(1);
        let command = parse_terminal_command(line)?;
        if let ParsedTerminalCommand::Pending(command) = command {
            self.pending = Some(PendingMultiline {
                request_id,
                command,
                lines: Vec::new(),
                bytes: 0,
            });
            return Ok(TerminalInput::Continue { prompt: "... " });
        }
        let ParsedTerminalCommand::Ready(command) = command else {
            unreachable!()
        };
        Ok(TerminalInput::Request(LiveRequest::new(
            request_id, command,
        )))
    }

    pub fn cancel_pending(&mut self) -> bool {
        self.pending.take().is_some()
    }

    pub fn completion_context(&self) -> CompletionContext {
        let Some(PendingMultiline {
            command: PendingCommand::Edit { target, .. },
            ..
        }) = self.pending.as_ref()
        else {
            return CompletionContext::default();
        };
        CompletionContext {
            owner: Some(target.name.clone()),
            file: target.file.clone(),
            owner_signature: target.signature.clone(),
            source_offset: None,
            expected_type: None,
        }
    }
}

enum ParsedTerminalCommand {
    Ready(LiveCommand),
    Pending(PendingCommand),
}

fn parse_terminal_command(line: &str) -> Result<ParsedTerminalCommand, String> {
    let args = split_terminal_args(line)?;
    let Some(command) = args.first().map(String::as_str) else {
        return Err("empty live command".to_string());
    };
    let ready = |command| Ok(ParsedTerminalCommand::Ready(command));
    match command {
        ":help" => ready(LiveCommand::Help),
        ":status" => ready(LiveCommand::Status),
        ":pause" => ready(LiveCommand::Pause),
        ":resume" => ready(LiveCommand::Resume),
        ":quit" => ready(LiveCommand::Quit),
        ":step" => ready(LiveCommand::Step {
            ticks: args
                .get(1)
                .map(|value| parse_u32("ticks", value))
                .transpose()?
                .unwrap_or(1),
        }),
        ":cancel" => ready(LiveCommand::Cancel {
            request_id: required_arg(&args, 1, "request id")?
                .parse::<u64>()
                .map_err(|error| format!("invalid request id: {error}"))?,
        }),
        ":symbols" | ":find" => ready(LiveCommand::Symbols {
            query: args.get(1).filter(|arg| !arg.starts_with("--")).cloned(),
            kind: terminal_selector_value(&args, "--kind"),
            files: terminal_selector_values(&args, "--file"),
            owner: terminal_selector_value(&args, "--owner"),
            page: terminal_selector_value(&args, "--page")
                .map(|value| parse_u32("page", &value))
                .transpose()?
                .unwrap_or(0),
            limit: terminal_selector_value(&args, "--limit")
                .map(|value| parse_u32("limit", &value))
                .transpose()?
                .map(|value| value as usize)
                .unwrap_or_else(default_symbol_page_limit),
        }),
        ":read" => ready(LiveCommand::Read {
            name: required_arg(&args, 1, "symbol name")?.to_string(),
            kind: args.get(2).cloned(),
            file: terminal_selector_value(&args, "--file")
                .or_else(|| args.get(3).filter(|arg| !arg.starts_with("--")).cloned()),
            owner: terminal_selector_value(&args, "--owner"),
            signature: terminal_selector_value(&args, "--signature"),
        }),
        ":references" | ":refs" => ready(LiveCommand::References {
            symbol: required_arg(&args, 1, "symbol")?.to_string(),
            limit: terminal_selector_value(&args, "--limit")
                .map(|value| parse_u32("limit", &value))
                .transpose()?
                .map(|value| value as usize)
                .unwrap_or_else(default_reference_limit),
        }),
        ":validate" => ready(LiveCommand::Validate {
            requirement: LiveValidationRequirement {
                path: required_arg(&args, 1, "state path")?.to_string(),
                op: required_arg(&args, 2, "operator")?.to_string(),
                value: parse_terminal_scalar(required_arg(&args, 3, "expected value")?),
            },
            frames: terminal_selector_value(&args, "--frames")
                .map(|value| parse_u32("frames", &value))
                .transpose()?
                .unwrap_or(0),
        }),
        ":complete" => {
            let buffer = args.get(1).cloned().unwrap_or_default();
            ready(LiveCommand::Complete {
                cursor: buffer.len(),
                buffer,
                limit: 32,
                context: CompletionContext::default(),
            })
        }
        ":palette" => ready(LiveCommand::Palette {
            query: args
                .get(1)
                .filter(|value| !value.starts_with("--"))
                .cloned()
                .unwrap_or_default(),
            page: terminal_selector_value(&args, "--page")
                .map(|value| parse_u32("page", &value))
                .transpose()?
                .unwrap_or(0),
            limit: terminal_selector_value(&args, "--limit")
                .map(|value| parse_u32("limit", &value))
                .transpose()?
                .map(|value| value as usize)
                .unwrap_or_else(default_completion_limit),
            context: CompletionContext {
                owner: terminal_selector_value(&args, "--owner"),
                file: terminal_selector_value(&args, "--file"),
                owner_signature: terminal_selector_value(&args, "--signature"),
                source_offset: terminal_selector_value(&args, "--offset")
                    .map(|value| parse_u32("offset", &value))
                    .transpose()?
                    .map(|value| value as usize),
                expected_type: terminal_selector_value(&args, "--expected-type"),
            },
        }),
        ":preview" => ready(LiveCommand::Preview),
        ":apply" => ready(LiveCommand::Apply { run_tests: true }),
        ":changes" => ready(LiveCommand::Changes),
        ":undo" => ready(LiveCommand::Undo { run_tests: true }),
        ":redo" => ready(LiveCommand::Redo { run_tests: true }),
        ":inspect" if args.len() == 1 => ready(LiveCommand::InspectAll {
            limit: default_inspect_limit(),
            concise: false,
        }),
        ":inspect" => ready(LiveCommand::Inspect {
            path: required_arg(&args, 1, "state path")?.to_string(),
        }),
        ":watch" => ready(LiveCommand::Watch {
            path: required_arg(&args, 1, "state path")?.to_string(),
        }),
        ":unwatch" => ready(LiveCommand::Unwatch {
            path: args.get(1).cloned(),
        }),
        ":set" => ready(LiveCommand::Set {
            path: required_arg(&args, 1, "state path")?.to_string(),
            expression: required_arg(&args, 2, "expression")?.to_string(),
            preview: args.iter().any(|arg| arg == "--preview"),
        }),
        ":print" => ready(LiveCommand::Print {
            expression: required_arg(&args, 1, "expression")?.to_string(),
        }),
        ":do" => Ok(ParsedTerminalCommand::Pending(PendingCommand::Do {
            preview: args.iter().any(|arg| arg == "--preview"),
        })),
        ":cell" if args.get(1).map(String::as_str) == Some("put") => {
            Ok(ParsedTerminalCommand::Pending(PendingCommand::CellPut {
                name: required_arg(&args, 2, "cell name")?.to_string(),
            }))
        }
        ":cell" if args.get(1).map(String::as_str) == Some("run") => ready(LiveCommand::CellRun {
            name: required_arg(&args, 2, "cell name")?.to_string(),
            preview: args.iter().any(|arg| arg == "--preview"),
        }),
        ":cell" if args.get(1).map(String::as_str) == Some("list") => ready(LiveCommand::CellList),
        ":cell" if args.get(1).map(String::as_str) == Some("clear") => {
            ready(LiveCommand::CellClear {
                name: args.get(2).cloned(),
            })
        }
        ":cell" if args.get(1).map(String::as_str) == Some("persist") => {
            ready(LiveCommand::CellPersist {
                name: required_arg(&args, 2, "cell name")?.to_string(),
                target: LiveSymbolTarget {
                    kind: Some(required_arg(&args, 3, "symbol kind")?.to_string()),
                    name: required_arg(&args, 4, "symbol name")?.to_string(),
                    file: terminal_selector_value(&args, "--file")
                        .or_else(|| args.get(5).filter(|arg| !arg.starts_with("--")).cloned()),
                    owner: terminal_selector_value(&args, "--owner"),
                    signature: terminal_selector_value(&args, "--signature"),
                },
                preview: args.iter().any(|arg| arg == "--preview"),
                run_tests: true,
            })
        }
        ":add" | ":update" | ":delete" => {
            let operation = match command {
                ":add" => LiveEditOperation::Add,
                ":update" => LiveEditOperation::Update,
                _ => LiveEditOperation::Delete,
            };
            let target = LiveSymbolTarget {
                kind: Some(required_arg(&args, 1, "symbol kind")?.to_string()),
                name: required_arg(&args, 2, "symbol name")?.to_string(),
                file: terminal_selector_value(&args, "--file")
                    .or_else(|| args.get(3).filter(|arg| !arg.starts_with("--")).cloned()),
                owner: terminal_selector_value(&args, "--owner"),
                signature: terminal_selector_value(&args, "--signature"),
            };
            let preview = args.iter().any(|arg| arg == "--preview");
            if operation == LiveEditOperation::Delete {
                ready(LiveCommand::Edit {
                    operation,
                    target,
                    source: None,
                    expected_source_hash: None,
                    preview,
                    run_tests: true,
                })
            } else {
                Ok(ParsedTerminalCommand::Pending(PendingCommand::Edit {
                    operation,
                    target,
                    preview,
                }))
            }
        }
        _ => Err(format!("unknown live command '{command}'; use :help")),
    }
}

fn parse_terminal_scalar(value: &str) -> serde_json::Value {
    serde_json::from_str(value).unwrap_or_else(|_| serde_json::Value::String(value.to_string()))
}

fn terminal_selector_value(args: &[String], option: &str) -> Option<String> {
    args.windows(2)
        .find(|pair| pair[0] == option)
        .map(|pair| pair[1].clone())
}

fn terminal_selector_values(args: &[String], option: &str) -> Vec<String> {
    args.windows(2)
        .filter(|pair| pair[0] == option)
        .map(|pair| pair[1].clone())
        .collect()
}

fn required_arg<'a>(args: &'a [String], index: usize, name: &str) -> Result<&'a str, String> {
    args.get(index)
        .map(String::as_str)
        .ok_or_else(|| format!("missing {name}"))
}

fn parse_u32(name: &str, value: &str) -> Result<u32, String> {
    value
        .parse::<u32>()
        .map_err(|error| format!("invalid {name} '{value}': {error}"))
}

fn split_terminal_args(line: &str) -> Result<Vec<String>, String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut escaped = false;
    for character in line.trim().chars() {
        if escaped {
            current.push(character);
            escaped = false;
            continue;
        }
        if character == '\\' {
            escaped = true;
            continue;
        }
        if let Some(expected) = quote {
            if character == expected {
                quote = None;
            } else {
                current.push(character);
            }
            continue;
        }
        if character == '\'' || character == '"' {
            quote = Some(character);
        } else if character.is_whitespace() {
            if !current.is_empty() {
                args.push(std::mem::take(&mut current));
            }
        } else {
            current.push(character);
        }
    }
    if quote.is_some() {
        return Err("unterminated quoted argument".to_string());
    }
    if escaped {
        current.push('\\');
    }
    if !current.is_empty() {
        args.push(current);
    }
    Ok(args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_batch_has_a_stable_json_contract() {
        let request = LiveRequest::new(
            7,
            LiveCommand::EditBatch {
                edits: vec![LiveEdit {
                    operation: LiveEditOperation::Update,
                    target: LiveSymbolTarget {
                        name: "tick".into(),
                        kind: Some("function".into()),
                        file: Some("src/main.stasis".into()),
                        owner: None,
                        signature: None,
                    },
                    source: Some("function tick(): void {}".into()),
                    expected_source_hash: Some("before".into()),
                }],
                preview: false,
                run_tests: true,
            },
        );
        let json = serde_json::to_value(&request).expect("serialize");
        assert_eq!(json["type"], "edit_batch");
        assert_eq!(json["edits"][0]["operation"], "update");
        assert_eq!(json["edits"][0]["target"]["name"], "tick");
        assert_eq!(
            serde_json::from_value::<LiveRequest>(json).expect("round trip"),
            request
        );
    }

    #[test]
    fn bounded_queue_reports_backpressure_without_blocking() {
        let (client, server) = live_session(1);
        client
            .submit(LiveRequest::new(1, LiveCommand::Status))
            .expect("first request");
        assert_eq!(
            client
                .submit(LiveRequest::new(2, LiveCommand::Status))
                .expect_err("queue should be full"),
            "live-session command queue is full"
        );
        assert_eq!(server.drain(1)[0].request_id, 1);
    }

    #[test]
    fn response_output_is_bounded_and_explicitly_truncated() {
        let (client, mut server) = live_session(1);
        server.set_output_limit(256);
        server
            .respond(LiveResponse::success(
                7,
                9,
                "symbols",
                serde_json::json!({"source": "x".repeat(4096)}),
            ))
            .expect("response");
        let response = client
            .receive_timeout(Duration::from_millis(10))
            .expect("receive");
        assert!(response.truncated);
        assert_eq!(response.request_id, 7);
        assert_eq!(response.tick, 9);

        server
            .respond(LiveResponse::failure(8, 10, "x".repeat(4096)))
            .expect("failure response");
        let failure = client
            .receive_timeout(Duration::from_millis(10))
            .expect("receive failure");
        assert!(failure.truncated);
        assert_eq!(
            failure.error.as_deref(),
            Some("live response exceeded the output limit")
        );
        assert!(serde_json::to_vec(&failure).expect("encode").len() <= 256);
    }

    #[test]
    fn terminal_multiline_edit_preserves_inline_source() {
        let mut terminal = TerminalBuffer::new();
        assert!(matches!(
            terminal
                .feed_line(":update function tick src/main.stasis")
                .expect("start"),
            TerminalInput::Continue { .. }
        ));
        terminal
            .feed_line("function tick(): i32 {")
            .expect("body one");
        terminal.feed_line("    return 2;").expect("body two");
        terminal.feed_line("}").expect("body three");
        let TerminalInput::Request(request) = terminal.feed_line(":end").expect("finish") else {
            panic!("expected request")
        };
        let LiveCommand::Edit { source, target, .. } = request.command else {
            panic!("expected edit")
        };
        assert_eq!(target.name, "tick");
        assert_eq!(target.file.as_deref(), Some("src/main.stasis"));
        assert!(source.expect("source").contains("return 2"));
    }

    #[test]
    fn completion_uses_deterministic_prefix_ranking_and_deduplication() {
        let mut index = CompletionIndex::default();
        index.replace([
            CompletionItem {
                text: "tick".into(),
                kind: "function".into(),
                detail: "tick(): i32".into(),
                type_name: None,
                source: None,
                selector: None,
                scope: None,
            },
            CompletionItem {
                text: "tick".into(),
                kind: "function".into(),
                detail: "tick(): i32".into(),
                type_name: None,
                source: None,
                selector: None,
                scope: None,
            },
            CompletionItem {
                text: "tick".into(),
                kind: "function".into(),
                detail: "tick(value: i32): i32".into(),
                type_name: None,
                source: None,
                selector: None,
                scope: None,
            },
            CompletionItem {
                text: "ticker".into(),
                kind: "global".into(),
                detail: "i32".into(),
                type_name: None,
                source: None,
                selector: None,
                scope: None,
            },
            CompletionItem {
                text: "title".into(),
                kind: "global".into(),
                detail: "utf8[32]".into(),
                type_name: None,
                source: None,
                selector: None,
                scope: None,
            },
        ]);
        let result = index.complete(":read tick", 10, 10);
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].text, "tick");
        assert_eq!(result[1].text, "tick");
        assert_eq!(result[2].text, "ticker");
        assert_eq!(index.complete("éti", 1, 10).len(), 4);
    }

    #[test]
    fn completion_query_fuzzy_ranks_context_and_reports_replacement_range() {
        let mut index = CompletionIndex::default();
        index.replace([
            CompletionItem {
                text: "hero.hp".into(),
                kind: "field".into(),
                detail: "i32 via local hero: Player".into(),
                type_name: Some("i32".into()),
                source: None,
                selector: None,
                scope: None,
            },
            CompletionItem {
                text: "hero.damage".into(),
                kind: "method".into(),
                detail: "damage(player: Player, amount: i32): i32".into(),
                type_name: Some("i32".into()),
                source: None,
                selector: None,
                scope: None,
            },
            CompletionItem {
                text: ":help".into(),
                kind: "command".into(),
                detail: "live command".into(),
                type_name: None,
                source: None,
                selector: None,
                scope: None,
            },
        ]);
        let member = index.query("call(hrohp", 10, 10);
        assert_eq!(member.replacement_start, 5);
        assert_eq!(member.replacement_end, 10);
        assert_eq!(member.items[0].text, "hero.hp");
        assert!(!member.truncated);

        let commands = index.query(":h", 2, 1);
        assert_eq!(commands.items[0].text, ":help");
        assert!(!commands.truncated);

        let bounded = index.query("hero.", 5, 1);
        assert_eq!(bounded.items.len(), 1);
        assert!(bounded.truncated);
    }

    #[test]
    fn completion_query_filters_scopes_shadowing_and_expected_types() {
        let scoped =
            |detail: &str, owner: &str, from: usize, to: usize, owner_end: usize| CompletionItem {
                text: "value".into(),
                kind: "local".into(),
                detail: detail.into(),
                type_name: Some(if detail == "inner" { "i32" } else { "f32" }.into()),
                source: Some("src/main.stasis".into()),
                selector: None,
                scope: Some(CompletionScope {
                    owner: owner.into(),
                    file: "src/main.stasis".into(),
                    owner_signature: Some(format!("{owner}(): i32")),
                    owner_end: Some(owner_end),
                    visible_from: from,
                    visible_to: to,
                }),
            };
        let mut index = CompletionIndex::default();
        index.replace([
            scoped("outer", "tick", 10, 100, 100),
            scoped("inner", "tick", 50, 70, 100),
            scoped("render local", "render", 20, 90, 90),
        ]);
        let context = CompletionContext {
            owner: Some("tick".into()),
            file: Some("src/main.stasis".into()),
            owner_signature: None,
            source_offset: Some(60),
            expected_type: Some("i32".into()),
        };
        let result = index.query_with_context("val", 3, 10, &context);
        assert_eq!(result.items.len(), 1);
        assert_eq!(result.items[0].detail, "inner");

        let target_end = CompletionContext {
            owner: Some("tick".into()),
            file: Some("src/main.stasis".into()),
            ..CompletionContext::default()
        };
        let result = index.query_with_context("val", 3, 10, &target_end);
        assert_eq!(result.items.len(), 1);
        assert_eq!(result.items[0].detail, "outer");

        let mut nested_only = CompletionIndex::default();
        nested_only.replace([scoped("inner", "tick", 50, 70, 100)]);
        assert!(nested_only
            .query_with_context("val", 3, 10, &target_end)
            .items
            .is_empty());

        let render = CompletionContext {
            owner: Some("render".into()),
            source_offset: Some(50),
            ..CompletionContext::default()
        };
        assert_eq!(
            index.query_with_context("val", 3, 10, &render).items[0].detail,
            "render local"
        );
        assert!(index.query("val", 3, 10).items.is_empty());
    }

    #[test]
    fn completion_query_uses_signature_to_disambiguate_overload_scope() {
        let local = |text: &str, signature: &str| CompletionItem {
            text: text.into(),
            kind: "local".into(),
            detail: signature.into(),
            type_name: Some("i32".into()),
            source: Some("src/main.stasis".into()),
            selector: None,
            scope: Some(CompletionScope {
                owner: "update".into(),
                file: "src/main.stasis".into(),
                owner_signature: Some(signature.into()),
                owner_end: Some(100),
                visible_from: 10,
                visible_to: 100,
            }),
        };
        let mut index = CompletionIndex::default();
        index.replace([
            local("player_value", "update(player: Player): i32"),
            local("enemy_value", "update(enemy: Enemy): i32"),
        ]);
        let context = CompletionContext {
            owner: Some("update".into()),
            file: Some("src/main.stasis".into()),
            owner_signature: Some("update(player: Player): i32".into()),
            source_offset: None,
            expected_type: None,
        };
        let result = index.query_with_context("value", 5, 10, &context);
        assert_eq!(result.items.len(), 1);
        assert_eq!(result.items[0].text, "player_value");
    }

    #[test]
    fn completion_query_deduplicates_shadowing_before_bounding() {
        let scoped = |detail: &str, from: usize| CompletionItem {
            text: "value".into(),
            kind: "local".into(),
            detail: detail.into(),
            type_name: Some("i32".into()),
            source: Some("src/main.stasis".into()),
            selector: None,
            scope: Some(CompletionScope {
                owner: "tick".into(),
                file: "src/main.stasis".into(),
                owner_signature: Some("tick(): i32".into()),
                owner_end: Some(100),
                visible_from: from,
                visible_to: 100,
            }),
        };
        let mut index = CompletionIndex::default();
        index.replace([
            scoped("outer", 10),
            scoped("inner", 50),
            CompletionItem {
                text: "valid".into(),
                kind: "function".into(),
                detail: "valid(): i32".into(),
                type_name: Some("i32".into()),
                source: None,
                selector: None,
                scope: None,
            },
        ]);
        let context = CompletionContext {
            owner: Some("tick".into()),
            file: Some("src/main.stasis".into()),
            owner_signature: Some("tick(): i32".into()),
            source_offset: Some(60),
            expected_type: None,
        };
        let result = index.query_with_context("va", 2, 2, &context);
        assert_eq!(result.items.len(), 2);
        assert_eq!(result.items[0].detail, "inner");
        assert_eq!(result.items[1].text, "valid");
        assert!(!result.truncated);
    }

    #[test]
    fn completion_query_preserves_scoped_method_overloads() {
        let method = |detail: &str| CompletionItem {
            text: "hero.damage".into(),
            kind: "method".into(),
            detail: detail.into(),
            type_name: Some("i32".into()),
            source: Some("src/main.stasis".into()),
            selector: None,
            scope: Some(CompletionScope {
                owner: "tick".into(),
                file: "src/main.stasis".into(),
                owner_signature: Some("tick(): i32".into()),
                owner_end: Some(100),
                visible_from: 10,
                visible_to: 100,
            }),
        };
        let mut index = CompletionIndex::default();
        index.replace([
            method("damage(hero: Player, amount: i32): i32"),
            method("damage(hero: Player, amount: f32): i32"),
        ]);
        let context = CompletionContext {
            owner: Some("tick".into()),
            file: Some("src/main.stasis".into()),
            owner_signature: Some("tick(): i32".into()),
            source_offset: None,
            expected_type: None,
        };
        let result = index.query_with_context("hrodam", 6, 10, &context);
        assert_eq!(result.items.len(), 2);
        assert_ne!(result.items[0].detail, result.items[1].detail);
    }

    #[test]
    fn completion_query_reaches_late_catalog_items_with_bounded_top_k() {
        let mut items = (0..10_000)
            .map(|index| CompletionItem {
                text: format!("symbol_{index:05}"),
                kind: "function".into(),
                detail: format!("symbol_{index:05}(): i32"),
                type_name: Some("i32".into()),
                source: Some("src/generated.stasis".into()),
                selector: None,
                scope: None,
            })
            .collect::<Vec<_>>();
        items.push(CompletionItem {
            text: "very_late_target".into(),
            kind: "function".into(),
            detail: "very_late_target(): i32".into(),
            type_name: Some("i32".into()),
            source: Some("src/late.stasis".into()),
            selector: None,
            scope: None,
        });
        let mut index = CompletionIndex::default();
        index.replace(items);
        let target = index.query("vltgt", 5, 8);
        assert_eq!(target.items[0].text, "very_late_target");
        let bounded = index.query("symbol", 6, 8);
        assert_eq!(bounded.items.len(), 8);
        assert!(bounded.truncated);
    }

    #[test]
    fn completion_replacement_starts_after_unspaced_infix_operators() {
        let mut index = CompletionIndex::default();
        index.replace([CompletionItem {
            text: "player".into(),
            kind: "parameter".into(),
            detail: "Player".into(),
            type_name: Some("Player".into()),
            source: None,
            selector: None,
            scope: None,
        }]);
        for buffer in ["score+pla", "x!=pla", "value*pla", "items[pla"] {
            let result = index.query(buffer, buffer.len(), 4);
            assert_eq!(result.items[0].text, "player", "{buffer}");
            assert_eq!(&buffer[result.replacement_start..], "pla", "{buffer}");
        }
        assert_eq!(index.query(":hel", 4, 4).replacement_start, 0);
    }

    #[test]
    fn terminal_palette_command_preserves_query_text() {
        let mut terminal = TerminalBuffer::new();
        let TerminalInput::Request(request) = terminal
            .feed_line(
                ":palette hero.hp --page 2 --limit 12 --owner tick --file src/main.stasis --offset 42 --expected-type i32",
            )
            .expect("palette command")
        else {
            panic!("expected request")
        };
        assert_eq!(
            request.command,
            LiveCommand::Palette {
                query: "hero.hp".to_string(),
                page: 2,
                limit: 12,
                context: CompletionContext {
                    owner: Some("tick".into()),
                    file: Some("src/main.stasis".into()),
                    owner_signature: None,
                    source_offset: Some(42),
                    expected_type: Some("i32".into()),
                },
            }
        );
    }

    #[test]
    fn terminal_bare_inspect_requests_the_default_state_view() {
        let mut terminal = TerminalBuffer::new();
        let TerminalInput::Request(request) = terminal.feed_line(":inspect").expect("inspect")
        else {
            panic!("expected request")
        };
        assert_eq!(
            request.command,
            LiveCommand::InspectAll {
                limit: 32,
                concise: false,
            }
        );
    }

    #[test]
    fn terminal_completion_context_tracks_pending_semantic_owner() {
        let mut terminal = TerminalBuffer::new();
        terminal
            .feed_line(":update function tick src/main.stasis")
            .expect("start update");
        assert_eq!(
            terminal.completion_context(),
            CompletionContext {
                owner: Some("tick".into()),
                file: Some("src/main.stasis".into()),
                owner_signature: None,
                source_offset: None,
                expected_type: None,
            }
        );
        assert!(terminal.cancel_pending());
        assert_eq!(terminal.completion_context(), CompletionContext::default());
    }

    #[test]
    fn scratch_cells_are_session_only_and_record_tick_stamped_results() {
        let mut scratch = ScratchWorkspace::default();
        scratch
            .put("score", "score = score + 1;".into())
            .expect("put");
        scratch
            .record_result("score", 12, "ok".into())
            .expect("result");
        let cell = scratch.get("score").expect("cell");
        assert_eq!(cell.last_tick, Some(12));
        assert_eq!(cell.last_result.as_deref(), Some("ok"));
        scratch.clear(Some("score")).expect("clear");
        assert!(scratch.list().is_empty());
    }

    #[test]
    fn request_schema_and_ids_are_validated() {
        let (client, _) = live_session(1);
        let mut request = LiveRequest::new(0, LiveCommand::Status);
        assert!(client
            .submit(request.clone())
            .expect_err("zero id")
            .contains("request_id"));
        request.request_id = 1;
        request.schema_version = 99;
        assert!(client
            .submit(request)
            .expect_err("schema")
            .contains("schema version"));

        let oversized = LiveRequest::new(
            2,
            LiveCommand::Do {
                code: "x".repeat(MAX_LIVE_REQUEST_BYTES),
                preview: false,
            },
        );
        assert!(oversized.validate().expect_err("size").contains("limit"));
    }

    #[test]
    fn terminal_json_preserves_protocol_request_id() {
        let mut terminal = TerminalBuffer::new();
        let TerminalInput::Request(request) = terminal
            .feed_line(r#"{"schema_version":1,"request_id":42,"type":"status"}"#)
            .expect("json request")
        else {
            panic!("expected request")
        };
        assert_eq!(request.request_id, 42);
        assert_eq!(request.command, LiveCommand::Status);
    }

    #[test]
    fn terminal_multiline_can_be_canceled_and_is_bounded() {
        let mut terminal = TerminalBuffer::new();
        terminal.feed_line(":do").expect("start");
        assert!(terminal.cancel_pending());
        assert!(!terminal.cancel_pending());
        terminal.feed_line(":do").expect("restart");
        assert!(terminal
            .feed_line(&"x".repeat(MAX_LIVE_MULTILINE_BYTES + 1))
            .expect_err("bounded")
            .contains("exceeds"));
        assert!(!terminal.cancel_pending());
    }

    #[test]
    fn terminal_selectors_and_symbol_paging_are_explicit() {
        let mut terminal = TerminalBuffer::new();
        let TerminalInput::Request(request) = terminal
            .feed_line(
                r#":read update function --file src/game.stasis --owner Enemy --signature "update(i32): void""#,
            )
            .expect("selector")
        else {
            panic!("expected request")
        };
        let LiveCommand::Read {
            file,
            owner,
            signature,
            ..
        } = request.command
        else {
            panic!("expected read")
        };
        assert_eq!(file.as_deref(), Some("src/game.stasis"));
        assert_eq!(owner.as_deref(), Some("Enemy"));
        assert_eq!(signature.as_deref(), Some("update(i32): void"));

        let TerminalInput::Request(request) = terminal
            .feed_line(":symbols update --kind function --file src/game.stasis --file src/enemy.stasis --owner Enemy --page 2 --limit 10")
            .expect("page")
        else {
            panic!("expected request")
        };
        assert!(matches!(
            request.command,
            LiveCommand::Symbols {
                query: Some(ref query),
                kind: Some(ref kind),
                ref files,
                owner: Some(ref owner),
                page: 2,
                limit: 10,
            } if query == "update"
                && kind == "function"
                && files == &["src/game.stasis", "src/enemy.stasis"]
                && owner == "Enemy"
        ));
    }

    #[test]
    fn terminal_exposes_reference_and_runtime_validation_commands() {
        let mut terminal = TerminalBuffer::new();
        let TerminalInput::Request(references) = terminal
            .feed_line(":references GameState.player_y --limit 24")
            .expect("references")
        else {
            panic!("expected reference request");
        };
        assert_eq!(
            references.command,
            LiveCommand::References {
                symbol: "GameState.player_y".into(),
                limit: 24,
            }
        );

        let TerminalInput::Request(validation) = terminal
            .feed_line(":validate Render.command1_h eq 144 --frames 2")
            .expect("validation")
        else {
            panic!("expected validation request");
        };
        assert_eq!(
            validation.command,
            LiveCommand::Validate {
                requirement: LiveValidationRequirement {
                    path: "Render.command1_h".into(),
                    op: "eq".into(),
                    value: serde_json::json!(144),
                },
                frames: 2,
            }
        );
    }
}
