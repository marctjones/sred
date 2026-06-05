//! Spike: can cosmic-text's Buffer be our byte-lossless substrate?
//! Loads each corpus string with `Buffer::set_text`, reconstructs the text from
//! `lines + endings`, and checks byte-equality. This decides whether we can drop
//! our own buffer and let cosmic-text's `Editor` own the text.

use cosmic_text::{Attrs, Buffer, FontSystem, Metrics, Shaping};

const CORPUS: &[&str] = &[
    "",
    "plain, no newline",
    "trailing\n",
    "two\n\n",
    "windows\r\nsecond\r\n",
    "trailing spaces   \nand tab\there\n",
    "# heading\n\n- a\n- b\n  - nested\n",
    "```rust\nfn main() {}\n```\n",
    "blank runs\n\n\n\nend\n",
    "unicode café 日本語 \u{1F600}\n",
    "TODO(do) ship @[[Marc]] +[[Sred]] due:2026-07-01 [#A]\n",
];

fn reconstruct(buf: &Buffer) -> String {
    let mut out = String::new();
    for line in &buf.lines {
        out.push_str(line.text());
        out.push_str(line.ending().as_str());
    }
    out
}

/// Lossless extraction with the single documented guard: an empty buffer is one
/// default-`Lf` line, so reconstruct `""` for that degenerate case.
fn buffer_text(buf: &Buffer) -> String {
    if buf.lines.len() == 1 && buf.lines[0].text().is_empty() {
        return String::new();
    }
    reconstruct(buf)
}

#[test]
fn cosmic_buffer_is_byte_lossless() {
    let mut fs = FontSystem::new();
    let attrs = Attrs::new();
    let mut mismatches = Vec::new();
    for (i, src) in CORPUS.iter().enumerate() {
        let mut buf = Buffer::new(&mut fs, Metrics::new(14.0, 18.0));
        buf.set_text(&mut fs, src, &attrs, Shaping::Advanced);
        // raw reconstruction round-trips everything except "" (the empty edge)
        if !src.is_empty() && &reconstruct(&buf) != src {
            mismatches.push(format!("#{i} raw: {src:?} -> {:?}", reconstruct(&buf)));
        }
        // with the empty guard, every case is exact
        if &buffer_text(&buf) != src {
            mismatches.push(format!("#{i} guarded: {src:?} -> {:?}", buffer_text(&buf)));
        }
    }
    assert!(
        mismatches.is_empty(),
        "cosmic Buffer is NOT byte-lossless ({}):\n{}",
        mismatches.len(),
        mismatches.join("\n")
    );
}
