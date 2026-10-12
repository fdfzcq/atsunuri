//! Canvas and brush engine for Atsunuri.
//!
//! This crate has no windowing or GPU dependencies so it can be unit tested
//! and driven headlessly (e.g. by replaying recorded strokes).

mod brush;
mod canvas;
mod rect;

pub use brush::{Brush, InputPoint, Stroke};
pub use canvas::Canvas;
pub use rect::Rect;
