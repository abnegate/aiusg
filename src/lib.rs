#![doc = include_str!("../README.md")]
#![warn(missing_docs)]

#[cfg(feature = "cli")]
#[doc(hidden)]
pub mod app;
#[cfg(feature = "cli")]
#[doc(hidden)]
pub mod cli;
#[cfg(feature = "cli")]
#[doc(hidden)]
pub mod mcp;
pub mod model;
pub(crate) mod oauth;
pub mod provider;
#[cfg(feature = "cli")]
pub(crate) mod render;
pub mod store;
