//! Event sink trait — for emitting tool events.
//!
//! Forward-declared from EPIC-7. Implementors must be `Send + Sync`.

/// Marker trait for event sinks.
pub trait EventSink: Send + Sync {
    fn name(&self) -> &str {
        "anonymous"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct DummySink;
    impl EventSink for DummySink {}

    #[test]
    fn test_event_sink_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<DummySink>();
        assert_send_sync::<dyn EventSink>();
    }
}