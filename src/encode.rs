// Port of brlaser's encoder (src/line.cc, src/block.h, src/job.cc and
// build_page_params/ascii_job_name from src/main.cc).
// brlaser: Copyright 2013 Peter De Wachter. GPL-2.0-or-later.

use crate::raster::Page;

// ---- line.cc ---------------------------------------------------------------

fn write_overflow(value: i64, out: &mut Vec<u8>) {
    if value >= 0 {
        if value < 255 {
            out.push(value as u8);
        } else {
            out.extend(std::iter::repeat_n(255u8, (value / 255) as usize));
            out.push((value % 255) as u8);
        }
    }
}

fn write_substitute(offset: usize, data: &[u8], out: &mut Vec<u8>) {
    debug_assert!(offset < 10000 && !data.is_empty());
    const OFFSET_MAX: usize = 15;
    const COUNT_MAX: usize = 7;
    let count = data.len() - 1;
    out.push(((offset.min(OFFSET_MAX) << 3) | count.min(COUNT_MAX)) as u8);
    write_overflow(offset as i64 - OFFSET_MAX as i64, out);
    write_overflow(count as i64 - COUNT_MAX as i64, out);
    out.extend_from_slice(data);
}

fn write_repeat(offset: usize, count: usize, value: u8, out: &mut Vec<u8>) {
    debug_assert!(offset < 10000 && (2..10000).contains(&count));
    const OFFSET_MAX: usize = 3;
    const COUNT_MAX: usize = 31;
    let count = count - 2;
    out.push((128 | (offset.min(OFFSET_MAX) << 5) | count.min(COUNT_MAX)) as u8);
    write_overflow(offset as i64 - OFFSET_MAX as i64, out);
    write_overflow(count as i64 - COUNT_MAX as i64, out);
    out.push(value);
}

fn repeat_length(line: &[u8], first: usize, last: usize) -> usize {
    line[first..last]
        .iter()
        .position(|&b| b != line[first])
        .unwrap_or(last - first)
}

/// Bytes to emit as a literal run before a repeat or an unchanged pair starts.
fn substitute_length(line: &[u8], reference: &[u8], first: usize, last: usize) -> usize {
    let (mut prev, mut it) = (first, first);
    while it + 1 < last {
        if line[it] == reference[it] && line[it + 1] == reference[it + 1] {
            return it - first;
        }
        if line[it] == line[it + 1] && line[it] == line[prev] {
            return prev - first;
        }
        prev = it;
        it += 1;
    }
    last - first
}

/// Encode `line` as edits against the previous line `reference`.
pub fn encode_line_delta(line: &[u8], reference: &[u8]) -> Vec<u8> {
    assert_eq!(line.len(), reference.len());
    if line.iter().all(|&b| b == 0) {
        return vec![0xFF];
    }
    let mut out = Vec::with_capacity(line.len() + 16);
    out.push(0); // edit count, patched below
    const MAX_EDITS: u8 = 254;
    let mut num_edits = 0u8;

    let same_tail = line
        .iter()
        .rev()
        .zip(reference.iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let end = line.len() - same_tail;
    let mut pos = 0;
    loop {
        let offset = line[pos..end]
            .iter()
            .zip(&reference[pos..end])
            .take_while(|(a, b)| a == b)
            .count();
        pos += offset;
        if pos == end {
            break;
        }
        num_edits += 1;
        if num_edits == MAX_EDITS {
            // Out of edits: dump the rest of the line as one substitute.
            write_substitute(offset, &line[pos..end], &mut out);
            break;
        }
        let s = substitute_length(line, reference, pos, end);
        if s > 0 {
            write_substitute(offset, &line[pos..pos + s], &mut out);
            pos += s;
        } else {
            let r = repeat_length(line, pos, end);
            debug_assert!(r >= 2);
            write_repeat(offset, r, line[pos], &mut out);
            pos += r;
        }
    }
    out[0] = num_edits;
    out
}

/// Encode `line` on its own (start of a band or block).
pub fn encode_line(line: &[u8]) -> Vec<u8> {
    if line.iter().all(|&b| b == 0) {
        return vec![0xFF];
    }
    let mut out = Vec::with_capacity(line.len() + 16);
    out.push(1);
    write_substitute(0, line, &mut out);
    out
}

// ---- block.h ---------------------------------------------------------------

const MAX_BLOCK_SIZE: usize = 16350;

#[derive(Default)]
struct Block {
    lines: Vec<Vec<u8>>,
    line_bytes: usize,
}

impl Block {
    fn line_fits(&self, size: usize) -> bool {
        self.line_bytes + size < MAX_BLOCK_SIZE
    }

    fn add_line(&mut self, line: Vec<u8>) {
        debug_assert!(!line.is_empty() && self.line_fits(line.len()));
        self.line_bytes += line.len();
        self.lines.push(line);
    }

    fn flush(&mut self, out: &mut Vec<u8>) {
        if self.line_bytes == 0 {
            return;
        }
        out.extend_from_slice(format!("{}w", self.line_bytes + 2).as_bytes());
        out.push(0);
        out.push(self.lines.len() as u8);
        for l in self.lines.drain(..) {
            out.extend_from_slice(&l);
        }
        self.line_bytes = 0;
    }
}

// ---- main.cc: build_page_params / ascii_job_name ------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageParams {
    pub num_copies: u32,
    pub resolution: u32,
    pub duplex: bool,
    pub economode: bool,
    pub sourcetray: &'static str,
    pub mediatype: String,
    pub papersize: &'static str,
}

impl PageParams {
    pub fn from_page(p: &Page) -> Self {
        const SOURCES: [&str; 6] = ["AUTO", "T1", "T2", "T3", "MP", "MANUAL"];
        let papersize = match p.page_size_name.as_str() {
            "A4" => "A4",
            "A5" => "A5",
            "A6" => "A6",
            "B5" => "B5",
            "B6" => "B6",
            "EnvC5" => "C5",
            "EnvMonarch" => "MONARCH",
            "EnvPRC5" | "EnvDL" => "DL",
            "Executive" => "EXECUTIVE",
            "Legal" => "LEGAL",
            "Letter" => "LETTER",
            _ => "A4",
        };
        PageParams {
            num_copies: p.num_copies,
            resolution: p.resolution[0],
            duplex: p.duplex,
            economode: p.economode,
            sourcetray: SOURCES
                .get(p.media_position as usize)
                .copied()
                .unwrap_or(SOURCES[0]),
            mediatype: p.media_type.clone(),
            papersize,
        }
    }
}

/// brlaser's `ascii_job_name`: join the plain-ASCII parts with '/', max 79 chars.
pub fn ascii_job_name(parts: &[&str]) -> String {
    let mut r = parts
        .iter()
        .filter(|p| !p.is_empty() && p.bytes().all(|b| (32..=126).contains(&b)))
        .copied()
        .collect::<Vec<_>>()
        .join("/");
    if r.is_empty() {
        r = "brlaser".into();
    }
    r.truncate(79);
    r
}

// ---- job.cc ----------------------------------------------------------------

/// Encode raster pages into one Brother HBP job (PJL + PCL-ish raster).
pub fn encode(pages: &[Page], job_name: &str) -> Vec<u8> {
    // job::job: replace dubious characters.
    let name: String = job_name
        .chars()
        .map(|c| {
            if (c as u32) < 32 || (c as u32) >= 127 || c == '"' || c == '\\' {
                ' '
            } else {
                c
            }
        })
        .collect();
    let mut out = Vec::new();
    if pages.is_empty() {
        return out;
    }
    // begin_job
    out.extend_from_slice(&[0u8; 128]);
    out.extend_from_slice(format!("\x1b%-12345X@PJL\n@PJL JOB NAME=\"{name}\"\n").as_bytes());

    let mut current: Option<PageParams> = None;
    for page in pages {
        let params = PageParams::from_page(page);
        if current.as_ref() != Some(&params) {
            write_page_header(&params, &mut out);
            current = Some(params);
        }
        encode_page(page, &mut out);
    }
    // end_job
    out.extend_from_slice(
        format!("\x1b%-12345X@PJL\n@PJL EOJ NAME=\"{name}\"\n\x1b%-12345X\n").as_bytes(),
    );
    out
}

fn write_page_header(p: &PageParams, out: &mut Vec<u8>) {
    let mut s = String::from("\x1b%-12345X@PJL\n");
    if p.resolution != 1200 {
        s += "@PJL SET RAS1200MODE = FALSE\n";
        s += &format!("@PJL SET RESOLUTION = {}\n", p.resolution);
    } else {
        s += "@PJL SET RAS1200MODE = TRUE\n@PJL SET RESOLUTION = 600\n";
    }
    s += &format!(
        "@PJL SET ECONOMODE = {}\n",
        if p.economode { "ON" } else { "OFF" }
    );
    s += &format!("@PJL SET SOURCETRAY = {}\n", p.sourcetray);
    s += &format!("@PJL SET MEDIATYPE = {}\n", p.mediatype);
    s += &format!("@PJL SET PAPER = {}\n", p.papersize);
    s += "@PJL SET PAGEPROTECT = AUTO\n@PJL SET ORIENTATION = PORTRAIT\n";
    s += "@PJL ENTER LANGUAGE = PCL\n";
    s += "\x1bE";
    s += &format!("\x1b&l{}X", p.num_copies.max(1));
    if p.duplex {
        s += "\x1b&l2S";
    }
    out.extend_from_slice(s.as_bytes());
}

fn encode_page(page: &Page, out: &mut Vec<u8>) {
    // XXX (brlaser) brother driver uses 128 lines per band
    const LINES_PER_BAND: usize = 64;
    let mut lines = page.data.chunks_exact(page.bytes_per_line as usize);
    let Some(first) = lines.next() else { return };
    let mut block = Block::default();
    block.add_line(encode_line(first));
    out.extend_from_slice(b"\x1b*b1030m");
    let mut reference = first;
    for (i, line) in lines.enumerate().map(|(i, l)| (i + 1, l)) {
        let encoded = if i % LINES_PER_BAND == 0 {
            block.flush(out);
            encode_line(line)
        } else {
            let e = encode_line_delta(line, reference);
            if block.line_fits(e.len()) {
                e
            } else {
                block.flush(out);
                encode_line(line)
            }
        };
        block.add_line(encoded);
        reference = line;
    }
    block.flush(out);
    out.extend_from_slice(b"1030M\x0c");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_line_is_ff() {
        assert_eq!(encode_line(&[0; 8]), [0xFF]);
        assert_eq!(encode_line_delta(&[0; 8], &[1; 8]), [0xFF]);
    }

    #[test]
    fn full_line_is_one_substitute_with_overflow() {
        // 20 bytes: count 19 -> low 7, overflow 12; offset 0 -> no overflow.
        let l = [0xAA; 20];
        let e = encode_line(&l);
        assert_eq!(&e[..3], &[1, 7, 12]);
        assert_eq!(&e[3..], &l);
    }

    #[test]
    fn delta_repeat_and_substitute() {
        let reference = [0u8; 40];
        let mut line = [0u8; 40];
        line[20..30].fill(0xFF); // 10 x 0xFF at offset 20
        // repeat: offset 20 -> low 3 + overflow 17, count 10-2=8
        assert_eq!(
            encode_line_delta(&line, &reference),
            [1, 0x80 | (3 << 5) | 8, 17, 0xFF]
        );
        line[35] = 7; // then a literal one byte after a gap of 5
        assert_eq!(
            encode_line_delta(&line, &reference),
            [2, 0x80 | (3 << 5) | 8, 17, 0xFF, 5 << 3, 7]
        );
        assert_eq!(encode_line_delta(&line, &line), [0]);
    }

    #[test]
    fn edit_cap_dumps_rest_as_one_substitute() {
        let reference = [0u8; 1240];
        let mut line = [0u8; 1240];
        line.iter_mut().step_by(3).for_each(|b| *b = 0x81);
        let e = encode_line_delta(&line, &reference);
        assert_eq!(e[0], 254);
        // 253 one-byte substitutes (offset 2), then the 254th covers the rest.
        assert_eq!(&e[1..3], &[0, 0x81]);
        assert_eq!(&e[3..5], &[2 << 3, 0x81]);
        // Last edit: cmd + 2 count-overflow bytes (480-7 = 255+218) + 481 data bytes.
        let rest = 1240 - 3 * 253;
        assert_eq!(
            &e[e.len() - rest - 3..e.len() - rest],
            &[(2 << 3) | 7, 255, 218]
        );
        assert_eq!(e.len(), 1 + 2 + 252 * 2 + 3 + rest);
    }

    #[test]
    fn overflow_of_255_multiples() {
        let mut v = Vec::new();
        write_overflow(255, &mut v);
        assert_eq!(v, [255, 0]);
        v.clear();
        write_overflow(600, &mut v);
        assert_eq!(v, [255, 255, 90]);
    }

    #[test]
    fn job_name_rules() {
        assert_eq!(ascii_job_name(&["1", "", "café", "doc.pdf"]), "1/doc.pdf");
        assert_eq!(ascii_job_name(&[]), "brlaser");
        assert_eq!(ascii_job_name(&[&"x".repeat(100)]).len(), 79);
    }
}
