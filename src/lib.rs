#![doc = include_str!("../README.md")]

#[cfg(feature = "cli")]
pub mod app;
#[cfg(feature = "cli")]
pub mod cli;
#[cfg(feature = "cli")]
pub mod mcp;
pub mod model;
pub mod oauth;
pub mod provider;
pub mod render;
pub mod store;
