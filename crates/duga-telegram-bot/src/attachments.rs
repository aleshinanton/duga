//! Telegram attachment downloading.

use anyhow::Result;
use duga_config::TelegramConfig;
use std::path::{Path, PathBuf};
use teloxide::net::Download;
use teloxide::prelude::*;
use teloxide::types::Message;

#[derive(Clone, Debug)]
pub struct DownloadedFile {
    pub kind: String,
    pub file_id: String,
    pub path: PathBuf,
}

/// Download all supported attachments from a message.
pub async fn download_attachments(
    bot: &Bot,
    msg: &Message,
    save_dir: &Path,
    config: &TelegramConfig,
) -> Result<Vec<DownloadedFile>> {
    let mut files = Vec::new();
    let max_size = config.attachments.max_file_size_mb * 1024 * 1024;

    // Photos (use the largest available).
    if let Some(photo_sizes) = msg.photo() {
        if let Some(largest) = photo_sizes.last() {
            let size_ok = (largest.file.size as u64) <= max_size;
            if size_ok {
                if let Ok(tg_file) = bot.get_file(largest.file.id.clone()).await {
                    let filename = format!("photo_{}.jpg", msg.id.0);
                    let path = save_dir.join(&filename);
                    download_file(bot, &tg_file, &path).await?;
                    files.push(DownloadedFile {
                        kind: "photo".into(),
                        file_id: largest.file.id.to_string(),
                        path,
                    });
                }
            }
        }
    }

    // Document.
    if let Some(doc) = msg.document() {
        if (doc.file.size as u64) <= max_size {
            if let Ok(tg_file) = bot.get_file(doc.file.id.clone()).await {
                let name = doc
                    .file_name
                    .as_deref()
                    .unwrap_or("document")
                    .replace('/', "_")
                    .replace('\\', "_");
                let path =
                    save_dir.join(format!("{}_{}", msg.id.0, sanitize_filename(&name)));
                download_file(bot, &tg_file, &path).await?;
                files.push(DownloadedFile {
                    kind: "document".into(),
                    file_id: doc.file.id.to_string(),
                    path,
                });
            }
        }
    }

    // Audio.
    if let Some(audio) = msg.audio() {
        if (audio.file.size as u64) <= max_size {
            if let Ok(tg_file) = bot.get_file(audio.file.id.clone()).await {
                let name = audio
                    .file_name
                    .as_deref()
                    .unwrap_or("audio")
                    .replace('/', "_");
                let path =
                    save_dir.join(format!("{}_{}", msg.id.0, sanitize_filename(&name)));
                download_file(bot, &tg_file, &path).await?;
                files.push(DownloadedFile {
                    kind: "audio".into(),
                    file_id: audio.file.id.to_string(),
                    path,
                });
            }
        }
    }

    // Voice.
    if let Some(voice) = msg.voice() {
        if (voice.file.size as u64) <= max_size {
            if let Ok(tg_file) = bot.get_file(voice.file.id.clone()).await {
                let path = save_dir.join(format!("voice_{}.ogg", msg.id.0));
                download_file(bot, &tg_file, &path).await?;
                files.push(DownloadedFile {
                    kind: "voice".into(),
                    file_id: voice.file.id.to_string(),
                    path,
                });
            }
        }
    }

    // Video.
    if let Some(video) = msg.video() {
        if (video.file.size as u64) <= max_size {
            if let Ok(tg_file) = bot.get_file(video.file.id.clone()).await {
                let name = video
                    .file_name
                    .as_deref()
                    .unwrap_or("video")
                    .replace('/', "_");
                let path =
                    save_dir.join(format!("{}_{}", msg.id.0, sanitize_filename(&name)));
                download_file(bot, &tg_file, &path).await?;
                files.push(DownloadedFile {
                    kind: "video".into(),
                    file_id: video.file.id.to_string(),
                    path,
                });
            }
        }
    }

    Ok(files)
}

async fn download_file(bot: &Bot, file: &teloxide::types::File, path: &Path) -> Result<()> {
    let mut dest = tokio::fs::File::create(&path).await?;
    bot.download_file(&file.path, &mut dest).await?;
    Ok(())
}

pub fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Generate a human-readable summary of downloaded attachments.
pub fn summarize_attachments(files: &[DownloadedFile]) -> String {
    if files.is_empty() {
        return String::new();
    }
    let mut lines = Vec::new();
    for f in files {
        let name = f.path.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let size = std::fs::metadata(&f.path)
            .map(|m| m.len())
            .unwrap_or(0);
        let size_str = if size < 1024 {
            format!("{} B", size)
        } else if size < 1024 * 1024 {
            format!("{:.1} KB", size as f64 / 1024.0)
        } else {
            format!("{:.1} MB", size as f64 / (1024.0 * 1024.0))
        };
        lines.push(format!("- {} ({}, {})", name, f.kind, size_str));
    }
    format!("[Attachments:\n{}\n]", lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_sanitize_filename() {
        assert_eq!(sanitize_filename("hello world.txt"), "hello_world.txt");
        assert_eq!(sanitize_filename("path/to/file"), "path_to_file");
        assert_eq!(sanitize_filename("normal-name.txt"), "normal-name.txt");
    }

    #[test]
    fn test_summarize_attachments_empty() {
        let summary = summarize_attachments(&[]);
        assert!(summary.is_empty());
    }

    #[test]
    fn test_summarize_attachments_single() {
        let dir = tempfile::TempDir::new().unwrap();
        let file_path = dir.path().join("photo_123.jpg");
        let mut f = std::fs::File::create(&file_path).unwrap();
        f.write_all(&[0u8; 1024]).unwrap();
        drop(f);

        let files = vec![DownloadedFile {
            kind: "photo".into(),
            file_id: "abc123".into(),
            path: file_path,
        }];
        let summary = summarize_attachments(&files);
        assert!(summary.starts_with("[Attachments:"));
        assert!(summary.contains("photo_123.jpg"));
        assert!(summary.contains("(photo"));
        assert!(summary.contains("1.0 KB"));
        assert!(summary.ends_with("]"));
    }

    #[test]
    fn test_summarize_attachments_multiple() {
        let dir = tempfile::TempDir::new().unwrap();
        let file1 = dir.path().join("doc.pdf");
        let file2 = dir.path().join("audio.mp3");
        std::fs::write(&file1, b"pdf content").unwrap();
        std::fs::write(&file2, b"audio content").unwrap();

        let files = vec![
            DownloadedFile {
                kind: "document".into(),
                file_id: "doc123".into(),
                path: file1,
            },
            DownloadedFile {
                kind: "audio".into(),
                file_id: "aud456".into(),
                path: file2,
            },
        ];
        let summary = summarize_attachments(&files);
        assert!(summary.contains("doc.pdf"));
        assert!(summary.contains("audio.mp3"));
        assert!(summary.contains("(document"));
        assert!(summary.contains("(audio"));
        // Should have two lines
        assert_eq!(summary.matches("- ").count(), 2);
    }

    #[test]
    fn test_summarize_attachments_missing_file() {
        let files = vec![DownloadedFile {
            kind: "document".into(),
            file_id: "doc123".into(),
            path: PathBuf::from("/nonexistent/file.pdf"),
        }];
        // Should not panic; size defaults to 0 B
        let summary = summarize_attachments(&files);
        assert!(summary.contains("file.pdf"));
        assert!(summary.contains("0 B"));
    }
}
