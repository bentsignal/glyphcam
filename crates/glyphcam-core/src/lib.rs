//! glyphcam core pipeline.
//!
//! Frame flow: a source produces [`RgbImage`] frames, the [`track::Tracker`]
//! finds the face and a person mask on a worker thread, and the
//! [`render::Renderer`] samples the source through a fixed
//! [`framing::FrameXf`] (fill + optional mirror) and blits glyphs from a
//! pre-rasterized atlas into a BGRA [`Frame`].

pub mod color;
pub mod depth;
pub mod framing;
pub mod glyphs;
pub mod image;
pub mod pipeline;
pub mod render;
pub mod settings;
pub mod source;
pub mod track;

pub use crate::image::{Frame, RgbImage};
pub use pipeline::Pipeline;
pub use settings::Settings;
