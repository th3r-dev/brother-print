# brother-print

Print to a Brother DCP-1600 series laser printer from macOS, in Rust, with no Brother driver installed.

Some Brother lasers, such as the DCP-1610W, only accept Brother's own host-based print format (`application/vnd.brother-hbp`), not PDF, PostScript or AirPrint. Brother's driver normally converts each page. This crate converts pages itself:

1. **Render:** macOS's built-in `cupsfilter` (Apple's `cgpdftoraster`) turns a PDF, image or text file into 600 dpi black-and-white CUPS raster pages.
2. **Encode:** a Rust port of the encoder from [brlaser](https://github.com/pdewacht/brlaser), the open-source CUPS driver for Brother lasers, turns those pages into the printer's format.
3. **Send:** the job goes to the printer's raw port, TCP 9100.

> **Status: prototype.** It printed a one-page A4 PDF on a DCP-1610W. Placement on paper and other models are untested.

## Usage

```sh
cargo install --git https://github.com/th3r-dev/brother-print

brother-print 192.0.2.10 document.pdf                         # A4, 1 copy
brother-print 192.0.2.10 photo.jpg --copies 2 --paper Letter
brother-print 192.0.2.10 report.pdf --pages 1-3,5 --toner-save
brother-print 192.0.2.10 notes.txt --dry-run job.prn          # write the job, don't send it
```

```rust
use brother_print::{print_file, PrintOptions, Paper};

let opts = PrintOptions { copies: 1, page_ranges: None, paper: Paper::A4, toner_save: false };
print_file("192.0.2.10", std::path::Path::new("document.pdf"), &opts)?;
```

`render`, `encode` and `send` are also public, so an app can count pages or check the printer's status before sending.

- **Inputs:** PDF, JPEG, PNG, TIFF, HEIC (converted with `sips` first) and plain text.
- **Paper:** A4, A5, A6, B5, B6, Letter, Legal, Executive, and the envelopes C5, DL and Monarch.
- **Copies and page ranges:** handled by `cupsfilter`, with collated copies.

It needs macOS, for `cupsfilter` and `sips`.

## How it was checked

The encoder's output was compared **byte for byte** with brlaser's own filter, which was compiled locally as a reference:

- **20 synthetic rasters, all identical.** They include blank, full black, text-like and random-noise pages; A4 and Letter; multi-page jobs whose settings change between pages (copies, toner save, duplex, trays, an unknown paper size); and the 254-edit-per-line limit at 1200 dpi.
- **8 real conversions, all identical:** PDF on A4 and on Letter, a page range, plain text, PNG, JPEG, TIFF and HEIC.

`tests/oracle.rs` repeats the comparison when `BRLASER_ORACLE` points at a built `rastertobrlaser`.

```sh
cargo test
cargo clippy --all-targets -- -D warnings
```

## Limits

- **One model family.** Tested on a DCP-1610W. brlaser also lists the DCP-1510, HL-1110 and HL-1200 series, which may work.
- **Black and white only.** These are monochrome lasers.
- **No duplex.** The DCP-1600 series has no duplex unit, so it isn't offered.
- **Placement isn't confirmed.** Pages are rendered full-bleed, and the printer's own margins may clip content within about 4 mm of the edge.
- **Memory.** A job is held in memory: about 4.3 MB of raster per A4 page.

## References

| Source | Used for | License |
|---|---|---|
| [brlaser](https://github.com/pdewacht/brlaser) by Peter De Wachter | Encoder ported to Rust, and the reference for the byte-for-byte check | GPL-2.0-or-later |
| [brother-netscan](https://github.com/th3r-dev/brother-netscan) | Sibling crate that scans from the same printers over the network | MIT or Apache-2.0 |

## License

GPL-2.0-or-later, because it ports brlaser. See [LICENSE](LICENSE) and [NOTICE](NOTICE), which lists what was ported from which brlaser file.

Brother is a trademark of Brother Industries, Ltd. This project is not affiliated with or endorsed by Brother.
