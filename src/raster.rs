//! Minimal CUPS raster reader: v3 (uncompressed) and v2 (compressed) streams,
//! either byte order, 1-bit single-colour pages only.

use std::io::{Error, ErrorKind, Result};

const HEADER_LEN: usize = 1796;
const CSPACE_K: u32 = 3;

/// One page of 1-bit raster, 1 = black, rows of `bytes_per_line`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    pub width: u32,
    pub height: u32,
    pub bytes_per_line: u32,
    pub resolution: [u32; 2],
    /// PageSize in points.
    pub page_size: [u32; 2],
    pub num_copies: u32,
    pub media_position: u32,
    pub duplex: bool,
    /// cupsInteger[10], brlaser's economode flag.
    pub economode: bool,
    pub media_type: String,
    pub page_size_name: String,
    pub data: Vec<u8>,
}

fn bad(msg: impl Into<String>) -> Error {
    Error::new(ErrorKind::InvalidData, msg.into())
}

pub fn is_raster(head: &[u8]) -> bool {
    matches!(head.get(..4), Some(b"RaS3" | b"3SaR" | b"RaS2" | b"2SaR"))
}

/// Parse a whole CUPS raster stream.
pub fn parse(buf: &[u8]) -> Result<Vec<Page>> {
    let (big_endian, compressed) = match buf.get(..4) {
        Some(b"RaS3") => (true, false),
        Some(b"3SaR") => (false, false),
        Some(b"RaS2") => (true, true),
        Some(b"2SaR") => (false, true),
        Some(s) => {
            return Err(bad(format!(
                "not a CUPS raster v2/v3 stream (sync {s:02x?})"
            )));
        }
        None => return Err(bad("raster stream too short")),
    };
    let mut pos = 4;
    let mut pages = Vec::new();
    while pos < buf.len() {
        let h = buf
            .get(pos..pos + HEADER_LEN)
            .ok_or_else(|| bad(format!("page {}: truncated header", pages.len() + 1)))?;
        pos += HEADER_LEN;
        let u = |off: usize| {
            let b: [u8; 4] = h[off..off + 4].try_into().unwrap();
            if big_endian {
                u32::from_be_bytes(b)
            } else {
                u32::from_le_bytes(b)
            }
        };
        let s = |off: usize| {
            let f = &h[off..off + 64];
            let end = f.iter().position(|&b| b == 0).unwrap_or(64);
            String::from_utf8_lossy(&f[..end]).into_owned()
        };
        let n = pages.len() + 1;
        let (bpc, bpp, cspace, ncolors) = (u(384), u(388), u(400), u(420));
        let (width, height, bpl) = (u(372), u(376), u(392));
        // Same checks as brlaser's main.cc, plus colour space K (1 = black).
        if bpc != 1 || bpp != 1 || ncolors != 1 || cspace != CSPACE_K {
            return Err(bad(format!(
                "page {n}: need 1-bit K raster, got bits/color {bpc}, bits/pixel {bpp}, \
                 colors {ncolors}, colorspace {cspace}"
            )));
        }
        if bpl > 10000 || bpl == 0 || (width as u64) > bpl as u64 * 8 {
            return Err(bad(format!(
                "page {n}: bogus line size {bpl} for width {width}"
            )));
        }
        let size = bpl as usize * height as usize;
        let data = if compressed {
            let (d, used) = decompress(&buf[pos..], bpl as usize, height as usize)
                .map_err(|e| bad(format!("page {n}: {e}")))?;
            pos += used;
            d
        } else {
            let d = buf
                .get(pos..pos + size)
                .ok_or_else(|| bad(format!("page {n}: truncated pixel data")))?;
            pos += size;
            d.to_vec()
        };
        pages.push(Page {
            width,
            height,
            bytes_per_line: bpl,
            resolution: [u(276), u(280)],
            page_size: [u(352), u(356)],
            num_copies: u(340),
            media_position: u(324),
            duplex: u(272) != 0,
            economode: u(452 + 4 * 10) != 0,
            media_type: s(128),
            page_size_name: s(1732),
            data,
        });
    }
    Ok(pages)
}

/// v2 compression (1 byte per pixel at 1 bpp): per line group a repeat byte,
/// then runs: n < 128 repeats the next byte n+1 times, n >= 128 copies 257-n bytes.
fn decompress(
    src: &[u8],
    bpl: usize,
    height: usize,
) -> std::result::Result<(Vec<u8>, usize), String> {
    let mut out = Vec::with_capacity(bpl * height);
    let mut p = 0;
    let mut next = || -> std::result::Result<u8, String> {
        let b = *src.get(p).ok_or("truncated compressed data")?;
        p += 1;
        Ok(b)
    };
    while out.len() < bpl * height {
        let repeat = next()? as usize + 1;
        let start = out.len();
        while out.len() - start < bpl {
            let n = next()?;
            let left = bpl - (out.len() - start);
            if n < 128 {
                let count = n as usize + 1;
                if count > left {
                    return Err("run crosses line end".into());
                }
                let v = next()?;
                out.extend(std::iter::repeat_n(v, count));
            } else {
                let count = 257 - n as usize;
                if count > left {
                    return Err("literal crosses line end".into());
                }
                for _ in 0..count {
                    out.push(next()?);
                }
            }
        }
        let rows = repeat.min(height - out.len() / bpl + 1) - 1;
        for _ in 0..rows {
            out.extend_from_within(start..start + bpl);
        }
    }
    Ok((out, p))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(le: bool, w: u32, h: u32, bpl: u32, comp: u32) -> Vec<u8> {
        let mut v = vec![0u8; HEADER_LEN];
        let mut put = |off: usize, x: u32| {
            v[off..off + 4].copy_from_slice(&if le { x.to_le_bytes() } else { x.to_be_bytes() })
        };
        for (o, x) in [
            (276, 600),
            (280, 600),
            (340, 1),
            (372, w),
            (376, h),
            (384, 1),
            (388, 1),
            (392, bpl),
            (400, 3),
            (404, comp),
            (420, 1),
        ] {
            put(o, x);
        }
        v[128..133].copy_from_slice(b"PLAIN");
        v[1732..1734].copy_from_slice(b"A4");
        v
    }

    #[test]
    fn reads_v3_both_endians() {
        for (sync, le) in [(b"3SaR", true), (b"RaS3", false)] {
            let mut s = sync.to_vec();
            s.extend(header(le, 16, 2, 2, 0));
            s.extend([1, 2, 3, 4]);
            let p = parse(&s).unwrap();
            assert_eq!(p.len(), 1);
            assert_eq!(p[0].data, [1, 2, 3, 4]);
            assert_eq!(p[0].page_size_name, "A4");
            assert_eq!(p[0].media_type, "PLAIN");
        }
    }

    #[test]
    fn reads_v2_runs_literals_and_line_repeats() {
        let mut s = b"RaS2".to_vec();
        s.extend(header(false, 32, 3, 4, 1));
        // line 0 twice: 3x 0xAA then 1x 0x55; line 2: literal 4 bytes
        s.extend([1, 2, 0xAA, 0, 0x55, 0, 253, 9, 8, 7, 6]);
        let p = parse(&s).unwrap();
        assert_eq!(
            p[0].data,
            [0xAA, 0xAA, 0xAA, 0x55, 0xAA, 0xAA, 0xAA, 0x55, 9, 8, 7, 6]
        );
    }

    #[test]
    fn rejects_8bit_and_garbage() {
        let mut s = b"3SaR".to_vec();
        let mut h = header(true, 8, 1, 8, 0);
        h[384..388].copy_from_slice(&8u32.to_le_bytes());
        s.extend(h);
        s.extend([0; 8]);
        assert!(parse(&s).unwrap_err().to_string().contains("1-bit"));
        assert!(parse(b"%PDF-1.4").is_err());
        assert!(
            parse(b"3SaR\0\0")
                .unwrap_err()
                .to_string()
                .contains("truncated")
        );
    }
}
