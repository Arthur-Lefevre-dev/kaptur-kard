//! Lightweight atomic counters shared across capture / render / UI.
//!
//! Display pipeline delay is capture timestamp → present. Software cannot
//! measure true end-to-end input lag (controller / HDMI / panel).

pub mod counters;
pub mod percentile;
pub mod process;

pub use counters::{MetricsSnapshot, SharedMetrics, FRAME_TIME_RING};
pub use percentile::{fps_from_frame_ns, one_percent_low_fps, percentile_u64};
pub use process::{sample as sample_process, ProcessSample};
