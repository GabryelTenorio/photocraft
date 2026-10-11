//! Olympus/OM System predictive sensor storage, measured from CC0 camera files.
//!
//! The public Olympus patents US7333035B2, US7715639B2, US7259698B2 and
//! JP2006211155A informed the experiments, but do not specify this ORF stream.
//! The headers, entropy parameters, escape marker, residual correction and
//! gradient threshold below come from independent measurements, not from a
//! third-party decoder. In particular, ORF retains a one-bit marker between
//! the escaped quotient and remainder, unlike US7259698B2's example.

use crate::error::{RawError, Result};

const HEADER_12: &[u8] = &[0, 0, 0, 0, 1, 0, 0];
const HEADER_14: &[u8] = &[0, 0, 0, 0, 0, 1, 0, 0];
const ESCAPE: u32 = 12;
const MAGNITUDE_BITS: u32 = 15;

/// The caller checks the sensor dimensions against its configured `Limits`.
pub(crate) fn decode(bytes: &[u8], width: usize, height: usize, bits: u8) -> Result<Vec<u16>> {
    let header = match bits {
        12 => HEADER_12,
        14 => HEADER_14,
        _ => return Err(RawError::unsupported("ORF compressed precision is not 12 or 14 bits")),
    };
    let actual = bytes.get(..header.len()).ok_or_else(|| RawError::malformed("ORF compressed header truncated"))?;
    if actual != header {
        return Err(RawError::unsupported("ORF compressed stream header is not recognized"));
    }
    if width < 2 || height < 2 {
        return Err(RawError::malformed("ORF compressed sensor is smaller than a Bayer cell"));
    }
    if !width.is_multiple_of(2) {
        return Err(RawError::unsupported("ORF compressed sensor width is not even"));
    }
    let count = width.checked_mul(height).ok_or_else(|| RawError::malformed("ORF compressed dimensions overflow"))?;
    let previous_row = width.checked_mul(2).ok_or_else(|| RawError::malformed("ORF compressed row size overflow"))?;
    let payload = bytes.get(header.len()..).ok_or_else(|| RawError::malformed("ORF compressed payload missing"))?;
    // Every symbol needs at least sign + two low bits + delimiter + two
    // remainder bits. Reject impossible lengths before reserving the output.
    let minimum = count.checked_mul(3).map(|n| n.div_ceil(4)).ok_or_else(|| RawError::malformed("ORF compressed sample count overflow"))?;
    if payload.len() < minimum {
        return Err(RawError::malformed("ORF compressed payload is too short for its dimensions"));
    }
    let mut data = Vec::new();
    data.try_reserve_exact(count).map_err(|_| RawError::LimitExceeded("ORF compressed output allocation failed".into()))?;
    let mut reader = Bits { remaining: payload, buffer: 0, available: 0 };
    let maximum = (1i32 << bits) - 1;
    for y in 0..height {
        let mut even = State::default();
        let mut odd = State::default();
        for x in 0..width {
            let state = if x.is_multiple_of(2) { &mut even } else { &mut odd };
            let error = state.residual(&mut reader)?;
            let value = predict(&data, previous_row, x, y)? + error;
            if !(0..=maximum).contains(&value) {
                return Err(RawError::malformed("ORF compressed sample exceeds its precision"));
            }
            data.push(value as u16);
        }
    }
    reader.finish()?;
    Ok(data)
}

#[derive(Default)]
struct State {
    magnitude: u32,
    flat_run: u8,
    bias: i32,
}

impl State {
    #[inline]
    fn residual(&mut self, reader: &mut Bits<'_>) -> Result<i32> {
        let flat = self.flat_run >= 3;
        let previous_bits = u32::BITS - self.magnitude.leading_zeros();
        let k = if flat { previous_bits.max(2) } else { previous_bits.saturating_sub(2).max(4) };
        let sign = reader.read(1)?;
        let low = reader.read(2)?;
        let mut quotient = 0;
        while quotient < ESCAPE && reader.read(1)? == 0 {
            quotient += 1;
        }
        if quotient == ESCAPE {
            let high_bits = MAGNITUDE_BITS.checked_sub(k).ok_or_else(|| RawError::malformed("ORF compressed Rice parameter overflow"))?;
            quotient = reader.read(high_bits)?;
            if reader.read(1)? != 1 || quotient < ESCAPE {
                return Err(RawError::malformed("ORF compressed escape is invalid"));
            }
        }
        let magnitude = (quotient << k) | reader.read(k)?;
        if magnitude >= (1 << MAGNITUDE_BITS) {
            return Err(RawError::malformed("ORF compressed residual exceeds its code width"));
        }
        self.magnitude = magnitude;
        self.flat_run = if magnitude <= 16 { self.flat_run.saturating_add(1) } else { 0 };

        let signed = if sign == 0 { magnitude as i32 } else { -(magnitude as i32) - 1 };
        let corrected = signed + self.bias;
        // Magnitude is at most 32767. This recurrence contracts the previous
        // bias by 1/8, keeping every intermediate comfortably within i32.
        let error = corrected * 4 + low as i32;
        self.bias = (3 * corrected + self.bias).div_euclid(32);
        Ok(error)
    }
}

/// Same-color neighbours are two sensor positions apart in a Bayer image.
#[inline]
fn predict(data: &[u16], previous_row: usize, x: usize, y: usize) -> Result<i32> {
    let sample_before = |distance| {
        data.len()
            .checked_sub(distance)
            .and_then(|at| data.get(at))
            .copied()
            .map(i32::from)
            .ok_or_else(|| RawError::malformed("ORF compressed predictor position is invalid"))
    };
    if y < 2 {
        return if x < 2 { Ok(0) } else { sample_before(2) };
    }
    let above = sample_before(previous_row)?;
    if x < 2 {
        return Ok(above);
    }
    let left = sample_before(2)?;
    let diagonal_distance = previous_row.checked_add(2).ok_or_else(|| RawError::malformed("ORF compressed diagonal position overflow"))?;
    let diagonal = sample_before(diagonal_distance)?;
    let lower = left.min(above);
    let upper = left.max(above);
    Ok(if diagonal >= upper {
        lower
    } else if diagonal <= lower {
        upper
    } else if upper - diagonal <= 32 && diagonal - lower <= 32 {
        (left + above) / 2
    } else {
        left + above - diagonal
    })
}

/// At most 22 bits accumulate: callers request at most 15, and refill one byte
/// at a time. Consumed bits are removed, so the final buffered bits are padding.
struct Bits<'a> {
    remaining: &'a [u8],
    buffer: u32,
    available: u32,
}

impl Bits<'_> {
    #[inline]
    fn read(&mut self, count: u32) -> Result<u32> {
        if count > MAGNITUDE_BITS {
            return Err(RawError::malformed("ORF compressed bit field is too wide"));
        }
        while self.available < count {
            let (&byte, rest) = self.remaining.split_first().ok_or_else(|| RawError::malformed("ORF compressed data truncated"))?;
            self.remaining = rest;
            self.buffer = (self.buffer << 8) | u32::from(byte);
            self.available += 8;
        }
        self.available -= count;
        let value = self.buffer >> self.available;
        self.buffer &= (1 << self.available) - 1;
        Ok(value)
    }

    fn finish(self) -> Result<()> {
        if !self.remaining.is_empty() || self.available > 7 {
            return Err(RawError::malformed("ORF compressed stream has extra data"));
        }
        if self.buffer != (1 << self.available) - 1 {
            return Err(RawError::malformed("ORF compressed stream has invalid final padding"));
        }
        Ok(())
    }
}
