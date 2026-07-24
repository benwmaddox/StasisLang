#![forbid(unsafe_code)]
#![cfg_attr(not(debug_assertions), deny(warnings))]

pub mod live;
pub mod swap;
pub mod tick;
pub use stasis_assets as assets;
