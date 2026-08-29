use std::collections::BTreeMap;
use std::fmt;
use std::ops::Range;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct WorkspaceRevision(u64);

impl WorkspaceRevision {
    pub fn get(self) -> u64 {
        self.0
    }

    pub fn from_raw(value: u64) -> Self {
        Self(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
    pub line: u32,
    pub utf16_character: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextChange {
    pub range: Option<Range<usize>>,
    pub text: String,
}

impl TextChange {
    pub fn replace(range: Range<usize>, text: impl Into<String>) -> Self {
        Self {
            range: Some(range),
            text: text.into(),
        }
    }

    pub fn replace_all(text: impl Into<String>) -> Self {
        Self {
            range: None,
            text: text.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    pub version: Option<i64>,
    pub text: Arc<str>,
    line_starts: Arc<[usize]>,
}

impl Document {
    fn disk(text: String) -> Self {
        Self::new(None, text)
    }

    pub(super) fn overlay(version: i64, text: String) -> Self {
        Self::new(Some(version), text)
    }

    fn new(version: Option<i64>, text: String) -> Self {
        let mut line_starts = vec![0];
        line_starts.extend(
            text.bytes()
                .enumerate()
                .filter(|(_, byte)| *byte == b'\n')
                .map(|(offset, _)| offset + 1),
        );
        Self {
            version,
            text: text.into(),
            line_starts: line_starts.into(),
        }
    }

    pub fn byte_offset(&self, position: Position) -> Result<usize, PositionError> {
        let line_index = position.line as usize;
        let line_start = self
            .line_starts
            .get(line_index)
            .copied()
            .ok_or(PositionError::LineOutOfBounds(position.line))?;
        let line_end = self
            .line_starts
            .get(line_index + 1)
            .map_or(self.text.len(), |next_start| next_start - 1);
        let line = &self.text[line_start..line_end];
        let mut utf16 = 0u32;
        for (relative, character) in line.char_indices() {
            if utf16 == position.utf16_character {
                return Ok(line_start + relative);
            }
            utf16 = utf16.saturating_add(character.len_utf16() as u32);
            if utf16 > position.utf16_character {
                return Err(PositionError::InsideUtf16Character {
                    line: position.line,
                    utf16_character: position.utf16_character,
                });
            }
        }
        if utf16 == position.utf16_character {
            Ok(line_end)
        } else {
            Err(PositionError::CharacterOutOfBounds {
                line: position.line,
                utf16_character: position.utf16_character,
            })
        }
    }

    pub fn position(&self, byte_offset: usize) -> Result<Position, PositionError> {
        if byte_offset > self.text.len() || !self.text.is_char_boundary(byte_offset) {
            return Err(PositionError::InvalidByteOffset(byte_offset));
        }
        let line = self
            .line_starts
            .partition_point(|start| *start <= byte_offset)
            - 1;
        let line_start = self.line_starts[line];
        let utf16_character = self.text[line_start..byte_offset].encode_utf16().count() as u32;
        Ok(Position {
            line: line as u32,
            utf16_character,
        })
    }
}

#[derive(Debug, Clone)]
pub struct WorkspaceSnapshot {
    revision: WorkspaceRevision,
    documents: Arc<BTreeMap<String, Document>>,
}

impl WorkspaceSnapshot {
    pub fn revision(&self) -> WorkspaceRevision {
        self.revision
    }

    pub fn document(&self, path: &str) -> Option<&Document> {
        self.documents.get(path)
    }

    pub fn documents(&self) -> impl ExactSizeIterator<Item = (&str, &Document)> {
        self.documents
            .iter()
            .map(|(path, document)| (path.as_str(), document))
    }
}

#[derive(Debug, Clone)]
pub struct WorkspaceDocuments {
    revision: WorkspaceRevision,
    disk: BTreeMap<String, Arc<str>>,
    overlays: BTreeMap<String, Document>,
    snapshot: WorkspaceSnapshot,
}

impl Default for WorkspaceDocuments {
    fn default() -> Self {
        let revision = WorkspaceRevision(0);
        Self {
            revision,
            disk: BTreeMap::new(),
            overlays: BTreeMap::new(),
            snapshot: WorkspaceSnapshot {
                revision,
                documents: Arc::new(BTreeMap::new()),
            },
        }
    }
}

impl WorkspaceDocuments {
    pub fn snapshot(&self) -> WorkspaceSnapshot {
        self.snapshot.clone()
    }

    pub fn set_disk_document(&mut self, path: impl Into<String>, text: impl Into<String>) {
        let path = path.into();
        let text = text.into();
        if self
            .disk
            .get(&path)
            .is_some_and(|current| current.as_ref() == text)
        {
            return;
        }
        self.disk.insert(path, text.into());
        self.publish();
    }

    pub fn remove_disk_document(&mut self, path: &str) {
        if self.disk.remove(path).is_some() {
            self.publish();
        }
    }

    pub fn open_document(
        &mut self,
        path: impl Into<String>,
        version: i64,
        text: impl Into<String>,
    ) {
        self.overlays
            .insert(path.into(), Document::overlay(version, text.into()));
        self.publish();
    }

    pub fn change_document(
        &mut self,
        path: &str,
        version: i64,
        changes: &[TextChange],
    ) -> Result<(), DocumentChangeError> {
        let document = self
            .overlays
            .get(path)
            .ok_or_else(|| DocumentChangeError::NotOpen(path.to_string()))?;
        let current_version = document
            .version
            .expect("open document always has a version");
        if version <= current_version {
            return Err(DocumentChangeError::StaleVersion {
                path: path.to_string(),
                current: current_version,
                requested: version,
            });
        }
        let mut next = document.text.to_string();
        for change in changes {
            match &change.range {
                Some(range)
                    if range.start <= range.end
                        && range.end <= next.len()
                        && next.is_char_boundary(range.start)
                        && next.is_char_boundary(range.end) =>
                {
                    next.replace_range(range.clone(), &change.text);
                }
                Some(range) => {
                    return Err(DocumentChangeError::InvalidRange {
                        path: path.to_string(),
                        start: range.start,
                        end: range.end,
                        length: next.len(),
                    });
                }
                None => next = change.text.clone(),
            }
        }
        self.overlays
            .insert(path.to_string(), Document::overlay(version, next));
        self.publish();
        Ok(())
    }

    pub fn close_document(&mut self, path: &str) {
        if self.overlays.remove(path).is_some() {
            self.publish();
        }
    }

    fn publish(&mut self) {
        let mut documents = self
            .disk
            .iter()
            .map(|(path, text)| (path.clone(), Document::disk(text.to_string())))
            .collect::<BTreeMap<_, _>>();
        documents.extend(self.overlays.clone());
        self.revision = WorkspaceRevision(
            self.revision
                .get()
                .checked_add(1)
                .expect("workspace revision overflow"),
        );
        self.snapshot = WorkspaceSnapshot {
            revision: self.revision,
            documents: Arc::new(documents),
        };
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum DocumentChangeError {
    NotOpen(String),
    StaleVersion {
        path: String,
        current: i64,
        requested: i64,
    },
    InvalidRange {
        path: String,
        start: usize,
        end: usize,
        length: usize,
    },
}

impl fmt::Display for DocumentChangeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotOpen(path) => write!(formatter, "document '{path}' is not open"),
            Self::StaleVersion {
                path,
                current,
                requested,
            } => write!(
                formatter,
                "document '{path}' change version {requested} is not newer than {current}"
            ),
            Self::InvalidRange {
                path,
                start,
                end,
                length,
            } => write!(
                formatter,
                "document '{path}' byte range {start}..{end} is invalid for {length} bytes"
            ),
        }
    }
}

impl std::error::Error for DocumentChangeError {}

#[derive(Debug, PartialEq, Eq)]
pub enum PositionError {
    LineOutOfBounds(u32),
    InsideUtf16Character { line: u32, utf16_character: u32 },
    CharacterOutOfBounds { line: u32, utf16_character: u32 },
    InvalidByteOffset(usize),
}

impl fmt::Display for PositionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LineOutOfBounds(line) => write!(formatter, "line {line} is outside the document"),
            Self::InsideUtf16Character {
                line,
                utf16_character,
            } => write!(
                formatter,
                "UTF-16 position {line}:{utf16_character} splits a Unicode character"
            ),
            Self::CharacterOutOfBounds {
                line,
                utf16_character,
            } => write!(
                formatter,
                "UTF-16 position {line}:{utf16_character} is outside the line"
            ),
            Self::InvalidByteOffset(offset) => write!(
                formatter,
                "byte offset {offset} is outside the document or splits a Unicode character"
            ),
        }
    }
}

impl std::error::Error for PositionError {}
