//! P4: deterministic offline EEG preprocessing without UI, storage or model dependencies.
//!
//! Algorithms consume the P1 channel-major domain contract. The application owns
//! scheduling (use a blocking worker), timestamps and persistence. See `README.md`
//! for frequency response, boundary behavior and reproducibility guarantees.
//!
//! ```
//! use chrono::Utc;
//! use eeg_signal::{PipelineConfig, ProcessingContext, SignalPipeline, SignalStep};
//! # fn main() -> Result<(), eeg_signal::SignalError> {
//! let pipeline = SignalPipeline::new(PipelineConfig {
//!     steps: vec![SignalStep::DcRemoval {}, SignalStep::resample(250.0)],
//!     ..PipelineConfig::default()
//! })?;
//! let context = ProcessingContext::new(Utc::now());
//! // let processed = pipeline.process(&recording, &context)?;
//! # let _ = (pipeline, context);
//! # Ok(()) }
//! ```

#![forbid(unsafe_code)]

mod config;
mod dc;
mod error;
mod filter;
mod pipeline;
mod resample;

pub use config::{FilterPhase, PipelineConfig, ProcessingContext, ProcessingLimits, SignalStep};
pub use error::{SignalError, SignalResult};
pub use pipeline::{EegProcessor, SignalPipeline};
