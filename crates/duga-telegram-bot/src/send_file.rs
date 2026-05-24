//! SendFileTool — sends workspace files as Telegram attachments.
//!
//! This tool reads a file from the workspace and sends it to the current
//! Telegram chat as the appropriate attachment type (document, photo, audio,
//! video, or animation) based on the file extension.

use duga_config::TelegramSendFileConfig;
use duga_sandbox::Workspace;
use duga_tools::context::ToolContext;
use duga_tools::result::ToolCallResult;
use duga_tools::Tool;
use duga_types::error::ToolError;
use duga_types::tool_call::CallId;
use duga_types::tool_result::ToolResult;
use schemars::JsonSchema;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::ChatId;

/// Which Telegram send method to use for a given file.
#[derive(Debug, Clone, PartialEq)]
pub enum SendMethod {
    Document,
    Photo,
    Audio,
    Video,
    Animation,
    Voice,
}

/// Arguments for the `send_file` tool.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SendFileArgs {
    #[schemars(description = "Brief human-readable description of what this step does (shown to user)")]
    pub label: String,
    /// Path to the file in the workspace to send.
    pub path: String,
    /// Optional caption text (up to 1024 characters).
    #[serde(default)]
    pub caption: Option<String>,
    /// If true, attempt to send image files as photos instead of documents.
    #[serde(default)]
    pub as_photo: Option<bool>,
}

/// Tool that sends workspace files as Telegram attachments.
pub struct SendFileTool {
    bot: Bot,
    chat_id: ChatId,
    config: TelegramSendFileConfig,
    workspace: Arc<Workspace>,
}

impl SendFileTool {
    pub fn new(
        bot: Bot,
        chat_id: ChatId,
        config: TelegramSendFileConfig,
        workspace: Arc<Workspace>,
    ) -> Self {
        Self {
            bot,
            chat_id,
            config,
            workspace,
        }
    }

    /// Determine the send method based on file extension and the `as_photo` hint.
    fn get_send_method(path: &Path, as_photo: bool) -> SendMethod {
        match path.extension().and_then(|e| e.to_str()).map(|e| e.to_lowercase()).as_deref() {
            Some("jpg") | Some("jpeg") | Some("png") | Some("webp") => {
                if as_photo {
                    SendMethod::Photo
                } else {
                    SendMethod::Document
                }
            }
            Some("mp3") | Some("flac") | Some("m4a") | Some("wav") => SendMethod::Audio,
            Some("mp4") | Some("mov") | Some("webm") | Some("avi") | Some("mkv") => SendMethod::Video,
            Some("gif") => SendMethod::Animation,
            Some("ogg") => SendMethod::Voice,
            _ => SendMethod::Document,
        }
    }

    /// Describe the send method as a human-readable string.
    fn send_method_name(method: &SendMethod) -> &'static str {
        match method {
            SendMethod::Document => "document",
            SendMethod::Photo => "photo",
            SendMethod::Audio => "audio",
            SendMethod::Video => "video",
            SendMethod::Animation => "animation",
            SendMethod::Voice => "voice",
        }
    }
}

impl SendFileTool {
    /// Resolve a path (absolute or relative) within the workspace.
    ///
    /// LLMs running inside the Docker sandbox often pass absolute paths like
    /// `/workspace/chess.svg` (the sandbox mount point). On the host, the file
    /// actually lives at `{workspace_root}/chess.svg`, so we strip known
    /// sandbox mount prefixes before resolving.
    fn resolve_workspace_path(&self, path: &str) -> Result<PathBuf, String> {
        let raw = PathBuf::from(path);

        // First, try normal resolution (works for relative paths).
        if let Ok(resolved) = self.workspace.resolve(&raw) {
            return Ok(resolved);
        }

        // Absolute path: try stripping the workspace root prefix directly.
        if raw.is_absolute() {
            if let Ok(relative) = raw.strip_prefix(self.workspace.root_path()) {
                return self
                    .workspace
                    .resolve(relative)
                    .map_err(|_| format!("path escapes workspace: {}", path));
            }

            // Try canonicalizing — works if the absolute path exists on the host
            // (e.g. if the sandbox mount prefix happens to resolve).
            if let Ok(canonical) = raw.canonicalize() {
                if let Ok(relative) = canonical.strip_prefix(self.workspace.root_path()) {
                    return self
                        .workspace
                        .resolve(relative)
                        .map_err(|_| format!("path escapes workspace: {}", path));
                }
            }

            // Last resort: strip known sandbox mount prefixes.
            // The default Docker sandbox mount is `/workspace`.
            const KNOWN_MOUNT_PREFIXES: &[&str] = &["/workspace/", "/workspace"];
            for prefix in KNOWN_MOUNT_PREFIXES {
                if let Ok(relative) = raw.strip_prefix(prefix) {
                    return self
                        .workspace
                        .resolve(Path::new(relative))
                        .map_err(|_| format!("path escapes workspace: {}", path));
                }
            }
        }

        Err(format!("path escapes workspace: {}", path))
    }
}

impl Tool for SendFileTool {
    type Args = SendFileArgs;

    fn name(&self) -> &str {
        "send_file"
    }

    fn description(&self) -> &str {
        "Send a workspace file to the Telegram chat as a proper attachment (document, photo, audio, video, or animation)."
    }

    async fn execute(&self, ctx: ToolContext<'_>, args: Self::Args) -> ToolCallResult {
        let start = std::time::Instant::now();

        // Resolve the path within the workspace (supports absolute paths).
        let resolved = match self.resolve_workspace_path(&args.path) {
            Ok(p) => p,
            Err(msg) => {
                return Ok(ToolResult {
                    tool_call_id: CallId::new(),
                    success: false,
                    output: format!("Error: {}", msg),
                    metadata: serde_json::json!({"error": "path_escapes_workspace"}),
                    duration_ms: start.elapsed().as_millis() as u64,
                    stdout_bytes: 0,
                    stderr_bytes: 0,
                    truncated: false,
                });
            }
        };

        // Check that the file exists.
        if !self.workspace.is_file(&resolved) {
            return Ok(ToolResult {
                tool_call_id: CallId::new(),
                success: false,
                output: format!("Error: file not found: {}", args.path),
                metadata: serde_json::json!({"error": "file_not_found"}),
                duration_ms: start.elapsed().as_millis() as u64,
                stdout_bytes: 0,
                stderr_bytes: 0,
                truncated: false,
            });
        }

        // Check if tool is enabled.
        if !self.config.enabled {
            return Ok(ToolResult {
                tool_call_id: CallId::new(),
                success: false,
                output: "Error: send_file is disabled in bot configuration.".into(),
                metadata: serde_json::json!({"error": "disabled"}),
                duration_ms: start.elapsed().as_millis() as u64,
                stdout_bytes: 0,
                stderr_bytes: 0,
                truncated: false,
            });
        }

        // Check extension allowlist.
        if !self.config.allowed_extensions.is_empty() {
            let ext = resolved
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| format!(".{}", e.to_lowercase()))
                .unwrap_or_default();
            if !self.config.allowed_extensions.contains(&ext) {
                return Ok(ToolResult {
                    tool_call_id: CallId::new(),
                    success: false,
                    output: format!(
                        "Error: file extension '{}' is not in the allowed list: {:?}",
                        ext, self.config.allowed_extensions
                    ),
                    metadata: serde_json::json!({"error": "extension_not_allowed"}),
                    duration_ms: start.elapsed().as_millis() as u64,
                    stdout_bytes: 0,
                    stderr_bytes: 0,
                    truncated: false,
                });
            }
        }

        // Check file size.
        let file_size = self.workspace.root_dir().metadata(&resolved).ok().map(|m| m.len()).unwrap_or(0);
        let max_bytes = (self.config.max_file_size_mb as u64) * 1024 * 1024;
        if file_size > max_bytes {
            return Ok(ToolResult {
                tool_call_id: CallId::new(),
                success: false,
                output: format!(
                    "Error: file too large ({} bytes, max {} MB)",
                    file_size, self.config.max_file_size_mb
                ),
                metadata: serde_json::json!({"error": "file_too_large", "file_size": file_size}),
                duration_ms: start.elapsed().as_millis() as u64,
                stdout_bytes: 0,
                stderr_bytes: 0,
                truncated: false,
            });
        }

        // Cancellation check before sending.
        if ctx.is_cancelled() {
            return Err(ToolError::Cancelled);
        }

        // Determine the send method.
        let send_method = Self::get_send_method(&resolved, args.as_photo.unwrap_or(false));
        let method_name = Self::send_method_name(&send_method);

        // Build the full workspace path for reading the file.
        let full_path = self.workspace.root_path().join(&resolved);
        let input_file = teloxide::types::InputFile::file(&full_path)
            .file_name(resolved.file_name().unwrap_or_default().to_string_lossy().into_owned());

        // Prepare caption (Telegram limit: 1024 chars).
        let caption = args.caption.as_deref().map(|c| {
            if c.chars().count() > 1024 {
                c.chars().take(1021).collect::<String>() + "…"
            } else {
                c.to_string()
            }
        });

        // Send via the appropriate Telegram API method.
        let result = match send_method {
            SendMethod::Document => {
                let mut req = self.bot.send_document(self.chat_id, input_file);
                if let Some(ref cap) = caption {
                    req = req.caption(cap);
                }
                req.await.map_err(|e| ToolError::Denied(format!("Telegram API error: {e}")))
            }
            SendMethod::Photo => {
                let mut req = self.bot.send_photo(self.chat_id, input_file);
                if let Some(ref cap) = caption {
                    req = req.caption(cap);
                }
                req.await.map_err(|e| ToolError::Denied(format!("Telegram API error: {e}")))
            }
            SendMethod::Audio => {
                let mut req = self.bot.send_audio(self.chat_id, input_file);
                if let Some(ref cap) = caption {
                    req = req.caption(cap);
                }
                req.await.map_err(|e| ToolError::Denied(format!("Telegram API error: {e}")))
            }
            SendMethod::Video => {
                let mut req = self.bot.send_video(self.chat_id, input_file);
                if let Some(ref cap) = caption {
                    req = req.caption(cap);
                }
                req.await.map_err(|e| ToolError::Denied(format!("Telegram API error: {e}")))
            }
            SendMethod::Animation => {
                let mut req = self.bot.send_animation(self.chat_id, input_file);
                if let Some(ref cap) = caption {
                    req = req.caption(cap);
                }
                req.await.map_err(|e| ToolError::Denied(format!("Telegram API error: {e}")))
            }
            SendMethod::Voice => {
                let mut req = self.bot.send_voice(self.chat_id, input_file);
                if let Some(ref cap) = caption {
                    req = req.caption(cap);
                }
                req.await.map_err(|e| ToolError::Denied(format!("Telegram API error: {e}")))
            }
        };

        match result {
            Ok(_msg) => {
                let duration = start.elapsed().as_millis() as u64;
                let file_label = resolved.file_name()
                    .unwrap_or_default()
                    .to_string_lossy();
                let size_label = if file_size < 1024 {
                    format!("{} B", file_size)
                } else if file_size < 1024 * 1024 {
                    format!("{:.1} KB", file_size as f64 / 1024.0)
                } else {
                    format!("{:.1} MB", file_size as f64 / (1024.0 * 1024.0))
                };
                Ok(ToolResult {
                    tool_call_id: CallId::new(),
                    success: true,
                    output: format!("Sent `{}` as {} ({})", file_label, method_name, size_label),
                    metadata: serde_json::json!({
                        "file": file_label.to_string(),
                        "send_method": method_name,
                        "file_size_bytes": file_size,
                        "caption": caption,
                    }),
                    duration_ms: duration,
                    stdout_bytes: 0,
                    stderr_bytes: 0,
                    truncated: false,
                })
            }
            Err(e) => {
                Ok(ToolResult {
                    tool_call_id: CallId::new(),
                    success: false,
                    output: format!("Error sending file: {e}"),
                    metadata: serde_json::json!({"error": e.to_string()}),
                    duration_ms: start.elapsed().as_millis() as u64,
                    stdout_bytes: 0,
                    stderr_bytes: 0,
                    truncated: false,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_get_send_method_document() {
        // Default for images without as_photo
        assert_eq!(
            SendFileTool::get_send_method(Path::new("image.jpg"), false),
            SendMethod::Document
        );
        assert_eq!(
            SendFileTool::get_send_method(Path::new("doc.pdf"), false),
            SendMethod::Document
        );
        assert_eq!(
            SendFileTool::get_send_method(Path::new("code.rs"), false),
            SendMethod::Document
        );
        assert_eq!(
            SendFileTool::get_send_method(Path::new("data.json"), false),
            SendMethod::Document
        );
        assert_eq!(
            SendFileTool::get_send_method(Path::new("diagram.svg"), false),
            SendMethod::Document
        );
    }

    #[test]
    fn test_get_send_method_photo() {
        assert_eq!(
            SendFileTool::get_send_method(Path::new("photo.jpg"), true),
            SendMethod::Photo
        );
        assert_eq!(
            SendFileTool::get_send_method(Path::new("image.png"), true),
            SendMethod::Photo
        );
        assert_eq!(
            SendFileTool::get_send_method(Path::new("pic.webp"), true),
            SendMethod::Photo
        );
        assert_eq!(
            SendFileTool::get_send_method(Path::new("photo.JPEG"), true),
            SendMethod::Photo
        );
    }

    #[test]
    fn test_get_send_method_audio() {
        assert_eq!(
            SendFileTool::get_send_method(Path::new("song.mp3"), false),
            SendMethod::Audio
        );
        assert_eq!(
            SendFileTool::get_send_method(Path::new("track.flac"), false),
            SendMethod::Audio
        );
        assert_eq!(
            SendFileTool::get_send_method(Path::new("recording.m4a"), false),
            SendMethod::Audio
        );
        assert_eq!(
            SendFileTool::get_send_method(Path::new("audio.wav"), false),
            SendMethod::Audio
        );
    }

    #[test]
    fn test_get_send_method_video() {
        assert_eq!(
            SendFileTool::get_send_method(Path::new("video.mp4"), false),
            SendMethod::Video
        );
        assert_eq!(
            SendFileTool::get_send_method(Path::new("movie.mov"), false),
            SendMethod::Video
        );
        assert_eq!(
            SendFileTool::get_send_method(Path::new("clip.webm"), false),
            SendMethod::Video
        );
        assert_eq!(
            SendFileTool::get_send_method(Path::new("vid.avi"), false),
            SendMethod::Video
        );
        assert_eq!(
            SendFileTool::get_send_method(Path::new("vid.mkv"), false),
            SendMethod::Video
        );
    }

    #[test]
    fn test_get_send_method_animation() {
        assert_eq!(
            SendFileTool::get_send_method(Path::new("animation.gif"), false),
            SendMethod::Animation
        );
        assert_eq!(
            SendFileTool::get_send_method(Path::new("animated.GIF"), false),
            SendMethod::Animation
        );
    }

    #[test]
    fn test_get_send_method_voice() {
        assert_eq!(
            SendFileTool::get_send_method(Path::new("voice.ogg"), false),
            SendMethod::Voice
        );
    }

    #[test]
    fn test_send_method_name() {
        assert_eq!(SendFileTool::send_method_name(&SendMethod::Document), "document");
        assert_eq!(SendFileTool::send_method_name(&SendMethod::Photo), "photo");
        assert_eq!(SendFileTool::send_method_name(&SendMethod::Audio), "audio");
        assert_eq!(SendFileTool::send_method_name(&SendMethod::Video), "video");
        assert_eq!(SendFileTool::send_method_name(&SendMethod::Animation), "animation");
        assert_eq!(SendFileTool::send_method_name(&SendMethod::Voice), "voice");
    }
}
