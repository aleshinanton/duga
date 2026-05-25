//! Runtime configuration types.
//!
//! All limit structs derive `Default` with safe, conservative values.
//! Duration serialization supports human-readable formats: "10m", "120s", "1h", "500ms".

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Configuration for the loop-agnostic delegation system.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct LoopConfig {
    /// Loop ids that are available for delegation (e.g. `["problem_solving"]`).
    /// Empty means only `simple_react` is usable — delegation always fails.
    #[serde(default)]
    pub enabled_loops: Vec<String>,
    /// Maximum refinement iterations used by advanced loops.
    #[serde(default = "default_max_refinement_iterations")]
    pub max_refinement_iterations: u32,
    /// Hard cap on delegation chain depth (0 = no delegation allowed).
    #[serde(default = "default_max_delegation_depth")]
    pub max_delegation_depth: u32,
}

fn default_max_refinement_iterations() -> u32 {
    3
}

fn default_max_delegation_depth() -> u32 {
    2
}

impl Default for LoopConfig {
    fn default() -> Self {
        Self {
            enabled_loops: Vec::new(),
            max_refinement_iterations: default_max_refinement_iterations(),
            max_delegation_depth: default_max_delegation_depth(),
        }
    }
}

/// A steering rule loaded from config — injects guidance every iteration (or once).
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SteeringRule {
    /// The guidance text to inject.
    pub guidance: String,
    /// If true, push as a system message (higher LLM attention).
    #[serde(default)]
    pub as_system: bool,
    /// If true, inject every iteration. If false, inject only on the first step.
    #[serde(default)]
    pub repeat: bool,
}

/// Config-driven steering rules.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct SteeringConfig {
    #[serde(default)]
    pub rules: Vec<SteeringRule>,
}

/// Top-level agent configuration composing all sub-configs.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct AgentConfig {
    pub limits: AgentLimits,
    pub features: AgentFeatures,
    pub output: OutputLimits,
    pub think: ThinkLimits,
    #[serde(default)]
    pub loop_config: LoopConfig,
    #[serde(default)]
    pub steering: SteeringConfig,
}

/// Controls loop and sandbox resource limits.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct AgentLimits {
    pub max_steps: u32,
    pub max_tool_calls: u32,
    #[serde(with = "duration_format")]
    pub max_runtime: Duration,
    pub retry_on_error: u32,
}

/// Feature flags.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct AgentFeatures {
    pub streaming: bool,
}

/// Per-stream output byte limits.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct OutputLimits {
    pub max_stdout_bytes: u64,
    pub max_stderr_bytes: u64,
    pub max_combined_bytes: u64,
}

/// Per-tool thinking limits.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ThinkLimits {
    pub max_calls: u32,
    pub max_tokens: u32,
}

// ── Defaults ────────────────────────────────────────────────────────────────

impl Default for AgentLimits {
    fn default() -> Self {
        Self {
            max_steps: 50,
            max_tool_calls: 100,
            max_runtime: Duration::from_secs(10 * 60), // 10 minutes
            retry_on_error: 2,
        }
    }
}

impl Default for AgentFeatures {
    fn default() -> Self {
        Self { streaming: false }
    }
}

impl Default for OutputLimits {
    fn default() -> Self {
        Self {
            max_stdout_bytes: 4 * 1024 * 1024,   // 4 MB
            max_stderr_bytes: 4 * 1024 * 1024,   // 4 MB
            max_combined_bytes: 8 * 1024 * 1024, // 8 MB
        }
    }
}

impl Default for ThinkLimits {
    fn default() -> Self {
        Self {
            max_calls: 8,
            max_tokens: 4096,
        }
    }
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            limits: AgentLimits::default(),
            features: AgentFeatures::default(),
            output: OutputLimits::default(),
            think: ThinkLimits::default(),
            loop_config: LoopConfig::default(),
            steering: SteeringConfig::default(),
        }
    }
}

// ── Duration serde module ───────────────────────────────────────────────────

mod duration_format {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::Duration;

    pub fn serialize<S>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let secs = duration.as_secs();
        if secs >= 3600 && secs % 3600 == 0 {
            serializer.serialize_str(&format!("{}h", secs / 3600))
        } else if secs >= 60 && secs % 60 == 0 {
            serializer.serialize_str(&format!("{}m", secs / 60))
        } else {
            serializer.serialize_str(&format!("{}s", secs))
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        let s = s.trim();

        let parse_num = |s: &str| {
            s.parse::<u64>().map_err(|_| {
                serde::de::Error::custom(format!(
                    "duration string '{}' has invalid numeric part '{}'",
                    s, s
                ))
            })
        };

        let duration = if s.ends_with('h') {
            let n = parse_num(&s[..s.len() - 1])?;
            Duration::from_secs(n * 3600)
        } else if s.ends_with('m') {
            let n = parse_num(&s[..s.len() - 1])?;
            Duration::from_secs(n * 60)
        } else if s.ends_with("ms") {
            let n = parse_num(&s[..s.len() - 2])?;
            Duration::from_millis(n)
        } else if s.ends_with('s') {
            let n = parse_num(&s[..s.len() - 1])?;
            Duration::from_secs(n)
        } else {
            return Err(serde::de::Error::custom(format!(
                "cannot parse duration '{}' (expected e.g. 10m, 120s, 1h, 500ms)",
                s
            )));
        };

        if duration.is_zero() {
            return Err(serde::de::Error::custom(
                "duration must be positive (got 0)",
            ));
        }

        Ok(duration)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_agent_limits() {
        let lim = AgentLimits::default();
        assert_eq!(lim.max_steps, 50);
        assert_eq!(lim.max_tool_calls, 100);
        assert_eq!(lim.max_runtime, Duration::from_secs(600));
        assert_eq!(lim.retry_on_error, 2);
    }

    #[test]
    fn test_default_output_limits() {
        let out = OutputLimits::default();
        assert_eq!(out.max_stdout_bytes, 4 * 1024 * 1024);
        assert_eq!(out.max_stderr_bytes, 4 * 1024 * 1024);
        assert_eq!(out.max_combined_bytes, 8 * 1024 * 1024);
    }

    #[test]
    fn test_default_think_limits() {
        let think = ThinkLimits::default();
        assert_eq!(think.max_calls, 8);
        assert_eq!(think.max_tokens, 4096);
    }

    #[test]
    fn test_default_config() {
        let cfg = AgentConfig::default();
        assert_eq!(cfg.limits.max_steps, 50);
        assert!(!cfg.features.streaming);
    }

    #[test]
    fn test_duration_deserialize_minutes() {
        let json =
            r#"{"max_steps": 10, "max_tool_calls": 20, "max_runtime": "5m", "retry_on_error": 1}"#;
        let lim: AgentLimits = serde_json::from_str(json).unwrap();
        assert_eq!(lim.max_runtime, Duration::from_secs(300));
    }

    #[test]
    fn test_duration_deserialize_seconds() {
        let json = r#"{"max_runtime": "120s"}"#;
        #[derive(Deserialize)]
        struct H {
            #[serde(with = "duration_format")]
            max_runtime: Duration,
        }
        let h: H = serde_json::from_str(json).unwrap();
        assert_eq!(h.max_runtime, Duration::from_secs(120));
    }

    #[test]
    fn test_duration_deserialize_hours() {
        let json = r#"{"max_runtime": "2h"}"#;
        #[derive(Deserialize)]
        struct H {
            #[serde(with = "duration_format")]
            max_runtime: Duration,
        }
        let h: H = serde_json::from_str(json).unwrap();
        assert_eq!(h.max_runtime, Duration::from_secs(7200));
    }

    #[test]
    fn test_duration_deserialize_ms() {
        let json = r#"{"max_runtime": "500ms"}"#;
        #[derive(Deserialize)]
        struct H {
            #[serde(with = "duration_format")]
            max_runtime: Duration,
        }
        let h: H = serde_json::from_str(json).unwrap();
        assert_eq!(h.max_runtime, Duration::from_millis(500));
    }

    #[test]
    fn test_duration_serialize_minutes() {
        let d = Duration::from_secs(300);
        let json = serde_json::to_string(&d).unwrap();
        assert!(json.contains("300"));
    }

    #[test]
    fn test_duration_zero_rejected() {
        let json = r#"{"max_runtime": "0s"}"#;
        #[derive(Deserialize)]
        #[allow(dead_code)]
        struct H {
            #[serde(with = "duration_format")]
            max_runtime: Duration,
        }
        let result: Result<H, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_full_agent_config_roundtrip() {
        let cfg = AgentConfig::default();
        let json = serde_json::to_string(&cfg).unwrap();
        let decoded: AgentConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, cfg);
    }
}
