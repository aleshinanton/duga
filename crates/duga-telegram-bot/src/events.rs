//! Scheduled event handling for Telegram.
//!
//! Watches the `events/` directory for immediate, one-shot, and periodic
//! event files. Each event triggers an agent run in the specified chat.

use anyhow::{Context, Result};
use chrono::{DateTime, FixedOffset, Utc};
use cron::Schedule;
use serde::Deserialize;
use std::path::Path;
use std::str::FromStr;


/// Event types supported by the watcher.
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ScheduledEvent {
    /// Run immediately once, then delete the file.
    Immediate {
        #[serde(rename = "channelId")]
        channel_id: i64,
        text: String,
    },
    /// Run at a specific date-time, then delete the file.
    OneShot {
        #[serde(rename = "channelId")]
        channel_id: i64,
        text: String,
        at: String,
    },
    /// Run on a cron schedule. File is kept.
    Periodic {
        #[serde(rename = "channelId")]
        channel_id: i64,
        text: String,
        schedule: String,
        #[serde(default)]
        timezone: Option<String>,
    },
}

/// Scan the events directory and trigger any due events.
pub async fn scan_and_trigger<F>(
    events_dir: &Path,
    mut trigger: F,
) -> Result<usize>
where
    F: FnMut(i64, String),
{
    let mut triggered = 0;

    let mut entries = tokio::fs::read_dir(events_dir)
        .await
        .with_context(|| format!("reading events directory {}", events_dir.display()))?;

    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.extension().map_or(true, |ext| ext != "json") {
            continue;
        }

        let raw = tokio::fs::read_to_string(&path).await?;
        let event: ScheduledEvent = match serde_json::from_str(&raw) {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!("invalid event file {}: {e}", path.display());
                continue;
            }
        };

        match event {
            ScheduledEvent::Immediate { channel_id, text } => {
                trigger(channel_id, text);
                tokio::fs::remove_file(&path).await?;
                triggered += 1;
            }
            ScheduledEvent::OneShot {
                channel_id,
                text,
                at,
            } => {
                let at_time = match DateTime::<FixedOffset>::parse_from_rfc3339(&at) {
                    Ok(dt) => dt,
                    Err(e) => {
                        tracing::warn!("invalid datetime in event {}: {e}", path.display());
                        continue;
                    }
                };

                let now = Utc::now();
                if at_time <= now {
                    trigger(channel_id, text);
                    tokio::fs::remove_file(&path).await?;
                    triggered += 1;
                }
            }
            ScheduledEvent::Periodic {
                channel_id,
                text,
                schedule,
                ..
            } => {
                let cron_schedule = match Schedule::from_str(&schedule) {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::warn!("invalid cron schedule in event {}: {e}", path.display());
                        continue;
                    }
                };

                let now = Utc::now();
                // Check if the event should have triggered since our last check.
                if let Some(next) = cron_schedule.upcoming(Utc).next() {
                    // Trigger if we're within 30 seconds of the scheduled time.
                    let diff = (next - now).num_seconds().abs();
                    if diff <= 30 {
                        trigger(channel_id, text);
                        triggered += 1;
                    }
                }
            }
        }
    }

    Ok(triggered)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_immediate_event() {
        let json = r#"{"type": "immediate", "channelId": -123, "text": "Hello"}"#;
        let event: ScheduledEvent = serde_json::from_str(json).unwrap();
        match event {
            ScheduledEvent::Immediate {
                channel_id,
                text,
            } => {
                assert_eq!(channel_id, -123);
                assert_eq!(text, "Hello");
            }
            _ => panic!("expected Immediate"),
        }
    }

    #[test]
    fn test_parse_periodic_event() {
        let json = r#"{"type": "periodic", "channelId": -456, "text": "Check inbox", "schedule": "0 9 * * 1-5", "timezone": "Europe/Lisbon"}"#;
        let event: ScheduledEvent = serde_json::from_str(json).unwrap();
        match event {
            ScheduledEvent::Periodic {
                channel_id,
                text,
                schedule,
                ..
            } => {
                assert_eq!(channel_id, -456);
                assert_eq!(text, "Check inbox");
                assert_eq!(schedule, "0 9 * * 1-5");
            }
            _ => panic!("expected Periodic"),
        }
    }
}
