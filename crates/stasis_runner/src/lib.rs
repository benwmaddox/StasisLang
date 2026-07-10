#![forbid(unsafe_code)]
#![cfg_attr(not(debug_assertions), deny(warnings))]

pub mod swap;
pub use stasis_assets as assets;
