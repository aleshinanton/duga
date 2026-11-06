# Security boundaries and migration

## Execution modes

- `capability` (default) enables capability-bounded filesystem tools, **not subprocesses**. `shell` process execution fails closed. `cap_std` does not sandbox native child processes.
- `host` is an explicit opt-in to trusted host execution. Commands have the OS user's filesystem and network permissions. A binary allowlist, sanitized environment and working directory are not OS isolation. Do not use this mode for untrusted tasks.
- `docker` executes in a dedicated, pre-existing container. The operator must avoid privileged containers, host namespaces, Docker socket mounts and mounts exposing unrelated host data. Mount only the intended workspace. Install `sh` and `setsid` in the image. Do not share the container with other applications.

Host processes have a bounded lifetime, including output draining. Unix execution uses a process group; Windows starts the process suspended, assigns a kill-on-close Job Object, then resumes it. Cancellation, timeout and dropping the execution future terminate the owned process tree and abort pipe readers. Trusted host commands must not deliberately escape their process group on Unix.

Docker commands use a remote process group for normal completion cleanup. On cancellation, timeout or future drop, the dedicated container is stopped with a zero-second grace period. This also stops daemonized processes that escaped the command's process group. A normally awaited cleanup is bounded to five seconds; drop cleanup uses a bounded OS worker and can finish shortly after the caller returns. Docker daemon failure can prevent remote cleanup and is reported when awaited. **Restart the container explicitly before running another task after cancellation/timeout.** Container filesystem changes persist.

## Telegram isolation

Every chat gets its own workspace:

```
<workspace.root>/chats/<chat_id>/
```

History and downloaded attachments remain under:

```
<telegram.data_dir>/<chat_id>/
```

Only that chat's data directory is supplied to auxiliary filesystem tools. Skill and memory discovery uses the chat-specific workspace, not the shared parent. A skill's `global` scope is global only within this private workspace. Changing a `channel_id` tool argument cannot reach another chat's workspace.

Telegram rejects `host` mode. Docker container configuration must contain a `{chat_id}` placeholder:

```yaml
sandbox:
  mode: docker
  container: "duga-{chat_id}"
  workspace_mount: /workspace
  allow_all_binaries: true
  timeout: 120s
telegram:
  require_confirmation_for: [shell, write, edit]
```

Provision a separate container per authorized chat and bind **only** that chat's workspace to `/workspace`. Never bind the parent workspace or all Telegram data. Containers must also use separate writable volumes and must not contain another user's credentials. Container names alone cannot correct unsafe operator-provisioned mounts. Configured MCP servers and plugins are trusted integrations; their external services must enforce tenant separation independently.

Existing files under the old shared workspace are **not automatically copied** into chat workspaces: automatic copying would duplicate private data between users. Migrate only the intended files into each chat's directory. Existing per-chat session histories are retained.

## File reads and Telegram uploads

`read` and `send_file` open files through directory capabilities for both the workspace and auxiliary roots. Parent traversal is rejected before translating absolute paths or `/workspace` aliases. Escaping symlinks are rejected by capability resolution. Uploads use bytes from the already-opened file, never a second ambient path lookup. Auxiliary reads respect offset/limit without loading the whole file first. Upload reads are bounded even if the file grows after metadata inspection.

## Consent and limits

`allow_all_binaries` affects executable permissions only. It never overrides `require_confirmation_for`; per-action approval is an independent policy.

All built-in loop strategies share one runtime budget across planning, tool execution, auditing, voting, subtasks and delegation. Each LLM request consumes a step. Each tool invocation, including delegation, consumes a tool call; retries remain attempts of that invocation. Memory checkpoints do not reset counters or deadlines. The entire strategy is bounded by cancellation and `max_runtime`, including event emission and summarization. Starting a new root run resets the budget.

## Regression tests

- `crates/duga-sandbox/tests/security.rs`: namespace traversal, Unix auxiliary symlink escape/handle replacement, process-tree timeout/cancellation/future drop, inherited pipe lifetime.
- `crates/duga-core/tests/run_budget.rs`: deadlines, cancellation, step/tool caps for every strategy, delegation sharing its parent's budget.
- Telegram runtime unit tests: distinct workspace namespaces, rejection of host/shared-container mode, preservation of confirmation policy.

Live Docker cleanup and Unix process/symlink tests need their corresponding environments; Windows-only execution does not validate those paths.
