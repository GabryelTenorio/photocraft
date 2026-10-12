//! Version-16 Photoshop action sets. Layout evidence is documented in
//! https://github.com/storytold/photocraft/pull/1257; descriptors reuse the PSD reader.
//! This reader does not claim to translate every Photoshop event into an editor command.

use crate::descriptor::{Descriptor, UnicodeString};
use crate::error::{PsdError, Result};
use crate::io::Reader;

/// Maximum input size and total steps, before creating an action library.
pub const MAX_BYTES: usize = 32 << 20;
const MAX_ITEMS: usize = 10_000;
const MAX_NAME_UNITS: u32 = 1024;

/// One action set, in file order.
#[derive(Debug, Clone, PartialEq)]
pub struct ActionSet {
    /// Name stored in the Photoshop set.
    pub name: String,
    /// Actions in panel order.
    pub actions: Vec<Action>,
}

/// A named sequence of Photoshop events.
#[derive(Debug, Clone, PartialEq)]
pub struct Action {
    /// Display name.
    pub name: String,
    /// Events in playback order.
    pub steps: Vec<Step>,
}

/// A recorded event, with its original playback flags and descriptor.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    /// Whether playback includes this step.
    pub enabled: bool,
    /// Whether Photoshop pauses to show the step's dialog.
    pub dialog: bool,
    /// Photoshop string id or four-character code.
    pub event: String,
    /// Recorded parameters, if any.
    pub descriptor: Option<Descriptor>,
}

fn count(r: &mut Reader<'_>, minimum: u64) -> Result<usize> {
    let n = r.u32()?;
    if n > MAX_ITEMS as u32 {
        return Err(PsdError::LimitExceeded("too many action records"));
    }
    r.check_count(u64::from(n), minimum)?;
    Ok(n as usize)
}

fn read_name(r: &mut Reader<'_>) -> Result<String> {
    // Guard before the shared reader allocates. Names eventually become UI galleys.
    if r.clone().u32()? > MAX_NAME_UNITS {
        return Err(PsdError::LimitExceeded("action names exceed 1024 UTF-16 units"));
    }
    Ok(UnicodeString::read(r)?.to_string_lossy())
}

fn byte_string(r: &mut Reader<'_>, limit: u32) -> Result<String> {
    let n = r.u32()?;
    if n > limit {
        return Err(PsdError::LimitExceeded("action event or step label is too long"));
    }
    Ok(r.bytes_u64(u64::from(n))?.iter().map(|b| char::from(*b)).collect())
}

/// Parse a complete `.atn` file. Truncation, excessive counts and trailing data are errors.
pub fn parse(bytes: &[u8]) -> Result<ActionSet> {
    if bytes.len() > MAX_BYTES {
        return Err(PsdError::LimitExceeded("action file exceeds 32 MiB"));
    }
    let mut r = Reader::new(bytes);
    let version = r.u32()?;
    if version != 16 {
        return Err(PsdError::Unsupported(format!("action file version {version}; expected 16")));
    }
    let name = read_name(&mut r)?;
    r.u8()?; // Set expansion is view state.
    let n = count(&mut r, 15)?;
    let mut actions = Vec::with_capacity(n);
    let mut total = 0usize;
    for _ in 0..n {
        r.skip(6)?; // Function key, modifiers and button colour are not replay steps.
        let name = read_name(&mut r)?;
        r.u8()?;
        let n = count(&mut r, 20)?;
        total = total.saturating_add(n);
        if total > MAX_ITEMS {
            return Err(PsdError::LimitExceeded("action set exceeds 10000 steps"));
        }
        let mut steps = Vec::with_capacity(n);
        for _ in 0..n {
            r.u8()?;
            let enabled = r.u8()? != 0;
            let dialog = r.u8()? != 0;
            r.u8()?;
            let event = match r.array::<4>()? {
                tag if &tag == b"TEXT" => byte_string(&mut r, 256)?,
                tag if &tag == b"long" => r.array::<4>()?.iter().map(|b| char::from(*b)).collect(),
                found => return Err(PsdError::InvalidSignature { expected: "TEXT or long event", found }),
            };
            byte_string(&mut r, 4096)?; // Localized step label; event id determines behaviour.
            let descriptor = match r.u32()? {
                0 => None,
                u32::MAX => Some(Descriptor::read(&mut r)?),
                other => return Err(PsdError::invalid(format!("invalid action descriptor marker {other}"))),
            };
            steps.push(Step { enabled, dialog, event, descriptor });
        }
        actions.push(Action { name, steps });
    }
    if !r.is_empty() {
        return Err(PsdError::invalid("trailing bytes after action set"));
    }
    Ok(ActionSet { name, actions })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::Value;

    fn text(out: &mut Vec<u8>, s: &str) {
        out.extend_from_slice(&(s.len() as u32).to_be_bytes());
        out.extend_from_slice(s.as_bytes());
    }

    fn unicode(out: &mut Vec<u8>, s: &str) {
        UnicodeString::new(s).write(out);
    }

    fn sample() -> Vec<u8> {
        let mut out = 16u32.to_be_bytes().to_vec();
        unicode(&mut out, "Studio 🎨");
        out.push(1);
        out.extend_from_slice(&1u32.to_be_bytes());
        out.extend_from_slice(&[0; 6]);
        unicode(&mut out, "Blur");
        out.push(1);
        out.extend_from_slice(&2u32.to_be_bytes());
        out.extend_from_slice(&[0, 1, 0, 2]);
        out.extend_from_slice(b"TEXT");
        text(&mut out, "gaussianBlur");
        text(&mut out, "Gaussian Blur");
        out.extend_from_slice(&u32::MAX.to_be_bytes());
        out.extend_from_slice(&Descriptor::new("GsnB").with("Rds ", Value::UnitFloat { unit: *b"#Pxl", value: 2.5 }).to_bytes());
        out.extend_from_slice(&[0, 0, 1, 2]);
        out.extend_from_slice(b"longInvr");
        text(&mut out, "Invert");
        out.extend_from_slice(&0u32.to_be_bytes());
        out
    }

    #[test]
    fn oversized_names_are_rejected_before_allocating() {
        let mut bytes = 16u32.to_be_bytes().to_vec();
        bytes.extend_from_slice(&(MAX_NAME_UNITS + 1).to_be_bytes());
        assert!(matches!(parse(&bytes), Err(PsdError::LimitExceeded(_))));
    }

    #[test]
    fn reads_names_events_descriptors_and_flags() {
        let set = parse(&sample()).unwrap();
        assert_eq!(set.name, "Studio 🎨");
        assert_eq!(set.actions[0].name, "Blur");
        let steps = &set.actions[0].steps;
        assert_eq!(steps[0].event, "gaussianBlur");
        assert!(steps[0].enabled);
        assert_eq!(steps[0].descriptor.as_ref().unwrap().get("Rds "), Some(&Value::UnitFloat { unit: *b"#Pxl", value: 2.5 }));
        assert_eq!(steps[1].event, "Invr");
        assert!(!steps[1].enabled);
        assert!(steps[1].dialog);
    }

    #[test]
    fn rejects_every_truncation_and_trailing_bytes() {
        let bytes = sample();
        for n in 0..bytes.len() {
            assert!(parse(&bytes[..n]).is_err(), "truncation {n}");
        }
        let mut extra = bytes;
        extra.push(0);
        assert!(parse(&extra).is_err());
    }

    #[test]
    fn malformed_versions_counts_and_mutations_never_panic() {
        let bytes = sample();
        for i in 0..bytes.len() {
            for mask in [1, 128, 255] {
                let mut altered = bytes.clone();
                altered[i] ^= mask;
                let _ = parse(&altered);
            }
        }
        let mut version = bytes;
        version[..4].copy_from_slice(&15u32.to_be_bytes());
        assert!(parse(&version).is_err());
        let mut oversized = 16u32.to_be_bytes().to_vec();
        unicode(&mut oversized, "");
        oversized.push(0);
        oversized.extend_from_slice(&u32::MAX.to_be_bytes());
        assert!(parse(&oversized).is_err());
    }
}
