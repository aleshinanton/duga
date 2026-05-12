//! Monotonically increasing sequence number for event ordering.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Hash, PartialEq, PartialOrd, Serialize)]
pub struct Seq(u64);

impl Seq {
    pub fn next(self) -> Seq {
        Seq(self.0 + 1)
    }

    pub fn value(self) -> u64 {
        self.0
    }
}

impl From<u64> for Seq {
    fn from(n: u64) -> Self {
        Seq(n)
    }
}

impl From<Seq> for u64 {
    fn from(s: Seq) -> Self {
        s.0
    }
}

impl std::fmt::Display for Seq {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "seq-{}", self.0)
    }
}
