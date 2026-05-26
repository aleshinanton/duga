//! Agent runtime integration for Telegram.
//!
//! Wires Telegram messages into the duga agent loop, attaching event bridges,
//! JSONL replay sinks, and Telegram-specific context.

use crate::log::BotLogger;
use crate::render::TelegramEventRenderer;
use crate::safety::TelegramConfirmationProvider;
use crate::send_file::SendFileTool;
use crate::session::SessionManager;
use anyhow::{Context, Result};
use duga_config::{Config, TelegramConfig};
use duga_core::loop_context::LoopContext;
use duga_core::loops::{register_default_loops, SimpleReActLoop};
use duga_core::{Loop, LoopRegistry};
use duga_events::{Event, JsonlSink, RedactingSink, StoredEvent};
use duga_tools::ErasedTool;
use duga_tools_builtin::skill_install::InstallSkillTool;
use duga_tools_builtin::skill_list::ListSkillsTool;
use duga_tools_builtin::skill_remove::RemoveSkillTool;
use duga_runtime::events::FrontendEventBridge;
use duga_runtime::memory_context::{format_memory_for_prompt, load_persistent_memory};
use duga_runtime::skills::{discover_skills, format_skills_index_for_prompt};
use duga_runtime::{
    build_agent, build_dispatcher, build_llm, resolve_provider, sandbox_environment_context,
    tool_guidance, ConfirmationMiddleware, ConfirmationPolicy,
};
use duga_sandbox::{CancellationToken, Workspace};
use duga_types::message::Message;
use std::path::PathBuf;
use std::sync::Arc;
use teloxide::prelude::*;

#[derive(Clone)]
pub struct TelegramRuntime {
    config: Config,
}

/// Request to run a task in a chat.
pub struct RunRequest {
    pub chat_id: i64,
    pub task: String,
}

impl TelegramRuntime {
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    /// Run the agent loop for a chat message and return the final text.
    pub async fn run_task_for_chat(
        &self,
        chat_id: i64,
        task: String,
        bot: Bot,
        telegram_config: &TelegramConfig,
        _bot_logger: Arc<BotLogger>,
        session_manager: Arc<SessionManager>,
        cancellation: CancellationToken,
        steering_rx: Option<duga_core::steering::SteeringReceiver>,
    ) -> Result<String> {
        let selection = resolve_provider(&self.config)?;
        let llm = build_llm(&selection.provider, &selection.model, &self.config)?;

        let workspace =
            Arc::new(Workspace::open(&self.config.workspace.root).context("opening workspace")?);

        let aux_roots = vec![telegram_config.data_dir.clone()];
        let dispatcher = build_dispatcher(&self.config, workspace.clone(), aux_roots.clone())?;

        // Register Telegram-specific tool: send_file
        let send_file_tool = SendFileTool::new(
            bot.clone(),
            teloxide::types::ChatId(chat_id),
            telegram_config.send_file.clone(),
            workspace.clone(),
            aux_roots,
        );
        dispatcher
            .register_erased(ErasedTool::erase(send_file_tool))
            .context("registering send_file tool")?;

        // Register skill management tools (EPIC-30).
        // Skills live under workspace: workspace/skills/ (global), workspace/<chat_id>/skills/ (channel).
        let global_skills_dir = self.config.workspace.root.join("skills");
        let channel_skills_base = self.config.workspace.root.clone();
        dispatcher
            .register_erased(ErasedTool::erase(InstallSkillTool::new(
                global_skills_dir.clone(),
                channel_skills_base.clone(),
            )))
            .context("registering install-skill tool")?;
        dispatcher
            .register_erased(ErasedTool::erase(ListSkillsTool::new(
                global_skills_dir.clone(),
                channel_skills_base.clone(),
            )))
            .context("registering list-skills tool")?;
        dispatcher
            .register_erased(ErasedTool::erase(RemoveSkillTool::new(
                global_skills_dir.clone(),
                channel_skills_base,
            )))
            .context("registering remove-skill tool")?;

        // When allow_all_binaries is enabled, shell confirmations are redundant —
        // the operator has already accepted the risk of arbitrary command execution.
        let require_confirmation: Vec<String> = if self.config.sandbox.allow_all_binaries {
            telegram_config
                .require_confirmation_for
                .iter()
                .filter(|t| !matches!(t.as_str(), "shell" | "bash"))
                .cloned()
                .collect()
        } else {
            telegram_config.require_confirmation_for.clone()
        };
        if !require_confirmation.is_empty() {
            let policy = ConfirmationPolicy::new(
                require_confirmation,
                self.config.frontend.confirmation_timeout,
            );
            let provider = Arc::new(TelegramConfirmationProvider::new(
                bot.clone(),
                ChatId(chat_id),
                session_manager,
            ));
            dispatcher.set_confirmation(ConfirmationMiddleware::new(policy, provider));
        }

        // Set up event bridges.
        let (frontend_tx, frontend_bridge) = FrontendEventBridge::new(256);
        let frontend_sink = Arc::new(duga_runtime::events::FrontendEventSink::with_name(
            frontend_tx,
            "telegram",
        ));

        // JSONL replay sink for the chat.
        let chat_dir = telegram_config.data_dir.join(chat_id.to_string());
        tokio::fs::create_dir_all(&chat_dir).await?;
        let jsonl_path = chat_dir.join("session.jsonl");
        let jsonl_sink = Arc::new(JsonlSink::new(&jsonl_path)?);
        let replay_sink = Arc::new(RedactingSink::new(jsonl_sink));

        // Load previous conversation context so the agent remembers the chat.
        let conversation_history =
            load_conversation_history(&jsonl_path, &self.config.memory);

        // Load skills index (metadata-only) from workspace/skills/ and chat-level skills/.
        // Full bodies are lazy-loaded on demand when the LLM calls `read` on a SKILL.md.
        // Channel skills live under workspace/<chat_id>/skills/ (same tree as install-skill).
        let chat_skills = self.config.workspace.root.join(chat_id.to_string());
        let skill_indexes = discover_skills(
            &self.config.workspace.root,
            Some(&chat_skills),
        )
        .unwrap_or_default();
        let skills_prompt = format_skills_index_for_prompt(&skill_indexes);

        // Load persistent memory (MEMORY.md) from workspace and chat-level directories.
        let persistent_memory = load_persistent_memory(
            &self.config.workspace.root,
            Some(&chat_dir),
        )
        .unwrap_or(duga_runtime::memory_context::PersistentMemory {
            workspace_memory: None,
            channel_memory: None,
            workspace_path: self.config.workspace.root.join("MEMORY.md"),
        });
        let memory_prompt = format_memory_for_prompt(&persistent_memory);

        // Build frontend context for system prompt.
        //
        // The task anchoring prefix ("CURRENT TASK: ...") is injected by
        // AgentLoop::run() via Memory::set_task_anchor() and always appears
        // at position 0 of every LLM request.  We build the remaining
        // system prompt here — it sits right after the anchor.
        let env_ctx = sandbox_environment_context(&self.config);
        let tool_guide = tool_guidance();

        // Build the strategies prompt from the loop registry.
        let mut reg = LoopRegistry::new();
        reg.register(Box::new(SimpleReActLoop))
            .expect("SimpleReActLoop must register successfully");
        register_default_loops(&mut reg);
        let strategies = reg.build_strategies_prompt(
            &self.config.agent.loop_config.enabled_loops,
        );

        // Combine all system prompt sections, skipping empty ones.
        let mut sections: Vec<&str> = Vec::new();
        if !skills_prompt.is_empty() {
            sections.push(&skills_prompt);
        }
        if !memory_prompt.is_empty() {
            sections.push(&memory_prompt);
        }

        let extras = if sections.is_empty() {
            String::new()
        } else {
            format!("{}\n\n", sections.join("\n\n"))
        };

        let system_prompt = format!(
            "You are duga, a safe coding agent operating through Telegram.\n\
             Chat ID: {chat_id}\n\n\
             ## Skills\n\
             Skills provide specialized instructions for recurring tasks.\n\
             - View installed skills: use `list-skills` tool or /skills command\n\
             - Install new skills: use `install-skill` tool\n\
             - Remove skills: use `remove-skill` tool\n\
             - Use a skill: `read skills/<name>/SKILL.md` to load its full instructions\n\
             Global skills are in `skills/`. Chat-specific skills in `<chat_id>/skills/`.\n\n\
             {extras}\
             {env_ctx}\n\n\
             {tool_guide}\n\n\
             {strategies}\n\n\
             When facing a complex or multi-step problem, use the `think` tool first to \
             plan your approach before acting. This saves steps and produces better results.\n\
             Prefer `think` over running many small `shell` commands to explore the environment.\n\
             Be concise — Telegram messages have length limits.\n\
             You have a `send_file` tool available to send files (documents, images, audio, video) \
             to the user as proper Telegram attachments. When the user asks for a file you created, \
             use `send_file` with the file path instead of dumping file content into a text message."
        );

        let mut runtime = build_agent(
            &self.config,
            llm,
            dispatcher,
            workspace,
            vec![frontend_sink, replay_sink],
            Some(system_prompt),
        )?;

        // Extract the inner receiver from the bridge for the renderer.
        let mut renderer_rx = frontend_bridge.into_inner();

        // Spawn the renderer task.
        let mut renderer = TelegramEventRenderer::new(bot.clone(), ChatId(chat_id));
        let renderer_handle = tokio::spawn(async move { renderer.run(&mut renderer_rx).await });

        // Restore conversation history into memory.
        if let Some(history) = conversation_history {
            runtime.memory_mut().restore_history(history);
        }

        // Build LoopContext and run via SimpleReActLoop.
        // Scope ctx so borrows are released before we drop `runtime`.
        let result = {
            let loop_impl = SimpleReActLoop;
            let agent_config = self.config.agent.clone();
            let loop_config = &agent_config.loop_config;
            let mut ctx = LoopContext {
                config: &agent_config,
                memory: &mut runtime.memory,
                llm: &runtime.llm,
                tools: &runtime.dispatcher,
                workspace: &runtime.workspace,
                event_sink: &runtime.event_sink,
                summarizer: &runtime.summarizer,
                cancellation: &cancellation,
                registry: &runtime.registry,
                max_refinement_iterations: loop_config.max_refinement_iterations,
                max_delegation_depth: loop_config.max_delegation_depth,
                delegation_depth: 0,
                steer: steering_rx,
                steer_limits: None,            };

            loop_impl.run(task.clone(), &mut ctx).await
        }; // ctx dropped here → borrows released

        // Drop runtime (which holds the frontend event sink senders)
        // BEFORE awaiting the renderer.  Otherwise the renderer blocks on
        // rx.recv() waiting for the channel to close, but the sender is
        // still alive inside `runtime` — deadlock.
        drop(runtime);

        let _ = renderer_handle.await;

        match result {
            Ok(run_result) => {
                let mut text = run_result.message.text.unwrap_or_default();
                if run_result.loop_id != "simple_react" {
                    text.push_str(&format!(
                        "\n\n⟳ via *{}* loop",
                        run_result.loop_id
                    ));
                }
                tracing::info!(
                    loop_id = %run_result.loop_id,
                    steps = run_result.steps,
                    tool_calls = run_result.tool_calls,
                    "chat {chat_id} run completed successfully"
                );
                Ok(text)
            }
            Err(e) => {
                tracing::error!("chat {chat_id} run failed: {e}");
                Err(anyhow::anyhow!("Agent run failed: {e}"))
            }
        }
    }
}

/// Load previous conversation messages from a JSONL session file.
///
/// Applies the sliding window and token budget from `MemoryConfig` to
/// prevent old/irrelevant messages from saturating the context window.
fn load_conversation_history(
    path: &PathBuf,
    memory_config: &duga_config::MemoryConfig,
) -> Option<Vec<Message>> {
    use std::io::{BufRead, BufReader};

    let file = std::fs::File::open(path).ok()?;
    let reader = BufReader::new(file);

    let mut last_messages: Option<Vec<Message>> = None;
    for line in reader.lines() {
        let line = line.ok()?;
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Ok(event) = serde_json::from_str::<StoredEvent>(trimmed) {
            if let Event::LlmRequest { messages, .. } = event.event {
                last_messages = Some(messages);
            }
        }
    }

    let messages = last_messages?;
    if messages.is_empty() {
        return None;
    }

    // Filter out system messages (we provide a fresh system prompt).
    let mut history: Vec<Message> = messages
        .into_iter()
        .filter(|m| !matches!(m.role, duga_types::message::Role::System))
        .collect();

    let total_in_history = history.len();

    // Apply sliding window: keep only the most recent N messages.
    if memory_config.context_window_size > 0
        && history.len() > memory_config.context_window_size
    {
        let start = history.len() - memory_config.context_window_size;
        history = history.split_off(start);
    }

    // Apply token budget: drop oldest messages until under the cap.
    // Uses a simple character-based estimate (no LLM round-trip).
    if memory_config.max_context_tokens > 0 {
        while duga_core::memory::estimate_tokens(&history)
            > memory_config.max_context_tokens
            && history.len() > 1
        {
            history.remove(0);
        }
    }

    // Normalize: drop orphaned tool messages that lack a preceding
    // assistant with tool_calls (can happen after buggy compression in
    // previous runs).
    let history = normalize_tool_message_sequence(history);

    if history.is_empty() {
        None
    } else {
        tracing::info!(
            loaded = history.len(),
            total = total_in_history,
            window = memory_config.context_window_size,
            token_budget = memory_config.max_context_tokens,
            "Loaded {} messages from session history ({} total)",
            history.len(),
            total_in_history
        );
        Some(history)
    }
}

/// Remove orphaned tool messages from a message sequence.
///
/// OpenAI / DeepSeek require every tool-role message to follow an
/// assistant message that contains tool_calls.  A buggy compression
/// split in an earlier run can leave orphaned tool messages at the
/// start of the restored history; this function drops them.
fn normalize_tool_message_sequence(messages: Vec<Message>) -> Vec<Message> {
    use duga_types::message::{ContentBlock, Role};

    let mut out = Vec::with_capacity(messages.len());
    // Track the last assistant that had tool_calls so consecutive
    // tool results all remain valid.
    let mut last_assistant_had_tool_calls = false;
    for msg in messages {
        if msg.role == Role::Tool {
            if last_assistant_had_tool_calls {
                out.push(msg);
            }
            // else: orphaned — drop it silently.
            // Do NOT reset last_assistant_had_tool_calls here;
            // multiple tool results can follow one assistant.
        } else {
            let has_tool_calls = msg.role == Role::Assistant
                && msg
                    .content
                    .iter()
                    .any(|block| matches!(block, ContentBlock::ToolCall(_)));
            last_assistant_had_tool_calls = has_tool_calls;
            out.push(msg);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use duga_types::message::{ContentBlock, Message, Role};
    use duga_types::tool_call::ToolCall;

    fn assistant_with_tool_calls(tool_names: &[&str]) -> Message {
        let calls: Vec<_> = tool_names
            .iter()
            .map(|name| ContentBlock::ToolCall(ToolCall::new(*name, serde_json::json!({}))))
            .collect();
        Message {
            role: Role::Assistant,
            content: calls,
            name: None,
            pinned: false,
            reasoning_content: None,
        }
    }

    fn assistant_text(text: &str) -> Message {
        Message::assistant(Some(text.into()), vec![], None)
    }

    fn tool_msg(output: &str) -> Message {
        use duga_types::tool_call::CallId;
        Message::tool(CallId::new().as_uuid(), output.to_string())
    }

    fn user_msg(text: &str) -> Message {
        Message::user(text)
    }

    #[test]
    fn normalize_preserves_valid_tool_sequence() {
        let messages = vec![
            user_msg("run command"),
            assistant_with_tool_calls(&["bash"]),
            tool_msg("output"),
            assistant_text("done"),
        ];
        let result = normalize_tool_message_sequence(messages);
        assert_eq!(result.len(), 4);
        assert_eq!(result[0].role, Role::User);
        assert_eq!(result[1].role, Role::Assistant);
        assert_eq!(result[2].role, Role::Tool);
        assert_eq!(result[3].role, Role::Assistant);
    }

    #[test]
    fn normalize_drops_orphaned_tool_at_start() {
        // Simulates a compression split that left a tool message
        // at the start of recent_messages with no preceding assistant.
        let messages = vec![
            tool_msg("orphaned output"),
            user_msg("continue"),
            assistant_text("ok"),
        ];
        let result = normalize_tool_message_sequence(messages);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].role, Role::User);
        assert_eq!(result[1].role, Role::Assistant);
    }

    #[test]
    fn normalize_drops_orphaned_tool_after_text_assistant() {
        // A tool message after an assistant without tool_calls is orphaned.
        let messages = vec![
            user_msg("hello"),
            assistant_text("how can I help?"),
            tool_msg("orphaned"),
            user_msg("next"),
        ];
        let result = normalize_tool_message_sequence(messages);
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].role, Role::User);
        assert_eq!(result[1].role, Role::Assistant);
        assert_eq!(result[2].role, Role::User);
    }

    #[test]
    fn normalize_preserves_multiple_tool_results() {
        let messages = vec![
            user_msg("do two things"),
            assistant_with_tool_calls(&["bash", "read"]),
            tool_msg("bash output"),
            tool_msg("read output"),
            assistant_text("both done"),
        ];
        let result = normalize_tool_message_sequence(messages);
        assert_eq!(result.len(), 5);
        assert_eq!(result[2].role, Role::Tool);
        assert_eq!(result[3].role, Role::Tool);
    }

    // ── Skills and persistent memory injection tests ──────────────

    /// Verify that skills and persistent memory are loaded and included in
    /// the system prompt when the corresponding files exist.
    ///
    /// Regression test for: bot didn't use skills until explicitly mentioned
    /// in a fresh session, because skills and MEMORY.md were never loaded
    /// into the system prompt in `run_task_for_chat`.
    #[test]
    fn system_prompt_includes_skills_and_memory() {
        use duga_runtime::skills::{format_skills_for_prompt, load_skills};
        use duga_runtime::memory_context::{format_memory_for_prompt, load_persistent_memory};

        let ws = tempfile::tempdir().unwrap();

        // Create a skill at workspace/skills/code-review/SKILL.md
        let skill_dir = ws.path().join("skills").join("code-review");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: code-review\ndescription: Review code changes\n---\n\n# Code Review\n\nCheck for correctness, style, and security.",
        )
        .unwrap();

        // Create a second skill at workspace/skills/debug/SKILL.md
        let skill_dir2 = ws.path().join("skills").join("debug");
        std::fs::create_dir_all(&skill_dir2).unwrap();
        std::fs::write(
            skill_dir2.join("SKILL.md"),
            "---\nname: debug\ndescription: Debug application issues\n---\n\n# Debugging\n\nUse logging and binary search to isolate bugs.",
        )
        .unwrap();

        // Create MEMORY.md at workspace root
        std::fs::write(
            ws.path().join("MEMORY.md"),
            "# Project\n\nThis project is a Telegram coding bot.",
        )
        .unwrap();

        // Load skills
        let skills = load_skills(ws.path(), None).unwrap();
        assert_eq!(skills.len(), 2);
        let skill_names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
        assert!(skill_names.contains(&"code-review"));
        assert!(skill_names.contains(&"debug"));

        let skills_prompt = format_skills_for_prompt(&skills);
        assert!(skills_prompt.contains("## Available Skills"));
        assert!(skills_prompt.contains("code-review"));
        assert!(skills_prompt.contains("Review code changes"));
        assert!(skills_prompt.contains("debug"));
        assert!(skills_prompt.contains("Debug application issues"));
        assert!(skills_prompt.contains("Check for correctness"));
        assert!(skills_prompt.contains("Use logging and binary search"));

        // Load persistent memory
        let memory = load_persistent_memory(ws.path(), None).unwrap();
        assert!(memory.workspace_memory.is_some());
        assert!(memory.channel_memory.is_none());

        let memory_prompt = format_memory_for_prompt(&memory);
        assert!(memory_prompt.contains("## Workspace Memory"));
        assert!(memory_prompt.contains("Telegram coding bot"));

        // Simulate system prompt assembly (same logic as run_task_for_chat)
        let mut sections: Vec<&str> = Vec::new();
        if !skills_prompt.is_empty() {
            sections.push(&skills_prompt);
        }
        if !memory_prompt.is_empty() {
            sections.push(&memory_prompt);
        }
        let extras = if sections.is_empty() {
            String::new()
        } else {
            format!("{}\n\n", sections.join("\n\n"))
        };

        let system_prompt = format!(
            "You are duga, a safe coding agent.\n\
             {extras}\
             Some other context."
        );

        // Verify both skills and memory appear in the assembled prompt
        assert!(system_prompt.contains("## Available Skills"),
            "System prompt should contain skills section");
        assert!(system_prompt.contains("## Workspace Memory"),
            "System prompt should contain memory section");
        assert!(system_prompt.contains("code-review"),
            "System prompt should contain skill name");
        assert!(system_prompt.contains("Telegram coding bot"),
            "System prompt should contain memory content");
        assert!(system_prompt.contains("Some other context."),
            "System prompt should still contain original sections");
    }

    /// Verify that when no skills or MEMORY.md exist, the extras section
    /// is empty and the system prompt is not cluttered.
    #[test]
    fn system_prompt_empty_when_no_skills_or_memory() {
        use duga_runtime::skills::{format_skills_for_prompt, load_skills};
        use duga_runtime::memory_context::{format_memory_for_prompt, load_persistent_memory};

        let ws = tempfile::tempdir().unwrap();

        // No skills dir, no MEMORY.md — empty workspace.
        let skills = load_skills(ws.path(), None).unwrap();
        assert!(skills.is_empty());
        let skills_prompt = format_skills_for_prompt(&skills);
        assert!(skills_prompt.is_empty());

        let memory = load_persistent_memory(ws.path(), None).unwrap();
        assert!(memory.workspace_memory.is_none());
        let memory_prompt = format_memory_for_prompt(&memory);
        assert!(memory_prompt.is_empty());

        // Assemble system prompt (same logic as run_task_for_chat)
        let mut sections: Vec<&str> = Vec::new();
        if !skills_prompt.is_empty() {
            sections.push(&skills_prompt);
        }
        if !memory_prompt.is_empty() {
            sections.push(&memory_prompt);
        }
        let extras = if sections.is_empty() {
            String::new()
        } else {
            format!("{}\n\n", sections.join("\n\n"))
        };
        assert!(extras.is_empty(),
            "extras should be empty when no skills or memory exist");

        let system_prompt = format!(
            "You are duga, a safe coding agent.\n\
             {extras}\
             Core content."
        );
        assert!(!system_prompt.contains("## Available Skills"));
        assert!(!system_prompt.contains("## Workspace Memory"));
        assert!(system_prompt.contains("Core content."));
    }

    /// Verify that channel-level skills override workspace-level skills
    /// when both exist with the same name.
    #[test]
    fn channel_skills_override_workspace_skills() {
        use duga_runtime::skills::load_skills;

        let ws = tempfile::tempdir().unwrap();
        let ch = tempfile::tempdir().unwrap();

        // Workspace skill
        let ws_skill = ws.path().join("skills").join("review");
        std::fs::create_dir_all(&ws_skill).unwrap();
        std::fs::write(
            ws_skill.join("SKILL.md"),
            "---\nname: review\ndescription: Workspace review\n---\n\nWorkspace version.",
        )
        .unwrap();

        // Channel skill with same name
        let ch_skill = ch.path().join("skills").join("review");
        std::fs::create_dir_all(&ch_skill).unwrap();
        std::fs::write(
            ch_skill.join("SKILL.md"),
            "---\nname: review\ndescription: Channel review\n---\n\nChannel version.",
        )
        .unwrap();

        let skills = load_skills(ws.path(), Some(ch.path())).unwrap();
        assert_eq!(skills.len(), 1, "channel should override workspace");
        assert!(skills[0].body.contains("Channel version"),
            "channel skill body should be used, got: {}", skills[0].body);
        assert_eq!(skills[0].source, duga_runtime::skills::SkillSource::Channel);
    }

    /// Verify that channel-level MEMORY.md appears alongside workspace MEMORY.md.
    #[test]
    fn channel_memory_is_included() {
        use duga_runtime::memory_context::{format_memory_for_prompt, load_persistent_memory};

        let ws = tempfile::tempdir().unwrap();
        let ch = tempfile::tempdir().unwrap();

        std::fs::write(ws.path().join("MEMORY.md"), "Workspace memory.").unwrap();
        std::fs::write(ch.path().join("MEMORY.md"), "Channel memory.").unwrap();

        let memory = load_persistent_memory(ws.path(), Some(ch.path())).unwrap();
        assert!(memory.workspace_memory.is_some());
        assert!(memory.channel_memory.is_some());

        let prompt = format_memory_for_prompt(&memory);
        assert!(prompt.contains("## Session Memory"),
            "channel memory should be labelled 'Session Memory'");
        assert!(prompt.contains("## Workspace Memory"),
            "workspace memory should be labelled 'Workspace Memory'");
        assert!(prompt.contains("Channel memory."));
        assert!(prompt.contains("Workspace memory."));
        // Channel memory must appear before workspace memory in the prompt.
        let ch_pos = prompt.find("Channel").unwrap();
        let ws_pos = prompt.find("Workspace").unwrap();
        assert!(ch_pos < ws_pos,
            "channel memory should come before workspace memory");
    }

    // ── Renderer deadlock regression test ──────────────────────────

    /// Verify that the frontend event bridge completes cleanly: when the
    /// sender is dropped, the receiver stream terminates promptly.
    /// This guards against the deadlock fixed in
    /// `run_task_for_chat` where `renderer_handle.await` blocked
    /// indefinitely because the sending half was still alive inside
    /// `runtime`.
    #[tokio::test]
    async fn frontend_bridge_closes_when_sender_dropped() {
        use duga_runtime::events::{FrontendEvent, FrontendEventBridge, FrontendEventSink};
        use duga_events::{Event, EventSink};
        use std::sync::Arc;
        use std::time::Duration;

        let (tx, mut bridge) = FrontendEventBridge::new(16);
        let sink = Arc::new(FrontendEventSink::new(tx));

        // Spawn a "renderer" that collects events.
        let renderer_handle = tokio::spawn(async move {
            let mut events = Vec::new();
            while let Some(event) = bridge.recv().await {
                events.push(event);
            }
            events
        });

        // Emit a few events and then drop the sender.
        sink.emit(Event::AgentStarted {
            task: "test".into(),
        })
        .await
        .unwrap();
        sink.emit(Event::AgentFinished {
            text: Some("done".into()),
        })
        .await
        .unwrap();
        drop(sink);

        // The renderer must complete within 5 seconds.
        let events = tokio::time::timeout(Duration::from_secs(5), renderer_handle)
            .await
            .expect("renderer deadlocked — sender was dropped but receiver did not close")
            .expect("renderer task panicked");

        assert_eq!(events.len(), 2);
        assert!(matches!(events[0], FrontendEvent::RunStarted { .. }));
        assert!(matches!(events[1], FrontendEvent::RunFinished { .. }));
    }
}
