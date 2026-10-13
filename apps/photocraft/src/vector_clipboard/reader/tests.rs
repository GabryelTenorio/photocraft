use super::*;
use std::cell::Cell;
use std::rc::Rc;

const SVG: &[u8] = b"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"2\" height=\"3\"/>";

#[derive(Default)]
struct Calls {
    size: Cell<usize>,
    copy: Cell<usize>,
    closed: Cell<usize>,
}

#[derive(Default)]
struct Lock(Rc<Calls>);

impl std::ops::Deref for Lock {
    type Target = Rc<Calls>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        self.closed.set(self.closed.get() + 1);
    }
}

#[derive(Default)]
struct MockClipboard {
    svg: Option<Vec<u8>>,
    text: Option<Vec<u8>>,
    reported_size: Option<usize>,
    copied: Option<usize>,
    read_error: bool,
    calls: Lock,
}

impl MockClipboard {
    fn bytes(&self, format: Format) -> Option<&[u8]> {
        match format {
            Format::Svg => self.svg.as_deref(),
            Format::Text => self.text.as_deref(),
        }
    }
}

impl ClipboardData for MockClipboard {
    fn available(&self, format: Format) -> bool {
        self.bytes(format).is_some()
    }

    fn size(&self, format: Format) -> Result<usize, String> {
        self.calls.size.set(self.calls.size.get() + 1);
        self.reported_size.or_else(|| self.bytes(format).map(<[u8]>::len)).ok_or_else(|| "missing format".into())
    }

    fn copy(&mut self, format: Format, out: &mut [u8]) -> Result<usize, String> {
        self.calls.copy.set(self.calls.copy.get() + 1);
        if self.read_error {
            return Err("read failed".into());
        }
        let bytes = self.bytes(format).unwrap();
        out.copy_from_slice(bytes);
        Ok(self.copied.unwrap_or(bytes.len()))
    }

    fn sequence(&self) -> Option<u32> {
        Some(42)
    }
}

fn text_bytes(text: &str) -> Vec<u8> {
    text.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect()
}

#[test]
fn registered_svg_wins_over_unicode_text_in_one_locked_snapshot() {
    let clipboard = MockClipboard { svg: Some(SVG.to_vec()), text: Some(text_bytes("<svg/>")), ..Default::default() };
    let calls = clipboard.calls.clone();
    let payload = read_with(|| Ok(clipboard)).unwrap().unwrap();
    assert_eq!(payload.svg, SVG);
    assert_eq!(payload.source, VectorClipboardSource::RegisteredSvg);
    assert_eq!(payload.sequence, Some(42));
    assert_eq!(calls.size.get(), 1);
    assert_eq!(calls.copy.get(), 1);
    assert_eq!(calls.closed.get(), 1);
}

#[test]
fn unicode_svg_preserves_non_ascii_and_surrogate_pairs() {
    let svg = "\u{feff}<?xml version=\"1.0\"?>\n<!-- synthetic -->\n<svg><text>Grün 文 🦀</text></svg>";
    let payload = read_with(|| Ok(MockClipboard { text: Some(text_bytes(svg)), ..Default::default() })).unwrap().unwrap();
    assert_eq!(payload.svg, svg.as_bytes());
    assert_eq!(payload.source, VectorClipboardSource::SvgText);
}

#[test]
fn registered_svg_removes_only_trailing_native_nuls() {
    let svg = [b"\xef\xbb\xbf".as_slice(), SVG, &[0, 0, 0]].concat();
    let payload = read_with(|| Ok(MockClipboard { svg: Some(svg), ..Default::default() })).unwrap().unwrap();
    assert_eq!(payload.svg, [b"\xef\xbb\xbf".as_slice(), SVG].concat());
    assert!(read_with(|| Ok(MockClipboard { svg: Some(b"<svg>\0</svg>".to_vec()), ..Default::default() })).is_err());
}

#[test]
fn no_format_and_ordinary_text_are_quiet() {
    let clipboard = MockClipboard::default();
    let calls = clipboard.calls.clone();
    assert!(read_with(|| Ok(clipboard)).unwrap().is_none());
    assert_eq!(calls.size.get(), 0);
    assert_eq!(calls.copy.get(), 0);
    assert_eq!(calls.closed.get(), 1);
    for text in ["", "hello", "quoted <svg/>", "<svgarbage/>", "<html><svg/></html>", "<?xml-stylesheet?><svg/>", "<!-- unfinished <svg/>"] {
        assert!(read_with(|| Ok(MockClipboard { text: Some(text_bytes(text)), ..Default::default() })).unwrap().is_none(), "{text:?}");
    }
}

#[test]
fn registered_svg_rejects_wrong_roots_and_invalid_encoding_without_fallback() {
    for svg in [Vec::new(), b"<svgarbage/>".to_vec(), b"<html><svg/></html>".to_vec(), vec![0xff, 0xfe], b"<!-- cut short".to_vec()] {
        let clipboard = MockClipboard { svg: Some(svg), text: Some(text_bytes("<svg/>")), ..Default::default() };
        let calls = clipboard.calls.clone();
        assert!(read_with(|| Ok(clipboard)).is_err());
        assert_eq!(calls.size.get(), 1, "never silently fall back from advertised SVG");
        assert_eq!(calls.closed.get(), 1);
    }
}

#[test]
fn limits_are_checked_before_reading_or_allocating_payloads() {
    for format in [Format::Svg, Format::Text] {
        let clipboard = MockClipboard {
            svg: (format == Format::Svg).then(Vec::new),
            text: (format == Format::Text).then(Vec::new),
            reported_size: Some(MAX_VECTOR_CLIPBOARD_BYTES + 1),
            ..Default::default()
        };
        let calls = clipboard.calls.clone();
        assert!(read_with(|| Ok(clipboard)).unwrap_err().contains("16 MiB"));
        assert_eq!(calls.copy.get(), 0);
        assert_eq!(calls.closed.get(), 1);
    }
    let mut svg = SVG.to_vec();
    svg.resize(MAX_VECTOR_CLIPBOARD_BYTES, b' ');
    assert_eq!(read_with(|| Ok(MockClipboard { svg: Some(svg), ..Default::default() })).unwrap().unwrap().svg.len(), MAX_VECTOR_CLIPBOARD_BYTES);
}

#[test]
fn normalized_utf8_output_is_capped_even_when_utf16_input_fits() {
    let mut bytes = Vec::new();
    for _ in 0..(MAX_VECTOR_CLIPBOARD_BYTES / 3 + 1) {
        bytes.extend_from_slice(&('文' as u16).to_le_bytes());
    }
    bytes.extend_from_slice(&[0, 0]);
    assert!(bytes.len() < MAX_VECTOR_CLIPBOARD_BYTES);
    assert!(read_with(|| Ok(MockClipboard { text: Some(bytes), ..Default::default() })).unwrap_err().contains("16 MiB"));
}

#[test]
fn malformed_unicode_is_reported() {
    for bytes in [vec![b'<'], vec![0x00, 0xd8, 0, 0], vec![0x00, 0xdc, 0, 0], vec![b'a', 0]] {
        assert!(read_with(|| Ok(MockClipboard { text: Some(bytes), ..Default::default() })).is_err());
    }
}

#[test]
fn busy_read_failure_and_short_reads_do_not_produce_payloads() {
    let busy = read_with::<MockClipboard>(|| Err("clipboard busy".into()));
    assert_eq!(busy.unwrap_err(), "clipboard busy");
    for (read_error, copied) in [(true, None), (false, Some(SVG.len() - 1)), (false, Some(SVG.len() + 1))] {
        let clipboard = MockClipboard { svg: Some(SVG.to_vec()), read_error, copied, ..Default::default() };
        let calls = clipboard.calls.clone();
        assert!(read_with(|| Ok(clipboard)).is_err());
        assert_eq!(calls.closed.get(), 1);
    }
}

#[test]
fn candidate_requires_svg_root_token_after_optional_prolog_and_comments() {
    for valid in ["<svg/>", "<svg></svg>", " \n<svg\twidth=\"2\"/>", "<?xml version=\"1.0\"?><svg/>", "<!-- one --><!-- two --> <svg/>"] {
        assert!(svg_candidate(valid), "{valid}");
    }
    for invalid in ["<SVG/>", "<svg", "<svg:other/>", "<?xml?><svg/>", "<!DOCTYPE svg><svg/>", "<?xml version=\"1.0\" <svg/>", "<div><svg/></div>"] {
        assert!(!svg_candidate(invalid), "{invalid}");
    }
}
