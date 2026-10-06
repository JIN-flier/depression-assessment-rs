//! Independent per-channel DC removal: channels never share statistics or state.
use crate::{SignalResult, error::sample};

pub(crate) fn remove(row: &mut [f32]) -> SignalResult<()> {
    // f64 accumulation avoids long f32 records losing small amplitudes around a
    // large offset. Boundary validation guarantees a nonempty, finite row.
    let mean = row.iter().map(|&x| f64::from(x)).sum::<f64>() / row.len() as f64;
    for value in row {
        *value = sample(f64::from(*value) - mean, "dc_removal")?;
    }
    Ok(())
}
