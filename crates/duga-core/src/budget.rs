//! One run budget shared by every strategy and delegation. Not checkpointed with history.
use duga_types::{config::AgentConfig, error::AgentError};
use std::{
    sync::Mutex,
    time::{Duration, Instant},
};

#[derive(Debug, Default)]
pub(crate) struct RunBudget(Mutex<Option<State>>);
#[derive(Debug)]
struct State {
    started: Instant,
    steps: u32,
    tools: u32,
}
impl RunBudget {
    pub fn reset(&self) {
        *self.0.lock().unwrap() = Some(State {
            started: Instant::now(),
            steps: 0,
            tools: 0,
        });
    }
    fn access<T>(
        &self,
        config: &AgentConfig,
        f: impl FnOnce(&mut State) -> Result<T, AgentError>,
    ) -> Result<T, AgentError> {
        let mut lock = self.0.lock().unwrap();
        let state = lock.get_or_insert_with(|| State {
            started: Instant::now(),
            steps: 0,
            tools: 0,
        });
        if state.started.elapsed() >= config.limits.max_runtime {
            return Err(AgentError::Timeout);
        }
        f(state)
    }
    pub fn remaining(&self, config: &AgentConfig) -> Result<Duration, AgentError> {
        self.access(config, |s| {
            config
                .limits
                .max_runtime
                .checked_sub(s.started.elapsed())
                .filter(|d| !d.is_zero())
                .ok_or(AgentError::Timeout)
        })
    }
    pub fn step(&self, config: &AgentConfig) -> Result<(), AgentError> {
        self.access(config, |s| {
            if s.steps >= config.limits.max_steps {
                return Err(AgentError::MaxStepsReached {
                    limit: config.limits.max_steps,
                });
            }
            s.steps += 1;
            Ok(())
        })
    }
    pub fn tool(&self, config: &AgentConfig) -> Result<(), AgentError> {
        self.access(config, |s| {
            if s.tools >= config.limits.max_tool_calls {
                return Err(AgentError::MaxToolCallsReached {
                    limit: config.limits.max_tool_calls,
                });
            }
            s.tools += 1;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_counters_are_not_refreshed_between_phases() {
        let mut config = AgentConfig::default();
        config.limits.max_steps = 1;
        config.limits.max_tool_calls = 1;
        let budget = RunBudget::default();
        budget.step(&config).unwrap();
        budget.tool(&config).unwrap();
        assert!(matches!(
            budget.step(&config),
            Err(AgentError::MaxStepsReached { .. })
        ));
        assert!(matches!(
            budget.tool(&config),
            Err(AgentError::MaxToolCallsReached { .. })
        ));
        budget.reset();
        budget.step(&config).unwrap();
    }
}
