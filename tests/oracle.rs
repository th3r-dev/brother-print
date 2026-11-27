//! Byte-for-byte comparison against brlaser's own `rastertobrlaser`.
//! Set BRLASER_ORACLE to the compiled filter; skipped otherwise.

use brother_print::{encode, encode::ascii_job_name, raster};
use std::process::Command;

struct Spec {
    size_name: &'static str,
    points: [u32; 2],
    res: u32,
    copies: u32,
    eco: bool,
    duplex: bool,
    media_pos: u32,
    pattern: fn(&mut Rng, usize, &mut [u8]),
}

impl Spec {
    fn new(size_name: &'static str, points: [u32; 2]) -> Self {
        Spec {
            size_name,
            points,
            res: 600,
            copies: 1,
            eco: false,
            duplex: false,
            media_pos: 0,
            pattern: |_, _, _| {},
        }
    }
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u8 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 24) as u8
    }
}

fn blank(_: &mut Rng, _: usize, _: &mut [u8]) {}
fn black(_: &mut Rng, _: usize, l: &mut [u8]) {
    l.fill(0xFF)
}
fn noise(r: &mut Rng, _: usize, l: &mut [u8]) {
    l.iter_mut().for_each(|b| *b = r.next())
}
/// Lines of "glyphs": short runs, gaps, a few rules, blank leading between rows.
fn text(r: &mut Rng, y: usize, l: &mut [u8]) {
    if y % 120 > 70 || y < 300 {
        return;
    }
    if y % 600 == 310 {
        let n = l.len();
        l[50..n - 50].fill(0xFF);
        return;
    }
    let (mut x, n) = (40, l.len());
    while x + 8 < n - 40 {
        let w = 1 + (r.next() % 5) as usize;
        for b in &mut l[x..x + w] {
            *b = r.next() & r.next();
        }
        x += w + (r.next() % 4) as usize;
    }
}
/// Every third byte set on even rows, odd rows blank: each even row is ~bpl/3
/// edits against the blank row above, which hits brlaser's 254-edit cap.
fn sparse(_: &mut Rng, y: usize, l: &mut [u8]) {
    if y.is_multiple_of(2) {
        l.iter_mut().step_by(3).for_each(|b| *b = 0x81);
    }
}

fn header(s: &Spec, w: u32, h: u32, bpl: u32, comp: u32) -> Vec<u8> {
    let mut v = vec![0u8; 1796];
    let mut put = |off: usize, x: u32| v[off..off + 4].copy_from_slice(&x.to_le_bytes());
    for (o, x) in [
        (272, s.duplex as u32),
        (276, s.res),
        (280, s.res),
        (324, s.media_pos),
        (340, s.copies),
        (352, s.points[0]),
        (356, s.points[1]),
        (372, w),
        (376, h),
        (384, 1),
        (388, 1),
        (392, bpl),
        (400, 3),
        (404, comp),
        (420, 1),
        (452 + 40, s.eco as u32),
    ] {
        put(o, x);
    }
    v[128..133].copy_from_slice(b"PLAIN");
    v[1732..1732 + s.size_name.len()].copy_from_slice(s.size_name.as_bytes());
    v
}

/// v2 compression: literal chunks, runs for repeated bytes, one line at a time.
fn compress_line(l: &[u8], out: &mut Vec<u8>) {
    out.push(0);
    let mut i = 0;
    while i < l.len() {
        let run = l[i..].iter().take_while(|&&b| b == l[i]).count().min(128);
        if run >= 2 || i + 1 == l.len() {
            out.extend([(run - 1) as u8, l[i]]);
            i += run;
        } else {
            let n = (l.len() - i).clamp(2, 128);
            out.push((257 - n) as u8);
            out.extend_from_slice(&l[i..i + n]);
            i += n;
        }
    }
}

fn make_raster(pages: &[Spec], compressed: bool) -> Vec<u8> {
    let mut out = if compressed { b"2SaR" } else { b"3SaR" }.to_vec();
    let mut rng = Rng(0x2545F4914F6CDD1D);
    for s in pages {
        let w = s.points[0] * s.res / 72;
        let h = s.points[1] * s.res / 72;
        let bpl = w.div_ceil(8);
        out.extend(header(s, w, h, bpl, compressed as u32));
        let mut line = vec![0u8; bpl as usize];
        for y in 0..h as usize {
            line.fill(0);
            (s.pattern)(&mut rng, y, &mut line);
            if compressed {
                compress_line(&line, &mut out);
            } else {
                out.extend_from_slice(&line);
            }
        }
    }
    out
}

const A4: [u32; 2] = [595, 842];
const LETTER: [u32; 2] = [612, 792];

fn cases() -> Vec<(&'static str, Vec<Spec>)> {
    let p = |size, pts, pattern: fn(&mut Rng, usize, &mut [u8])| Spec {
        pattern,
        ..Spec::new(size, pts)
    };
    vec![
        ("blank-a4", vec![p("A4", A4, blank)]),
        ("black-a4", vec![p("A4", A4, black)]),
        ("text-a4", vec![p("A4", A4, text)]),
        ("noise-a4", vec![p("A4", A4, noise)]),
        ("text-letter", vec![p("Letter", LETTER, text)]),
        ("noise-letter", vec![p("Letter", LETTER, noise)]),
        (
            "multipage-same-params",
            vec![p("A4", A4, text), p("A4", A4, blank), p("A4", A4, black)],
        ),
        (
            "multipage-param-changes",
            vec![
                p("Letter", LETTER, text),
                Spec {
                    copies: 3,
                    eco: true,
                    ..p("A4", A4, text)
                },
                Spec {
                    duplex: true,
                    media_pos: 4,
                    ..p("Legal", [612, 1008], noise)
                },
                Spec {
                    media_pos: 9,
                    ..p("EnvDL", [312, 624], text)
                },
                // Unknown size name: brlaser falls back to PAPER = A4.
                p("Tabloid", [792, 1224], text),
            ],
        ),
        (
            "1200dpi-edit-cap",
            vec![Spec {
                res: 1200,
                ..p("A4", A4, sparse)
            }],
        ),
        ("sparse-600", vec![p("Letter", LETTER, sparse)]),
    ]
}

#[test]
fn matches_brlaser_byte_for_byte() {
    let Ok(oracle) = std::env::var("BRLASER_ORACLE") else {
        eprintln!("BRLASER_ORACLE not set, skipping oracle comparison");
        return;
    };
    let dir = std::env::temp_dir().join(format!("brother-print-oracle-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut failed = Vec::new();
    for (name, pages) in cases() {
        for compressed in [false, true] {
            let ras = make_raster(&pages, compressed);
            let path = dir.join(format!("{name}-{compressed}.ras"));
            std::fs::write(&path, &ras).unwrap();
            let out = Command::new(&oracle)
                .args(["1", "user", "title", "1", ""])
                .arg(&path)
                .output()
                .unwrap();
            assert!(out.status.success(), "oracle failed on {name}");
            let ours = encode(
                &raster::parse(&ras).unwrap(),
                &ascii_job_name(&["1", "user", "title"]),
            );
            let tag = format!("{name} (v{})", if compressed { 2 } else { 3 });
            if ours == out.stdout {
                eprintln!("MATCH {tag}: {} bytes", ours.len());
            } else {
                let at = ours.iter().zip(&out.stdout).position(|(a, b)| a != b);
                eprintln!(
                    "DIFF  {tag}: ours {} vs brlaser {} bytes, first diff at {at:?}",
                    ours.len(),
                    out.stdout.len()
                );
                failed.push(tag);
            }
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(failed.is_empty(), "mismatches: {failed:?}");
}
