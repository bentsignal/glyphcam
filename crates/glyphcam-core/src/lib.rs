//! glyphcam core pipeline.
//!
//! Frame flow: a source produces [`RgbImage`] frames, the [`track::Tracker`]
//! finds the face (and optionally a person mask) on a worker thread, the
//! [`stabilize::Stabilizer`] turns face observations into a smoothed,
//! view-model-style camera transform, and the [`render::Renderer`] samples the
//! source through that transform and blits glyphs from a pre-rasterized atlas
//! into a BGRA [`Frame`].

pub mod color;
pub mod glyphs;
pub mod image;
pub mod pipeline;
pub mod render;
pub mod settings;
pub mod source;
pub mod stabilize;
pub mod track;

pub use crate::image::{Frame, RgbImage};
pub use pipeline::Pipeline;
pub use settings::Settings;
