//! Enhancer — local image & video enhancement.
//!
//! The library holds everything except the eframe entry point so the integration
//! tests can drive the real pipeline.

pub mod app;
pub mod bench;
pub mod history;
pub mod i18n;
pub mod net;
pub mod outputs;
pub mod config;
pub mod editor;
pub mod devices;
pub mod engines;
pub mod logging;
pub mod media;
pub mod models;
pub mod monitor;
pub mod paths;
pub mod pipeline;
pub mod preview;
pub mod profiles;
pub mod resources;
pub mod sharp;
pub mod worker;
pub mod ui;
pub mod util;
