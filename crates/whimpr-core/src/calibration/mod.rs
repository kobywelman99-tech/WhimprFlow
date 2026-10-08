//! Voice calibration: a guided session where the user reads known text and the
//! brain (`crate::brain`) learns from the gap between what was meant and what the
//! recognizer heard.

pub mod align;
pub mod corpus;

pub use align::{diff, segments, Mishear, Segment};
pub use corpus::{prompts, Prompt};
