//! Event sink trait — for emitting tool events.

pub trait EventSink: Send + Sync {
    fn name(&self) -> &str {
        "anonymous"
    }
}

/// A no-op event sink for testing.
pub struct NullSink;

impl EventSink for NullSink {
    fn name(&self) -> &str { "null" }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_sink_is_send_sync() {
        fn assert_send_sync<T: Send + Sync + ?Sized>() {}
        assert_send_sync::<NullSink>();
        assert_send_sync::<dyn EventSink>();
    }
}