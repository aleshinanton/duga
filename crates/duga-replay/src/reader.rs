use duga_events::StoredEvent;
use serde_json::Value;
use std::io::BufRead;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum ReplayError {
    #[error("replay file not found: {0}")]
    FileNotFound(PathBuf),
    #[error("invalid json at line {line}: {error}")]
    InvalidJson { line: usize, error: String },
    #[error("missing field '{field}' at line {line}")]
    MissingField { line: usize, field: String },
}

pub struct JsonlReader;

impl JsonlReader {
    pub fn read(path: impl AsRef<Path>) -> Result<Vec<StoredEvent>, ReplayError> {
        let path = path.as_ref();
        let file =
            std::fs::File::open(path).map_err(|_| ReplayError::FileNotFound(path.to_path_buf()))?;
        let reader = std::io::BufReader::new(file);
        let mut events = Vec::new();

        for (index, line) in reader.lines().enumerate() {
            let line_no = index + 1;
            let line = line.map_err(|error| ReplayError::InvalidJson {
                line: line_no,
                error: error.to_string(),
            })?;
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }

            let value: Value =
                serde_json::from_str(trimmed).map_err(|error| ReplayError::InvalidJson {
                    line: line_no,
                    error: error.to_string(),
                })?;
            require_field(&value, line_no, "seq")?;
            if value.get("ts").is_none() && value.get("timestamp_ms").is_none() {
                return Err(ReplayError::MissingField {
                    line: line_no,
                    field: "ts".into(),
                });
            }
            require_field(&value, line_no, "type")?;

            let event =
                serde_json::from_str(trimmed).map_err(|error| ReplayError::InvalidJson {
                    line: line_no,
                    error: error.to_string(),
                })?;
            events.push(event);
        }

        Ok(events)
    }
}

fn require_field(value: &Value, line: usize, field: &str) -> Result<(), ReplayError> {
    if value.get(field).is_some() {
        Ok(())
    } else {
        Err(ReplayError::MissingField {
            line,
            field: field.into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use duga_events::{Event, StoredEvent};

    #[test]
    fn reads_valid_jsonl_and_skips_comments() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        let event = StoredEvent::new(
            1,
            Event::AgentFinished {
                text: Some("ok".into()),
            },
        );
        std::fs::write(
            &path,
            format!("# comment\n{}\n\n", serde_json::to_string(&event).unwrap()),
        )
        .unwrap();

        let events = JsonlReader::read(&path).unwrap();

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, event.event);
    }

    #[test]
    fn reports_corrupt_line_number() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        std::fs::write(&path, "{}\nnot json\n").unwrap();

        let err = JsonlReader::read(&path).unwrap_err();

        assert_eq!(
            err,
            ReplayError::MissingField {
                line: 1,
                field: "seq".into()
            }
        );
    }
}
