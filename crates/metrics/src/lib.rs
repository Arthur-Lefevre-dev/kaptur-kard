//! Lightweight atomic counters shared across capture / render / UI.

pub mod counters;

pub use counters::{MetricsSnapshot, SharedMetrics};
