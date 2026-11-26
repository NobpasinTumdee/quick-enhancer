//! "Processed with generative AI" note in saved files: a PNG `tEXt` chunk (keyword `Comment`
//! plus `Software`) or a JPEG `COM` segment. Other formats are left unchanged.

use std::path::Path;

pub const NOTE: &str = "Processed with generative AI by Quick Enhancer";
/// Prefix of the job note of a generative result (the History entry is marked from it).
pub const NOTE_MARK: &str = "✦";

/// Insert the note into an encoded PNG / JPEG. Returns `None` for other data.
pub fn with_note(data: &[u8], note: &str) -> Option<Vec<u8>> {
    const PNG_SIG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    if data.len() > 33 && data[..8] == PNG_SIG {
        // Signature (8) + IHDR chunk (4 len + 4 type + 13 data + 4 crc) = 33 bytes; insert after it.
        let mut out = Vec::with_capacity(data.len() + note.len() + 64);
        out.extend_from_slice(&data[..33]);
        for (key, text) in [("Comment", note), ("Software", "Quick Enhancer")] {
            let mut body = Vec::with_capacity(key.len() + 1 + text.len());
            body.extend_from_slice(key.as_bytes());
            body.push(0);
            body.extend(text.bytes().filter(|b| b.is_ascii()));
            out.extend_from_slice(&(body.len() as u32).to_be_bytes());
            let mut crc = crc32fast::Hasher::new();
            crc.update(b"tEXt");
            crc.update(&body);
            out.extend_from_slice(b"tEXt");
            out.extend_from_slice(&body);
            out.extend_from_slice(&crc.finalize().to_be_bytes());
        }
        out.extend_from_slice(&data[33..]);
        return Some(out);
    }
    if data.len() > 4 && data[0] == 0xFF && data[1] == 0xD8 {
        let text: Vec<u8> = note.bytes().take(60_000).collect();
        let mut out = Vec::with_capacity(data.len() + text.len() + 4);
        out.extend_from_slice(&data[..2]);
        // Keep a leading APP0 (JFIF) segment first, as readers expect.
        let mut pos = 2;
        if data[2] == 0xFF && data[3] == 0xE0 && data.len() > 6 {
            let len = u16::from_be_bytes([data[4], data[5]]) as usize;
            pos = (2 + 2 + len).min(data.len());
            out.extend_from_slice(&data[2..pos]);
        }
        out.extend_from_slice(&[0xFF, 0xFE]);
        out.extend_from_slice(&((text.len() + 2) as u16).to_be_bytes());
        out.extend_from_slice(&text);
        out.extend_from_slice(&data[pos..]);
        return Some(out);
    }
    None
}

/// Add the note to a saved file in place (best effort: errors are logged, the image is kept).
pub fn mark_file(path: &Path) {
    let res = std::fs::read(path).and_then(|d| match with_note(&d, NOTE) {
        Some(v) => std::fs::write(path, v),
        None => Ok(()),
    });
    if let Err(e) = res {
        tracing::warn!("could not add the generative-AI note to {}: {e}", path.display());
    }
}

/// The note, when the file has one (PNG `tEXt` Comment or JPEG `COM`).
pub fn read_note(data: &[u8]) -> Option<String> {
    let find = |needle: &[u8]| data.windows(needle.len()).position(|w| w == needle);
    let p = find(NOTE.as_bytes())?;
    Some(String::from_utf8_lossy(&data[p..p + NOTE.len()]).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encoded(fmt: image::ImageFormat) -> Vec<u8> {
        let img = image::RgbImage::from_fn(24, 16, |x, y| image::Rgb([(x * 10) as u8, (y * 15) as u8, 90]));
        let mut c = std::io::Cursor::new(Vec::new());
        img.write_to(&mut c, fmt).unwrap();
        c.into_inner()
    }

    #[test]
    fn png_note_is_valid_and_readable() {
        let png = encoded(image::ImageFormat::Png);
        let out = with_note(&png, NOTE).unwrap();
        assert_eq!(read_note(&out).as_deref(), Some(NOTE));
        // The png decoder checks chunk CRCs: the file must still decode, with the text chunk.
        let dec = png::Decoder::new(std::io::Cursor::new(&out));
        let reader = dec.read_info().unwrap();
        let texts: Vec<_> = reader.info().uncompressed_latin1_text.iter().map(|t| (t.keyword.clone(), t.text.clone())).collect();
        assert!(texts.contains(&("Comment".into(), NOTE.into())), "{texts:?}");
        let img = image::load_from_memory(&out).unwrap();
        assert_eq!((img.width(), img.height()), (24, 16));
    }

    #[test]
    fn jpeg_note_is_valid_and_readable() {
        let jpg = encoded(image::ImageFormat::Jpeg);
        let out = with_note(&jpg, NOTE).unwrap();
        assert_eq!(read_note(&out).as_deref(), Some(NOTE));
        let img = image::load_from_memory(&out).unwrap();
        assert_eq!((img.width(), img.height()), (24, 16));
        assert!(read_note(&jpg).is_none());
    }

    #[test]
    fn other_formats_are_left_alone() {
        assert!(with_note(b"RIFF....WEBPVP8 ", NOTE).is_none());
    }
}
