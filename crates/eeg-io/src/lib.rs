//! P3 EEG import boundary: format adapters produce the existing domain model.
//!
//! Readers never access a database, UI, filters, or model runtime. Callers own
//! identity and acquisition metadata, and can inject any `Read` stream in tests.
//! Samples use the P1 channel-major `Vec<Vec<f32>>` contract; EEG amplitudes are
//! calibrated to microvolts without filtering, re-referencing, or resampling.
//!
//! ```
//! use domain::{RecordingId, SubjectId};
//! use eeg_io::{CsvOptions, CsvReader, EegReader, ImportContext};
//!
//! # fn main() -> Result<(), eeg_io::EegIoError> {
//! let context = ImportContext::new(RecordingId::new(), SubjectId::new());
//! let reader = CsvReader::new(CsvOptions::new(250.0));
//! let recording = reader.read(&mut b"Fp1,Fp2\n1,-1\n2,-2\n".as_slice(), &context)?;
//! assert_eq!(recording.samples[0], [1.0, 2.0]);
//! # Ok(())
//! # }
//! ```

#![forbid(unsafe_code)]

mod common;
mod csv_reader;
mod edf;
mod error;
mod mat;

pub use common::{ImportContext, ImportLimits};
pub use csv_reader::{CsvOptions, CsvReader};
pub use edf::EdfReader;
pub use error::{EegIoError, EegIoResult};
pub use mat::{MatLayout, MatOptions, MatReader, MatSamplingRate};

use domain::EegRecording;
use std::{fs::File, io::Read, path::Path};

/// Object-safe adapter contract. Parsing is synchronous and should run in a
/// background/blocking task when integrated with the desktop application.
pub trait EegReader: Send + Sync {
    fn read(&self, input: &mut dyn Read, context: &ImportContext) -> EegIoResult<EegRecording>;

    /// Adapter selection stays with callers because CSV/MAT need schema options.
    fn read_path(&self, path: &Path, context: &ImportContext) -> EegIoResult<EegRecording> {
        let mut file = File::open(path)?;
        self.read(&mut file, context)
    }
}
