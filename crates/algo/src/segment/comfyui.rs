//! ComfyUI HTTP backend for ML model segmentation.
//!
//! This module implements communication with a ComfyUI server running segmentation models:
//! - BiRefNet: High-quality dichotomous image segmentation for Select Subject and Remove Background
//! - SAM 2.1: Segment Anything Model for interactive object selection
//!
//! ## ComfyUI Setup
//!
//! Users must run ComfyUI locally (or on their network) with the following:
//! 1. ComfyUI installation: https://github.com/comfyanonymous/ComfyUI
//! 2. BiRefNet model: https://github.com/ZHO-ZHO-ZHO/ComfyUI-BiRefNet-ZHO
//! 3. SAM 2.1 model: https://github.com/kijai/ComfyUI-segment-anything-2
//!
//! ## Workflow Format
//!
//! ComfyUI workflows are JSON graphs where nodes process data. We construct workflows
//! programmatically that:
//! 1. Load the input image (as base64 PNG)
//! 2. Run the segmentation model (BiRefNet or SAM 2.1)
//! 3. Return the mask as base64 PNG
//!
//! ## Error Handling
//!
//! Network errors, model errors, or missing models return `None` to allow fallback to classical algorithms.

use photocraft_geom::Rect;
use super::{Region, Sampler, RgbImage};

/// ComfyUI HTTP client for model segmentation.
pub struct ComfyUIClient {
    base_url: String,
    timeout_ms: u64,
}

impl ComfyUIClient {
    /// Creates a new ComfyUI client.
    ///
    /// - `base_url`: ComfyUI server URL (e.g., "http://127.0.0.1:8188")
    /// - `timeout_ms`: Request timeout in milliseconds (default: 30000 = 30 seconds)
    pub fn new(base_url: String) -> Self {
        Self {
            base_url,
            timeout_ms: 30_000,
        }
    }

    /// Sets the request timeout in milliseconds.
    #[cfg_attr(not(test), allow(dead_code))] // Not wired to preferences yet.
    pub fn with_timeout(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = timeout_ms;
        self
    }

    /// Tests if the ComfyUI server is available and responsive.
    pub fn is_available(&self) -> bool {
        #[cfg(all(not(target_arch = "wasm32"), feature = "ml-segmentation"))]
        {
            // Quick health check: GET /system_stats or /queue should respond instantly
            let url = format!("{}/system_stats", self.base_url);
            match ureq::get(&url).timeout(std::time::Duration::from_millis(3000)).call() {
                Ok(response) => response.status() == 200,
                Err(_) => false,
            }
        }
        #[cfg(not(all(not(target_arch = "wasm32"), feature = "ml-segmentation")))]
        {
            false
        }
    }

    /// Segments the subject using BiRefNet.
    ///
    /// Returns a binary mask where 255 = subject, 0 = background.
    pub fn segment_subject(&self, sampler: &dyn Sampler, canvas: Rect) -> Option<Region> {
        #[cfg(all(not(target_arch = "wasm32"), feature = "ml-segmentation"))]
        {
            self.segment_with_birefnet(sampler, canvas)
        }
        #[cfg(not(all(not(target_arch = "wasm32"), feature = "ml-segmentation")))]
        {
            let _ = (sampler, canvas);
            None
        }
    }

    /// Segments an object using SAM 2.1 with a bounding box prompt.
    pub fn segment_object(&self, sampler: &dyn Sampler, canvas: Rect, rect: Rect) -> Option<Region> {
        #[cfg(all(not(target_arch = "wasm32"), feature = "ml-segmentation"))]
        {
            self.segment_with_sam(sampler, canvas, rect)
        }
        #[cfg(not(all(not(target_arch = "wasm32"), feature = "ml-segmentation")))]
        {
            let _ = (sampler, canvas, rect);
            None
        }
    }

    #[cfg(all(not(target_arch = "wasm32"), feature = "ml-segmentation"))]
    fn segment_with_birefnet(&self, sampler: &dyn Sampler, canvas: Rect) -> Option<Region> {
        // 1. Render the image
        let img = sampler.rgb(canvas);

        // 2. Encode as PNG
        let png_data = encode_rgb_to_png(&img)?;
        let base64_image = base64_encode(&png_data);

        // 3. Construct ComfyUI workflow for BiRefNet
        let workflow = serde_json::json!({
            "1": {
                "inputs": {
                    "image": base64_image,
                },
                "class_type": "LoadImage",
            },
            "2": {
                "inputs": {
                    "image": ["1", 0],
                },
                "class_type": "BiRefNetSegmentation",
            },
            "3": {
                "inputs": {
                    "images": ["2", 0],
                },
                "class_type": "SaveImage",
            }
        });

        // 4. Send workflow to ComfyUI
        let response = self.execute_workflow(workflow)?;

        // 5. Decode the mask
        let mask_data = response.get("mask")?.as_str()?;
        let mask_png = base64_decode(mask_data)?;

        // 6. Convert PNG to Region
        decode_png_to_region(&mask_png, canvas)
    }

    #[cfg(all(not(target_arch = "wasm32"), feature = "ml-segmentation"))]
    fn segment_with_sam(&self, sampler: &dyn Sampler, canvas: Rect, rect: Rect) -> Option<Region> {
        // Similar to BiRefNet but includes bounding box coordinates
        let img = sampler.rgb(canvas);
        let png_data = encode_rgb_to_png(&img)?;
        let base64_image = base64_encode(&png_data);

        // Normalize rect coordinates to [0, 1] relative to canvas
        let x0 = (rect.x0 - canvas.x0) as f32 / canvas.width() as f32;
        let y0 = (rect.y0 - canvas.y0) as f32 / canvas.height() as f32;
        let x1 = (rect.x1 - canvas.x0) as f32 / canvas.width() as f32;
        let y1 = (rect.y1 - canvas.y0) as f32 / canvas.height() as f32;

        let workflow = serde_json::json!({
            "1": {
                "inputs": {
                    "image": base64_image,
                },
                "class_type": "LoadImage",
            },
            "2": {
                "inputs": {
                    "image": ["1", 0],
                    "box_x0": x0,
                    "box_y0": y0,
                    "box_x1": x1,
                    "box_y1": y1,
                },
                "class_type": "SAM2Segmentation",
            },
            "3": {
                "inputs": {
                    "images": ["2", 0],
                },
                "class_type": "SaveImage",
            }
        });

        let response = self.execute_workflow(workflow)?;
        let mask_data = response.get("mask")?.as_str()?;
        let mask_png = base64_decode(mask_data)?;
        decode_png_to_region(&mask_png, canvas)
    }

    #[cfg(all(not(target_arch = "wasm32"), feature = "ml-segmentation"))]
    fn execute_workflow(&self, workflow: serde_json::Value) -> Option<serde_json::Value> {
        let url = format!("{}/prompt", self.base_url);
        let payload = serde_json::json!({
            "prompt": workflow,
            "client_id": "photocraft",
        });

        // Send the workflow
        let response = ureq::post(&url)
            .timeout(std::time::Duration::from_millis(self.timeout_ms))
            .send_json(&payload)
            .ok()?;

        if response.status() != 200 {
            return None;
        }

        // Parse the response
        let result: serde_json::Value = response.into_json().ok()?;

        // Extract the prompt_id
        let prompt_id = result.get("prompt_id")?.as_str()?;

        // Poll for completion (simplified; real implementation would use websockets)
        for _ in 0..60 {
            // Wait 500ms between polls
            std::thread::sleep(std::time::Duration::from_millis(500));

            // Check if the workflow is complete
            let history_url = format!("{}/history/{}", self.base_url, prompt_id);
            if let Ok(history_response) = ureq::get(&history_url).call()
                && let Ok(history) = history_response.into_json::<serde_json::Value>()
                && let Some(outputs) = history.get(prompt_id)?.get("outputs")
            {
                // Workflow complete, extract the output
                return Some(outputs.clone());
            }
        }

        None // Timeout after 30 seconds
    }
}

/// Encodes an RGB image to PNG bytes.
#[cfg(all(not(target_arch = "wasm32"), feature = "ml-segmentation"))]
fn encode_rgb_to_png(img: &RgbImage) -> Option<Vec<u8>> {
    // Convert f32 RGB to u8 RGB
    let mut rgb_u8 = Vec::with_capacity(img.w * img.h * 3);
    for pixel in &img.px {
        rgb_u8.push((pixel[0].clamp(0.0, 1.0) * 255.0).round() as u8);
        rgb_u8.push((pixel[1].clamp(0.0, 1.0) * 255.0).round() as u8);
        rgb_u8.push((pixel[2].clamp(0.0, 1.0) * 255.0).round() as u8);
    }

    // Encode as PNG using simple uncompressed format
    // This creates a valid PNG with IDAT chunks
    let mut png_data = Vec::new();

    // PNG signature
    png_data.extend_from_slice(&[137, 80, 78, 71, 13, 10, 26, 10]);

    // IHDR chunk
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(img.w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(img.h as u32).to_be_bytes());
    ihdr.push(8); // bit depth
    ihdr.push(2); // color type (RGB)
    ihdr.push(0); // compression
    ihdr.push(0); // filter
    ihdr.push(0); // interlace
    write_chunk(&mut png_data, b"IHDR", &ihdr);

    // IDAT chunk - write raw pixel data with filter byte 0 (none) per scanline
    let mut idat_data = Vec::new();
    for y in 0..img.h {
        idat_data.push(0); // filter type: none
        idat_data.extend_from_slice(&rgb_u8[y * img.w * 3..(y + 1) * img.w * 3]);
    }

    // Simple zlib compression (deflate with minimal header/footer)
    let compressed = miniz_compress(&idat_data)?;
    write_chunk(&mut png_data, b"IDAT", &compressed);

    // IEND chunk
    write_chunk(&mut png_data, b"IEND", &[]);

    Some(png_data)
}

#[cfg(all(not(target_arch = "wasm32"), feature = "ml-segmentation"))]
fn write_chunk(out: &mut Vec<u8>, chunk_type: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(chunk_type);
    out.extend_from_slice(data);
    let crc = crc32(chunk_type, data);
    out.extend_from_slice(&crc.to_be_bytes());
}

#[cfg(all(not(target_arch = "wasm32"), feature = "ml-segmentation"))]
fn crc32(chunk_type: &[u8], data: &[u8]) -> u32 {
    let mut crc = 0xFFFFFFFFu32;
    for &b in chunk_type.iter().chain(data.iter()) {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 { 0xEDB88320 ^ (crc >> 1) } else { crc >> 1 };
        }
    }
    !crc
}

#[cfg(all(not(target_arch = "wasm32"), feature = "ml-segmentation"))]
fn miniz_compress(data: &[u8]) -> Option<Vec<u8>> {
    // Simple DEFLATE compression using flate2
    // For now, return uncompressed deflate blocks (valid but inefficient)
    let mut out = Vec::new();

    // Zlib header (CMF and FLG bytes)
    out.push(0x78); // CMF: deflate, 32K window
    out.push(0x01); // FLG: no preset dict, fastest compression

    // Write uncompressed blocks
    let max_block = 65535;
    let mut pos = 0;
    while pos < data.len() {
        let block_len = (data.len() - pos).min(max_block);
        let is_final = pos + block_len >= data.len();

        // Block header
        out.push(if is_final { 0x01 } else { 0x00 });
        out.extend_from_slice(&(block_len as u16).to_le_bytes());
        out.extend_from_slice(&(!(block_len as u16)).to_le_bytes());

        // Block data
        out.extend_from_slice(&data[pos..pos + block_len]);
        pos += block_len;
    }

    // Adler-32 checksum
    let checksum = adler32(data);
    out.extend_from_slice(&checksum.to_be_bytes());

    Some(out)
}

#[cfg(all(not(target_arch = "wasm32"), feature = "ml-segmentation"))]
fn adler32(data: &[u8]) -> u32 {
    let mut a = 1u32;
    let mut b = 0u32;
    for &byte in data {
        a = (a + u32::from(byte)) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

/// Base64 encodes binary data.
#[cfg(all(not(target_arch = "wasm32"), feature = "ml-segmentation"))]
fn base64_encode(data: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::with_capacity(data.len().div_ceil(3) * 4);

    for chunk in data.chunks(3) {
        let b1 = chunk[0];
        let b2 = chunk.get(1).copied().unwrap_or(0);
        let b3 = chunk.get(2).copied().unwrap_or(0);

        let n = ((b1 as u32) << 16) | ((b2 as u32) << 8) | (b3 as u32);

        result.push(CHARS[((n >> 18) & 63) as usize] as char);
        result.push(CHARS[((n >> 12) & 63) as usize] as char);
        result.push(if chunk.len() > 1 { CHARS[((n >> 6) & 63) as usize] as char } else { '=' });
        result.push(if chunk.len() > 2 { CHARS[(n & 63) as usize] as char } else { '=' });
    }

    result
}

/// Base64 decodes a string to binary data.
#[cfg(all(not(target_arch = "wasm32"), feature = "ml-segmentation"))]
fn base64_decode(data: &str) -> Option<Vec<u8>> {
    let data = data.trim();
    if data.is_empty() {
        return Some(Vec::new());
    }

    let mut result = Vec::with_capacity(data.len() * 3 / 4);
    let mut buf = 0u32;
    let mut bits = 0;

    for ch in data.chars() {
        let val = match ch {
            'A'..='Z' => (ch as u32) - ('A' as u32),
            'a'..='z' => (ch as u32) - ('a' as u32) + 26,
            '0'..='9' => (ch as u32) - ('0' as u32) + 52,
            '+' => 62,
            '/' => 63,
            '=' => break,
            _ => continue,
        };

        buf = (buf << 6) | val;
        bits += 6;

        if bits >= 8 {
            bits -= 8;
            result.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }

    Some(result)
}

/// Decodes a PNG mask to a Region.
#[cfg(all(not(target_arch = "wasm32"), feature = "ml-segmentation"))]
fn decode_png_to_region(png_data: &[u8], canvas: Rect) -> Option<Region> {
    // Parse PNG and extract grayscale/alpha mask
    // For now, implement a simple PNG reader that extracts the mask

    if png_data.get(..8) != Some(&[137, 80, 78, 71, 13, 10, 26, 10][..]) {
        return None; // Not a PNG
    }

    let mut pos: usize = 8;
    let mut width = 0u32;
    let mut height = 0u32;
    let mut bit_depth = 0u8;
    let mut color_type = 0u8;
    let mut idat_chunks = Vec::new();

    // Parse chunks
    // Chunk lengths come from the server: never trust them for slicing.
    while let Some(header) = png_data.get(pos..pos.checked_add(8)?) {
        let length = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as usize;
        let chunk_type = &header[4..8];
        let data_end = pos.checked_add(8)?.checked_add(length)?;
        let chunk_data = png_data.get(pos + 8..data_end)?;

        match chunk_type {
            b"IHDR" if chunk_data.len() >= 13 => {
                width = u32::from_be_bytes([chunk_data[0], chunk_data[1], chunk_data[2], chunk_data[3]]);
                height = u32::from_be_bytes([chunk_data[4], chunk_data[5], chunk_data[6], chunk_data[7]]);
                bit_depth = chunk_data[8];
                color_type = chunk_data[9];
            }
            b"IDAT" => {
                idat_chunks.push(chunk_data.to_vec());
            }
            b"IEND" => break,
            _ => {}
        }

        pos = data_end.checked_add(4)?; // skip the CRC
    }

    if width == 0 || height == 0 || width != canvas.width() || height != canvas.height() {
        return None;
    }

    // Decompress IDAT data
    let compressed: Vec<u8> = idat_chunks.into_iter().flatten().collect();
    let decompressed = miniz_decompress(&compressed)?;

    // Extract mask based on color type
    // 0 = grayscale, 4 = grayscale+alpha, 6 = RGBA
    let samples_per_pixel = match color_type {
        0 => 1, // Grayscale
        2 => 3, // RGB
        4 => 2, // Grayscale + Alpha
        6 => 4, // RGBA
        _ => return None,
    };

    let bytes_per_sample = if bit_depth == 16 { 2 } else { 1 };
    let bytes_per_pixel = samples_per_pixel * bytes_per_sample;
    let bytes_per_scanline = 1 + (width as usize) * bytes_per_pixel; // +1 for filter byte

    if decompressed.len() < bytes_per_scanline * (height as usize) {
        return None;
    }

    // Extract mask values (0-255)
    let mut mask = Vec::with_capacity((width * height) as usize);

    for y in 0..height as usize {
        let row_start = y * bytes_per_scanline;
        let _filter = decompressed[row_start];
        let row_data = &decompressed[row_start + 1..row_start + bytes_per_scanline];

        for x in 0..width as usize {
            let pixel_start = x * bytes_per_pixel;

            // Extract mask value based on color type
            let value = match color_type {
                0 => row_data[pixel_start], // Grayscale
                4 => row_data[pixel_start], // Grayscale (ignore alpha)
                2 | 6 => {
                    // RGB or RGBA: convert to grayscale (average)
                    let r = row_data[pixel_start];
                    let g = row_data[pixel_start + bytes_per_sample];
                    let b = row_data[pixel_start + 2 * bytes_per_sample];
                    ((r as u16 + g as u16 + b as u16) / 3) as u8
                }
                _ => 0,
            };

            mask.push(value);
        }
    }

    super::trim_region(Region { bbox: canvas, mask })
}

#[cfg(all(not(target_arch = "wasm32"), feature = "ml-segmentation"))]
fn miniz_decompress(data: &[u8]) -> Option<Vec<u8>> {
    // Simple zlib decompression
    if data.len() < 6 {
        return None;
    }

    // Skip zlib header (2 bytes)
    let deflate_data = &data[2..data.len() - 4]; // Skip header and Adler-32

    let mut result = Vec::new();
    let mut pos = 0;

    while pos < deflate_data.len() {
        let header = deflate_data[pos];
        pos += 1;

        let is_final = (header & 1) != 0;
        let block_type = (header >> 1) & 3;

        if block_type == 0 {
            // Uncompressed block
            if pos + 4 > deflate_data.len() {
                return None;
            }
            let len = u16::from_le_bytes([deflate_data[pos], deflate_data[pos + 1]]) as usize;
            pos += 4; // Skip len and nlen

            if pos + len > deflate_data.len() {
                return None;
            }
            result.extend_from_slice(&deflate_data[pos..pos + len]);
            pos += len;
        } else {
            // Compressed blocks not supported in this simple implementation
            // For production, use a proper deflate library
            return None;
        }

        if is_final {
            break;
        }
    }

    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIG: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];

    /// A PNG written by the independent `png` crate (real, compressed deflate).
    fn png_crate_encode(w: u32, h: u32, color: png::ColorType, depth: png::BitDepth, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, w, h);
            enc.set_color(color);
            enc.set_depth(depth);
            if color == png::ColorType::Indexed {
                enc.set_palette(vec![0; 3 * 256]);
            }
            let mut wr = enc.write_header().unwrap();
            wr.write_image_data(data).unwrap();
        }
        out
    }

    /// A gray-8 PNG in the stored-deflate form this module's decoder understands.
    fn stored_gray_png(w: usize, h: usize, px: impl Fn(usize, usize) -> u8) -> Vec<u8> {
        let mut out = SIG.to_vec();
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&(w as u32).to_be_bytes());
        ihdr.extend_from_slice(&(h as u32).to_be_bytes());
        ihdr.extend_from_slice(&[8, 0, 0, 0, 0]);
        write_chunk(&mut out, b"IHDR", &ihdr);
        let mut raw = Vec::new();
        for y in 0..h {
            raw.push(0);
            raw.extend((0..w).map(|x| px(x, y)));
        }
        write_chunk(&mut out, b"IDAT", &miniz_compress(&raw).unwrap());
        write_chunk(&mut out, b"IEND", &[]);
        out
    }

    #[test]
    fn crc32_matches_the_iend_vector() {
        assert_eq!(crc32(b"IEND", &[]), 0xAE42_6082);
    }

    #[test]
    fn adler32_matches_the_reference_vector() {
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
        assert_eq!(adler32(&[]), 1);
    }

    #[test]
    fn base64_round_trips_every_length_and_matches_rfc4648() {
        for (raw, enc) in [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foob", "Zm9vYg=="), ("fooba", "Zm9vYmE="), ("foobar", "Zm9vYmFy")] {
            assert_eq!(base64_encode(raw.as_bytes()), enc);
            assert_eq!(base64_decode(enc).unwrap(), raw.as_bytes());
        }
        for n in 0..=64usize {
            let data: Vec<u8> = (0..n).map(|i| (i * 37 + 11) as u8).collect();
            assert_eq!(base64_decode(&base64_encode(&data)).unwrap(), data, "len {n}");
        }
    }

    #[test]
    fn base64_decode_tolerates_garbage_without_panicking() {
        for s in ["====", "Z", "Zg", "\u{0}\u{ffff}", "ab!@#cd", " \n\t"] {
            let _ = base64_decode(s);
        }
    }

    #[test]
    fn stored_deflate_round_trips_across_block_boundaries() {
        for n in [0usize, 1, 65_534, 65_535, 65_536, 200_000] {
            let data: Vec<u8> = (0..n).map(|i| (i % 251) as u8).collect();
            let z = miniz_compress(&data).unwrap();
            assert_eq!(miniz_decompress(&z).unwrap_or_default(), data, "len {n}");
        }
    }

    #[test]
    fn encoded_png_decodes_with_an_independent_reader() {
        let img = RgbImage::from_fn(37, 23, |x, y| [x as f32 / 36.0, y as f32 / 22.0, 0.5]);
        let bytes = encode_rgb_to_png(&img).unwrap();
        let dec = png::Decoder::new(std::io::Cursor::new(bytes));
        let mut r = dec.read_info().unwrap();
        let mut buf = vec![0; r.output_buffer_size().unwrap()];
        let info = r.next_frame(&mut buf).unwrap();
        assert_eq!((info.width, info.height, info.color_type, info.bit_depth), (37, 23, png::ColorType::Rgb, png::BitDepth::Eight));
        for (i, p) in img.px.iter().enumerate() {
            let want = p.map(|v| (v * 255.0).round() as u8);
            assert_eq!(buf[i * 3..i * 3 + 3], want, "pixel {i}");
        }
    }

    #[test]
    fn decode_gray_mask_to_region() {
        let canvas = Rect::new(0, 0, 20, 10);
        let png = stored_gray_png(20, 10, |x, _| if (5..15).contains(&x) { 255 } else { 0 });
        let r = decode_png_to_region(&png, canvas).unwrap();
        assert!(r.at(10, 5) >= 0.99 && r.at(2, 5) <= 0.01 && r.at(17, 5) <= 0.01);
        // An all-zero mask is no region at all.
        assert_eq!(decode_png_to_region(&stored_gray_png(20, 10, |_, _| 0), canvas), None);
    }

    #[test]
    fn decode_rejects_a_mask_of_the_wrong_size() {
        let png = stored_gray_png(20, 10, |_, _| 255);
        assert_eq!(decode_png_to_region(&png, Rect::new(0, 0, 21, 10)), None);
        assert_eq!(decode_png_to_region(&png, Rect::new(0, 0, 0, 0)), None);
    }

    /// Rule 9 regression: chunk lengths come from the server. A lying length used to slice past
    /// the end of the buffer and panic.
    #[test]
    fn decode_png_with_hostile_chunks_does_not_panic() {
        let good = stored_gray_png(8, 8, |_, _| 200);
        let canvas = Rect::new(0, 0, 8, 8);
        // Every truncation of a valid file. Cutting into the pixel data finds nothing; a missing
        // IDAT CRC or IEND trailer (the last 16 bytes) is tolerated.
        for n in 0..good.len() {
            let r = decode_png_to_region(&good[..n], canvas);
            if n < good.len() - 16 {
                assert_eq!(r, None, "truncated to {n}");
            }
        }
        // IHDR length patched to huge values (overflow on 32-bit if added unchecked).
        for len in [u32::MAX, u32::MAX - 7, 0x7FFF_FFFF, 1_000_000, 14] {
            let mut bad = good.clone();
            bad[8..12].copy_from_slice(&len.to_be_bytes());
            assert_eq!(decode_png_to_region(&bad, canvas), None, "length {len}");
        }
        // Signature only, wrong signature, and noise.
        assert_eq!(decode_png_to_region(&SIG, canvas), None);
        assert_eq!(decode_png_to_region(b"GIF89a....", canvas), None);
        let mut noise = SIG.to_vec();
        noise.extend((0..512u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8));
        let _ = decode_png_to_region(&noise, canvas);
    }

    #[test]
    fn decode_png_with_an_absurd_ihdr_does_not_allocate_or_panic() {
        let mut out = SIG.to_vec();
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&u32::MAX.to_be_bytes());
        ihdr.extend_from_slice(&u32::MAX.to_be_bytes());
        ihdr.extend_from_slice(&[16, 6, 0, 0, 0]);
        write_chunk(&mut out, b"IHDR", &ihdr);
        write_chunk(&mut out, b"IDAT", &miniz_compress(&[0; 16]).unwrap());
        write_chunk(&mut out, b"IEND", &[]);
        assert_eq!(decode_png_to_region(&out, Rect::new(0, 0, 64, 64)), None);
    }

    #[test]
    fn decode_png_with_unsupported_layouts_does_not_panic() {
        let canvas = Rect::new(0, 0, 4, 4);
        // Palette, 1-bit gray and 16-bit RGBA through a real (compressed) encoder.
        let pal = png_crate_encode(4, 4, png::ColorType::Indexed, png::BitDepth::Eight, &[0; 16]);
        let one = png_crate_encode(4, 4, png::ColorType::Grayscale, png::BitDepth::One, &[0xF0; 4]);
        let rgba16 = png_crate_encode(4, 4, png::ColorType::Rgba, png::BitDepth::Sixteen, &[0xFF; 128]);
        for p in [pal, one, rgba16] {
            let _ = decode_png_to_region(&p, canvas);
        }
    }

    /// Known limitation, pinned so it is not forgotten: the decoder only inflates stored
    /// (uncompressed) deflate blocks, so real compressed PNGs (what ComfyUI returns) are
    /// rejected. Flip this to a positive test when a real inflater replaces `miniz_decompress`.
    #[test]
    fn compressed_png_masks_are_not_supported_yet() {
        let png = png_crate_encode(4, 4, png::ColorType::Grayscale, png::BitDepth::Eight, &[255; 16]);
        assert_eq!(decode_png_to_region(&png, Rect::new(0, 0, 4, 4)), None);
    }

    #[test]
    fn unreachable_server_is_unavailable_and_finds_nothing() {
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let client = ComfyUIClient::new(format!("http://127.0.0.1:{port}")).with_timeout(1_000);
        assert!(!client.is_available());
        let img = RgbImage::from_fn(16, 16, |_, _| [0.5; 3]);
        let s = super::super::ImageSampler { img: &img, origin: (0, 0) };
        let canvas = Rect::new(0, 0, 16, 16);
        assert_eq!(client.segment_subject(&s, canvas), None);
        assert_eq!(client.segment_object(&s, canvas, Rect::new(2, 2, 10, 10)), None);
    }

    #[test]
    fn garbage_urls_fail_without_panicking() {
        let img = RgbImage::from_fn(8, 8, |_, _| [0.5; 3]);
        let s = super::super::ImageSampler { img: &img, origin: (0, 0) };
        for url in ["", "not a url", "file:///etc/passwd", "http://", "http://[::1", "ftp://127.0.0.1:1"] {
            let c = ComfyUIClient::new(url.to_string()).with_timeout(500);
            assert!(!c.is_available(), "{url:?}");
            assert_eq!(c.segment_subject(&s, Rect::new(0, 0, 8, 8)), None, "{url:?}");
        }
    }
}
