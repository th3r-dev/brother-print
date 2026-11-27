//! Print to a Brother DCP-1600 series laser over raw TCP 9100 without a
//! Brother driver: macOS `cupsfilter` makes CUPS raster, a Rust port of
//! brlaser encodes it, and the job goes straight to the printer socket.

pub mod encode;
pub mod raster;

use std::io::{self, Error, ErrorKind, Read, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;
use std::time::Duration;

pub use encode::encode;
pub use raster::Page;

const PPD: &str = include_str!("dcp1600.ppd");

/// Paper sizes in brlaser's DCP-1600 PPD; names are the PPD PageSize keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Paper {
    #[default]
    A4,
    A5,
    A6,
    B5,
    B6,
    EnvC5,
    EnvMonarch,
    EnvDL,
    Executive,
    Legal,
    Letter,
}

impl Paper {
    const ALL: [Paper; 11] = [
        Paper::A4,
        Paper::A5,
        Paper::A6,
        Paper::B5,
        Paper::B6,
        Paper::EnvC5,
        Paper::EnvMonarch,
        Paper::EnvDL,
        Paper::Executive,
        Paper::Legal,
        Paper::Letter,
    ];

    pub fn ppd_name(self) -> &'static str {
        match self {
            Paper::A4 => "A4",
            Paper::A5 => "A5",
            Paper::A6 => "A6",
            Paper::B5 => "B5",
            Paper::B6 => "B6",
            Paper::EnvC5 => "EnvC5",
            Paper::EnvMonarch => "EnvMonarch",
            Paper::EnvDL => "EnvDL",
            Paper::Executive => "Executive",
            Paper::Legal => "Legal",
            Paper::Letter => "Letter",
        }
    }
}

impl FromStr for Paper {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        Paper::ALL
            .into_iter()
            .find(|p| p.ppd_name().eq_ignore_ascii_case(s))
            .ok_or_else(|| {
                let names: Vec<_> = Paper::ALL.iter().map(|p| p.ppd_name()).collect();
                format!("unknown paper '{s}' (one of {})", names.join(", "))
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrintOptions {
    pub copies: u16,
    /// CUPS page-ranges syntax, e.g. "1-3,5".
    pub page_ranges: Option<String>,
    pub paper: Paper,
    pub toner_save: bool,
}

impl Default for PrintOptions {
    fn default() -> Self {
        PrintOptions {
            copies: 1,
            page_ranges: None,
            paper: Paper::A4,
            toner_save: false,
        }
    }
}

fn invalid(msg: impl Into<String>) -> Error {
    Error::new(ErrorKind::InvalidInput, msg.into())
}

/// Deletes the path on drop.
struct TempFile(PathBuf);
impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn temp_path(suffix: &str) -> TempFile {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    TempFile(
        std::env::temp_dir().join(format!("brother-print-{}-{n}-{suffix}", std::process::id())),
    )
}

/// Rasterize a file into full-page 600 dpi 1-bit pages with macOS's
/// `cupsfilter` and the bundled PPD. Works for PDF, PNG, JPEG, TIFF, plain
/// text (and HEIC/HEIF via `sips`). cupsfilter does page ranges
/// (`-o page-ranges`) and copies (`-n N -o Collate=True`, so pages are
/// repeated collated and each page header says NumCopies 1).
/// A file that already is CUPS raster is parsed as is and `opts` is ignored.
///
/// ponytail: whole job held in memory (~4.3 MB per A4 page); stream per page
/// if long documents matter.
pub fn render(input: &Path, opts: &PrintOptions) -> io::Result<Vec<Page>> {
    let mut head = [0u8; 4];
    let n = std::fs::File::open(input)
        .and_then(|mut f| f.read(&mut head))
        .map_err(|e| Error::new(e.kind(), format!("{}: {e}", input.display())))?;
    if raster::is_raster(&head[..n]) {
        return raster::parse(&std::fs::read(input)?);
    }
    if opts.copies == 0 {
        return Err(invalid("copies must be at least 1"));
    }
    if let Some(r) = &opts.page_ranges
        && (r.is_empty()
            || !r
                .bytes()
                .all(|b| b.is_ascii_digit() || b == b',' || b == b'-'))
    {
        return Err(invalid(format!("bad page range '{r}' (use e.g. 1-3,5)")));
    }

    // cupsfilter has no HEIC/HEIF type; sips (also built in) converts to PNG.
    let ext = input
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let png = temp_path("input.png");
    let input = if ext == "heic" || ext == "heif" {
        let out = Command::new("/usr/bin/sips")
            .args(["-s", "format", "png"])
            .arg(input)
            .arg("--out")
            .arg(&png.0)
            .output()?;
        if !out.status.success() {
            return Err(Error::other(format!(
                "sips could not convert {}: {}",
                input.display(),
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        png.0.as_path()
    } else {
        input
    };

    let ppd = temp_path("dcp1600.ppd");
    std::fs::write(&ppd.0, PPD)?;
    let mut cmd = Command::new("/usr/sbin/cupsfilter");
    cmd.arg("-p")
        .arg(&ppd.0)
        .args(["-m", "application/vnd.cups-raster"])
        .args(["-o", &format!("PageSize={}", opts.paper.ppd_name())])
        .args([
            "-o",
            if opts.toner_save {
                "brlaserEconomode=True"
            } else {
                "brlaserEconomode=False"
            },
        ])
        // Collated copies: the filter repeats the pages, NumCopies stays 1.
        .args(["-n", &opts.copies.to_string(), "-o", "Collate=True"]);
    if let Some(r) = &opts.page_ranges {
        cmd.args(["-o", &format!("page-ranges={r}")]);
    }
    let out = cmd.arg(input).output()?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let msg: Vec<&str> = err
            .lines()
            .filter(|l| !l.starts_with("DEBUG") && !l.starts_with("INFO"))
            .collect();
        return Err(Error::other(format!(
            "cupsfilter failed on {}: {}",
            input.display(),
            msg.join("; ")
        )));
    }
    let pages = raster::parse(&out.stdout)?;
    if pages.is_empty() {
        return Err(invalid("nothing to print (empty document or page range)"));
    }
    Ok(pages)
}

/// Write a job to `host` (default port 9100) and close the connection.
pub fn send(host: &str, job: &[u8]) -> io::Result<()> {
    let target = if host.contains(':') && !host.starts_with('[') && host.matches(':').count() > 1 {
        format!("[{host}]:9100") // bare IPv6
    } else if host
        .rsplit_once(':')
        .is_some_and(|(_, p)| p.parse::<u16>().is_ok())
    {
        host.to_string()
    } else {
        format!("{host}:9100")
    };
    let addrs: Vec<_> = target
        .to_socket_addrs()
        .map_err(|e| Error::new(e.kind(), format!("cannot resolve {target}: {e}")))?
        .collect();
    let mut last = Error::new(ErrorKind::NotFound, format!("no address for {target}"));
    let mut stream = None;
    for a in addrs {
        match TcpStream::connect_timeout(&a, Duration::from_secs(10)) {
            Ok(s) => {
                stream = Some(s);
                break;
            }
            Err(e) => last = Error::new(e.kind(), format!("connect {a}: {e}")),
        }
    }
    let mut s = stream.ok_or(last)?;
    // The printer stops reading while it is busy or out of paper.
    s.set_write_timeout(Some(Duration::from_secs(120)))?;
    s.write_all(job).map_err(|e| {
        Error::new(
            e.kind(),
            format!("printer stopped accepting data ({e}); check paper and printer status"),
        )
    })?;
    s.flush()?;
    s.shutdown(Shutdown::Write)?;
    // Drain whatever the printer says so closing does not reset the connection.
    s.set_read_timeout(Some(Duration::from_secs(5)))?;
    let _ = io::copy(&mut s, &mut io::sink());
    Ok(())
}

/// Render, encode and send one file.
pub fn print_file(host: &str, input: &Path, opts: &PrintOptions) -> io::Result<()> {
    let pages = render(input, opts)?;
    send(host, &encode(&pages, &job_name(input)))
}

/// Job name shown on the printer: brlaser's rules applied to the file name.
pub fn job_name(input: &Path) -> String {
    let name = input.file_name().and_then(|n| n.to_str()).unwrap_or("");
    encode::ascii_job_name(&["brother-print", name])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn send_writes_job_and_reports_refused() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let t = std::thread::spawn(move || {
            let mut got = Vec::new();
            l.accept().unwrap().0.read_to_end(&mut got).unwrap();
            got
        });
        send(&addr.to_string(), b"job bytes").unwrap();
        assert_eq!(t.join().unwrap(), b"job bytes");
        // Same port, listener gone: connection refused, with the address in the message.
        let e = send(&addr.to_string(), b"x").unwrap_err();
        assert!(e.to_string().contains(&addr.to_string()), "{e}");
    }

    #[test]
    fn paper_parse() {
        assert_eq!("letter".parse::<Paper>(), Ok(Paper::Letter));
        assert!("A3".parse::<Paper>().is_err());
    }

    #[test]
    fn bad_range_rejected_before_filtering() {
        let opts = PrintOptions {
            page_ranges: Some("1;rm".into()),
            ..Default::default()
        };
        let e = render(Path::new("Cargo.toml"), &opts).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::InvalidInput);
    }
}
