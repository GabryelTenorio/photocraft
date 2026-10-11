//! Bounded, explicit reads of vector data from the Windows clipboard.
//!
//! Registered SVG takes precedence over SVG published as Unicode text. The engine validates
//! the SVG document; this boundary only normalizes encoding and recognizes the root element.

use photocraft_ui_egui::VectorClipboard;

#[cfg(any(windows, test))]
use photocraft_ui_egui::{MAX_VECTOR_CLIPBOARD_BYTES, VectorClipboardSource};

pub(crate) fn read() -> Result<Option<VectorClipboard>, String> {
    #[cfg(windows)]
    {
        reader::read_with(windows::Clipboard::open)
    }
    #[cfg(not(windows))]
    {
        Ok(None)
    }
}

#[cfg(any(windows, test))]
mod reader {
    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(super) enum Format {
        Svg,
        Text,
    }

    /// The backend owns the clipboard lock for the entire read, including format selection.
    pub(super) trait ClipboardData {
        fn available(&self, format: Format) -> bool;
        fn size(&self, format: Format) -> Result<usize, String>;
        fn copy(&mut self, format: Format, out: &mut [u8]) -> Result<usize, String>;
        fn sequence(&self) -> Option<u32>;
    }

    pub(super) fn read_with<B: ClipboardData>(open: impl FnOnce() -> Result<B, String>) -> Result<Option<VectorClipboard>, String> {
        let mut clipboard = open()?;
        let format = if clipboard.available(Format::Svg) {
            Format::Svg
        } else if clipboard.available(Format::Text) {
            Format::Text
        } else {
            return Ok(None);
        };
        let size = clipboard.size(format)?;
        if size == 0 {
            return if format == Format::Text { Ok(None) } else { Err("The SVG clipboard data is empty".into()) };
        }
        if size > MAX_VECTOR_CLIPBOARD_BYTES {
            return Err("The vector clipboard data exceeds the 16 MiB limit".into());
        }
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(size).map_err(|_| "Not enough memory to read the vector clipboard data")?;
        bytes.resize(size, 0);
        let copied = clipboard.copy(format, &mut bytes)?;
        if copied != size {
            return Err("The vector clipboard data could not be read completely".into());
        }
        let svg = match format {
            Format::Svg => normalize_svg(bytes)?,
            Format::Text => {
                let text = unicode_text(&bytes)?;
                if !svg_candidate(&text) {
                    return Ok(None);
                }
                text.into_bytes()
            }
        };
        if !svg_candidate(std::str::from_utf8(&svg).map_err(|_| "The SVG clipboard data is not valid UTF-8")?) {
            return Err("The registered SVG clipboard format does not contain SVG markup".into());
        }
        Ok(Some(VectorClipboard {
            svg,
            source: match format {
                Format::Svg => VectorClipboardSource::RegisteredSvg,
                Format::Text => VectorClipboardSource::SvgText,
            },
            sequence: clipboard.sequence(),
        }))
    }

    fn normalize_svg(mut bytes: Vec<u8>) -> Result<Vec<u8>, String> {
        while bytes.last() == Some(&0) {
            bytes.pop();
        }
        if bytes.contains(&0) {
            return Err("The SVG clipboard data contains an embedded NUL".into());
        }
        std::str::from_utf8(&bytes).map_err(|_| "The SVG clipboard data is not valid UTF-8")?;
        Ok(bytes)
    }

    fn unicode_text(bytes: &[u8]) -> Result<String, String> {
        let (units, remainder) = bytes.as_chunks::<2>();
        if !remainder.is_empty() {
            return Err("The Unicode clipboard data has an invalid byte length".into());
        }
        if !units.contains(&[0, 0]) {
            return Err("The Unicode clipboard data is not NUL-terminated".into());
        }
        let mut text = String::new();
        // UTF-16 can expand to three UTF-8 bytes per code unit. Keep the normalized result capped.
        text.try_reserve(bytes.len().min(MAX_VECTOR_CLIPBOARD_BYTES)).map_err(|_| "Not enough memory to read the Unicode clipboard data")?;
        let units = units.iter().map(|pair| u16::from_le_bytes(*pair)).take_while(|unit| *unit != 0);
        for character in char::decode_utf16(units) {
            let character = character.map_err(|_| "The Unicode clipboard data is not valid UTF-16")?;
            let size = text.len().checked_add(character.len_utf8()).ok_or("The Unicode clipboard data is too large")?;
            if size > MAX_VECTOR_CLIPBOARD_BYTES {
                return Err("The vector clipboard data exceeds the 16 MiB limit".into());
            }
            text.try_reserve(character.len_utf8()).map_err(|_| "Not enough memory to read the Unicode clipboard data")?;
            text.push(character);
        }
        Ok(text)
    }

    /// Only recognize a root `<svg` token, never a substring in ordinary text or another XML root.
    fn svg_candidate(text: &str) -> bool {
        let mut rest = text.strip_prefix('\u{feff}').unwrap_or(text).trim_start_matches(xml_space);
        if let Some(after) = rest.strip_prefix("<?xml") {
            if !after.chars().next().is_some_and(xml_space) {
                return false;
            }
            let Some((_, after)) = after.split_once("?>") else { return false };
            rest = after.trim_start_matches(xml_space);
        }
        while let Some(after) = rest.strip_prefix("<!--") {
            let Some((_, after)) = after.split_once("-->") else { return false };
            rest = after.trim_start_matches(xml_space);
        }
        rest.strip_prefix("<svg").and_then(|after| after.chars().next()).is_some_and(|character| xml_space(character) || matches!(character, '/' | '>'))
    }

    fn xml_space(character: char) -> bool {
        matches!(character, ' ' | '\t' | '\r' | '\n')
    }

    #[cfg(test)]
    mod tests;
}

#[cfg(windows)]
mod windows {
    use super::reader::{ClipboardData, Format};
    use clipboard_win::{formats, raw};

    pub(super) struct Clipboard {
        _lock: clipboard_win::Clipboard,
        svg: u32,
    }

    impl Clipboard {
        pub(super) fn open() -> Result<Self, String> {
            let lock = clipboard_win::Clipboard::new_attempts(10).map_err(|error| format!("The clipboard is unavailable: {error}"))?;
            let svg = raw::register_format("image/svg+xml").ok_or("Could not register the SVG clipboard format")?.get();
            Ok(Self { _lock: lock, svg })
        }

        fn id(&self, format: Format) -> u32 {
            match format {
                Format::Svg => self.svg,
                Format::Text => formats::CF_UNICODETEXT,
            }
        }
    }

    impl ClipboardData for Clipboard {
        fn available(&self, format: Format) -> bool {
            raw::is_format_avail(self.id(format))
        }

        fn size(&self, format: Format) -> Result<usize, String> {
            raw::size(self.id(format)).map(|size| size.get()).ok_or_else(|| "Could not determine the vector clipboard data size".into())
        }

        fn copy(&mut self, format: Format, out: &mut [u8]) -> Result<usize, String> {
            raw::get(self.id(format), out).map_err(|error| format!("Could not read the vector clipboard data: {error}"))
        }

        fn sequence(&self) -> Option<u32> {
            raw::seq_num().map(|number| number.get())
        }
    }
}
