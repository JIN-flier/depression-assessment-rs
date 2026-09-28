//! Shared domain model for the depression assessment system.
//!
//! This crate deliberately contains data and local invariants only. It has no
//! dependency on UI, persistence, signal-processing, model-runtime, or LLM
//! implementations. Those crates communicate by using the types exported here.

#![forbid(unsafe_code)]

pub mod assessment;
pub mod eeg;
pub mod error;
pub mod evidence;
pub mod features;
pub mod identifiers;
pub mod model;
pub mod quality;
pub mod report;
pub mod scale;
pub mod subject;
pub mod types;
pub mod visit;

pub use assessment::*;
pub use eeg::*;
pub use error::*;
pub use evidence::*;
pub use features::*;
pub use identifiers::*;
pub use model::*;
pub use quality::*;
pub use report::*;
pub use scale::*;
pub use subject::*;
pub use types::*;
pub use visit::*;
