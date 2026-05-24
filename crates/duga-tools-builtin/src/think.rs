//! ThinkTool — echo thought + think limit enforcement.

use duga_tools::context::ToolContext;
use duga_tools::result::ToolCallResult;
use duga_tools::Tool;
use duga_types::error::ToolError;
use duga_types::tool_call::CallId;
use duga_types::tool_result::ToolResult;
use schemars::JsonSchema;
use serde::Deserialize;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ThinkArgs {
    #[schemars(
        description = "Brief human-readable description of what this step does (shown to user)"
    )]
    pub label: String,
    pub thought: String,
}

#[derive(Debug, Clone, Default)]
pub struct ThinkLimits {
    pub max_calls: usize,
    pub max_tokens: usize,
}

#[derive(Clone)]
pub struct ThinkTool {
    call_count: Arc<AtomicUsize>,
    token_count: Arc<AtomicUsize>,
    limits: ThinkLimits,
}

impl std::fmt::Debug for ThinkTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ThinkTool")
            .field("calls", &self.call_count.load(Ordering::SeqCst))
            .field("tokens", &self.token_count.load(Ordering::SeqCst))
            .field("limits", &self.limits)
            .finish()
    }
}

impl ThinkTool {
    pub fn new(limits: ThinkLimits) -> Self {
        Self {
            call_count: Arc::new(AtomicUsize::new(0)),
            token_count: Arc::new(AtomicUsize::new(0)),
            limits,
        }
    }
    fn estimate_tokens(text: &str) -> usize {
        (text.split_whitespace().count() as f64 * 1.3).ceil() as usize
    }
}

impl Default for ThinkTool {
    fn default() -> Self {
        Self::new(ThinkLimits {
            max_calls: 8,
            max_tokens: 4096,
        })
    }
}

impl Tool for ThinkTool {
    type Args = ThinkArgs;
    fn name(&self) -> &str {
        "think"
    }
    fn reset_limits(&self) {
        self.call_count.store(0, Ordering::SeqCst);
        self.token_count.store(0, Ordering::SeqCst);
    }
    fn description(&self) -> &str {
        "Think through a problem step by step"
    }
    fn retryable(&self) -> bool {
        false
    }

    async fn execute(&self, _ctx: ToolContext<'_>, args: Self::Args) -> ToolCallResult {
        let start = std::time::Instant::now();
        let prev_calls = self.call_count.fetch_add(1, Ordering::SeqCst);
        if prev_calls >= self.limits.max_calls {
            self.call_count.fetch_sub(1, Ordering::SeqCst);
            return Err(ToolError::Denied("think call limit reached".into()));
        }
        let est = Self::estimate_tokens(&args.thought);
        let prev_tok = self.token_count.fetch_add(est, Ordering::SeqCst);
        if prev_tok + est > self.limits.max_tokens {
            self.call_count.fetch_sub(1, Ordering::SeqCst);
            self.token_count.fetch_sub(est, Ordering::SeqCst);
            return Err(ToolError::Denied("think token limit reached".into()));
        }
        Ok(ToolResult {
            tool_call_id: CallId::new(),
            success: true,
            output: args.thought,
            metadata: serde_json::json!({"estimated_tokens": est}),
            duration_ms: start.elapsed().as_millis().min(u64::MAX as u128) as u64,
            stdout_bytes: 0,
            stderr_bytes: 0,
            truncated: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use duga_sandbox::exec::CancellationToken;
    use duga_sandbox::Workspace;
    use duga_tools::event_sink::NullSink;
    use tempfile::tempdir;

    fn make_ctx(ws: &Workspace) -> ToolContext<'static> {
        let ws: &'static Workspace = unsafe { std::mem::transmute(ws) };
        ToolContext {
            workspace: ws,
            cancellation: CancellationToken::new(),
            event_sink: &NullSink,
            aux_root: None,
        }
    }

    #[test]
    fn test_think_basic() {
        let dir = tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let tool = ThinkTool::new(ThinkLimits {
            max_calls: 10,
            max_tokens: 1000,
        });
        let rt = tokio::runtime::Runtime::new().unwrap();
        let r = rt
            .block_on(tool.execute(
                make_ctx(&ws),
                ThinkArgs {
                    label: "Thinking about hello".into(),
                    thought: "hello".into(),
                },
            ))
            .unwrap();
        assert_eq!(r.output, "hello");
    }

    #[test]
    fn test_think_limit() {
        let dir = tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let tool = ThinkTool::new(ThinkLimits {
            max_calls: 1,
            max_tokens: 1000,
        });
        let rt = tokio::runtime::Runtime::new().unwrap();
        assert!(rt
            .block_on(tool.execute(
                make_ctx(&ws),
                ThinkArgs {
                    label: "Thinking about a".into(),
                    thought: "a".into()
                }
            ))
            .is_ok());
        assert!(rt
            .block_on(tool.execute(
                make_ctx(&ws),
                ThinkArgs {
                    label: "Thinking about b".into(),
                    thought: "b".into()
                }
            ))
            .is_err());
    }

    #[test]
    fn test_think_limit_reset_restores_budget() {
        // After hitting the call limit and resetting, the tool works again.
        let dir = tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let tool = ThinkTool::new(ThinkLimits {
            max_calls: 1,
            max_tokens: 1000,
        });
        let rt = tokio::runtime::Runtime::new().unwrap();

        // First call: OK.
        rt.block_on(tool.execute(
            make_ctx(&ws),
            ThinkArgs {
                label: "First".into(),
                thought: "first".into(),
            },
        ))
        .unwrap();

        // Second call: fails (limit 1).
        assert!(rt
            .block_on(tool.execute(
                make_ctx(&ws),
                ThinkArgs {
                    label: "Second".into(),
                    thought: "second".into(),
                },
            ))
            .is_err());

        // Reset.
        tool.reset_limits();

        // After reset: works again.
        assert!(rt
            .block_on(tool.execute(
                make_ctx(&ws),
                ThinkArgs {
                    label: "Third after reset".into(),
                    thought: "third".into(),
                },
            ))
            .is_ok());
    }

    #[test]
    fn test_think_token_limit_reset_restores_budget() {
        // Token limits also reset.
        let dir = tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        // 5 tokens max, heuristic is word_count * 1.3 (ceil).
        // 3 words → 4 tokens; 4 words → 6 tokens.
        let tool = ThinkTool::new(ThinkLimits {
            max_calls: 10,
            max_tokens: 5,
        });
        let rt = tokio::runtime::Runtime::new().unwrap();

        // First call: 3 words → 4 tokens, fits.
        rt.block_on(tool.execute(
            make_ctx(&ws),
            ThinkArgs {
                label: "Small thought".into(),
                thought: "one two three".into(),
            },
        ))
        .unwrap();

        // Second call: 2 more words → 3 tokens, 4+3=7 > 5, fails.
        assert!(rt
            .block_on(tool.execute(
                make_ctx(&ws),
                ThinkArgs {
                    label: "Another".into(),
                    thought: "four five".into(),
                },
            ))
            .is_err());

        // Reset.
        tool.reset_limits();

        // After reset: works again.
        assert!(rt
            .block_on(tool.execute(
                make_ctx(&ws),
                ThinkArgs {
                    label: "After reset".into(),
                    thought: "fresh start".into(),
                },
            ))
            .is_ok());
    }
}
