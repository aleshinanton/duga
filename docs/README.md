# duga — Documentation

## Overview

**duga** is a hardened, agent-first LLM runtime. It provides tools, memory, sandboxing, and a core loop — and leaves all reasoning decisions to the LLM.

```text
User → Agent Loop → Tools → Agent Loop → Answer
```

## Documents

| File | Description |
|------|-------------|
| [architecture.md](architecture.md) | Full architecture specification — core loop, tools, memory, sandbox, plugins, events, threat model |
| [implementation-plan.md](implementation-plan.md) | Implementation-grade execution plan — crate tree, dependency graph, phases, testing strategy, GAP tracking |

## Key Principles

- **No hardcoded workflows.** The LLM decides behavior. The runtime enforces bounds.
- **Deterministic at the runtime layer.** Tool dispatch, event ordering, and memory bookkeeping are reproducible given the same LLM outputs.
- **Capability-bounded.** Tools receive only explicitly granted capabilities. No raw filesystem access, no shell interpretation, no inherited environment.
- **Observable.** Full replay-format event stream (JSONL) captures every decision.
- **Small.** LLM + tools + memory + sandbox + loop. Nothing more.

## Quick Links

- [Architecture: Core Loop](architecture.md#5-core-loop)
- [Architecture: Built-in Tools](architecture.md#11-built-in-tools)
- [Architecture: WASM Plugin Model](architecture.md#28-wasm-plugin-model)
- [Architecture: Threat Model](architecture.md#34-threat-model)
- [Plan: Crate Tree](implementation-plan.md#1-cargo-workspace-tree)
- [Plan: Implementation Phases](implementation-plan.md#12-implementation-phases)
- [Plan: GAP Tracking](implementation-plan.md#0-gaps-that-must-be-answered)
