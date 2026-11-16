//! DiffCode's interface, platform-neutral: a small immediate-mode toolkit (geometry, painter and text traits,
//! input, widgets) and, on top of it, every view and the app state. A shell supplies the window, the painter and
//! the text backend: Direct2D/DirectWrite on Windows, CoreGraphics/CoreText in the Mac test harness.

// Callbacks are boxed closures throughout; naming each signature would not make them clearer.
#![allow(clippy::type_complexity)]

pub mod app;
pub mod canvas;
pub mod geom;
pub mod icons;
pub mod input;
pub mod paint;
pub mod platform;
pub mod svg;
pub mod terminal;
pub mod text_field;
pub mod theme;
pub mod ui;
pub mod views;
pub mod widgets;

pub use diffcode_core as core;
