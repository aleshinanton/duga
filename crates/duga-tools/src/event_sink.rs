//! Event sink compatibility exports for tool crates.

pub use duga_events::{Event, EventFuture, EventSink, NullSink};

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
