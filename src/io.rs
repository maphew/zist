use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use crate::format::Format;
use crate::naming::{compressed_path, decompressed_path};

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    OutputExists(PathBuf),
    UnknownFormat,
    UnsupportedFormat(Format),
    MaxSizeExceeded(u64),
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "io: {e}"),
            Error::OutputExists(p) => write!(f, "refusing to clobber: {}", p.display()),
            Error::UnknownFormat => write!(f, "could not identify compression format"),
            Error::UnsupportedFormat(fmt) => write!(f, "no decompressor compiled in for {fmt}"),
            Error::MaxSizeExceeded(n) => {
                write!(f, "decompressed output exceeded --max-size ({n} bytes)")
            }
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Default, Clone, Copy)]
pub struct Options {
    /// Don't remove the source file after a successful operation.
    pub keep: bool,
    /// Overwrite an existing destination instead of erroring.
    pub force: bool,
    /// Cap decompressed output; decompress paths abort past this many bytes.
    /// Ignored on compress.
    pub max_decompressed: Option<u64>,
}

/// Compress `src` in place. Writes `src.<suffix>`; removes `src` unless `opts.keep`.
pub fn compress_in_place(
    src: &Path,
    fmt: Format,
    level: Option<i32>,
    opts: Options,
) -> Result<PathBuf> {
    let out_path = compressed_path(src, fmt);
    let output = open_output(&out_path, opts.force)?;
    let input = BufReader::new(File::open(src)?);
    if let Err(e) = encode(input, output, fmt, level) {
        let _ = fs::remove_file(&out_path);
        return Err(e);
    }
    if !opts.keep {
        fs::remove_file(src)?;
    }
    Ok(out_path)
}

/// Decompress `src` in place. Detects format; removes `src` unless `opts.keep`.
pub fn decompress_in_place(src: &Path, opts: Options) -> Result<(PathBuf, Format)> {
    let fmt = detect_format(src)?;
    let out_path = decompressed_path(src);
    let output = open_output(&out_path, opts.force)?;
    let input = BufReader::new(File::open(src)?);
    if let Err(e) = decode(input, output, fmt, opts.max_decompressed) {
        let _ = fs::remove_file(&out_path);
        return Err(e);
    }
    if !opts.keep {
        fs::remove_file(src)?;
    }
    Ok((out_path, fmt))
}

/// Stream the compressed form of `src` into `out`. Never touches the source.
pub fn compress_to_writer<W: Write>(
    src: &Path,
    fmt: Format,
    level: Option<i32>,
    out: W,
) -> Result<()> {
    let input = BufReader::new(File::open(src)?);
    encode(input, out, fmt, level)
}

/// Stream the decompressed form of `src` into `out`. Returns the detected format.
/// `max_decompressed` caps bytes written; pass `None` for no cap.
pub fn decompress_to_writer<W: Write>(
    src: &Path,
    out: W,
    max_decompressed: Option<u64>,
) -> Result<Format> {
    let fmt = detect_format(src)?;
    let input = BufReader::new(File::open(src)?);
    decode(input, out, fmt, max_decompressed)?;
    Ok(fmt)
}

/// `gzip -t` equivalent: decompress `src` into a sink and report any error.
pub fn test_file(src: &Path) -> Result<Format> {
    decompress_to_writer(src, io::sink(), None)
}

fn detect_format(src: &Path) -> Result<Format> {
    let mut head = [0u8; 6];
    let mut f = File::open(src)?;
    let n = f.read(&mut head)?;
    Format::detect(&head[..n]).ok_or(Error::UnknownFormat)
}

fn open_output(path: &Path, force: bool) -> Result<BufWriter<File>> {
    // --force must never follow a pre-existing symlink to truncate its target.
    // We unlink first (remove_file removes the symlink itself, not its target)
    // and then always use create_new, so any race loses to the new file.
    if force {
        match fs::symlink_metadata(path) {
            Ok(_) => fs::remove_file(path)?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(Error::Io(e)),
        }
    }
    let f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| match e.kind() {
            io::ErrorKind::AlreadyExists => Error::OutputExists(path.to_path_buf()),
            _ => Error::Io(e),
        })?;
    Ok(BufWriter::new(f))
}

fn encode<R: Read, W: Write>(mut input: R, output: W, fmt: Format, level: Option<i32>) -> Result<()> {
    match fmt {
        Format::Zstd => {
            let lvl = level.unwrap_or(3);
            let mut enc = zstd::stream::write::Encoder::new(output, lvl)?;
            io::copy(&mut input, &mut enc)?;
            let mut out = enc.finish()?;
            out.flush()?;
        }
        Format::Gzip => {
            let lvl = level.map(|l| l.clamp(0, 9) as u32).unwrap_or(6);
            let mut enc = flate2::write::GzEncoder::new(output, flate2::Compression::new(lvl));
            io::copy(&mut input, &mut enc)?;
            let mut out = enc.finish()?;
            out.flush()?;
        }
        Format::Xz => {
            let lvl = level.map(|l| l.clamp(0, 9) as u32).unwrap_or(6);
            let mut enc = xz2::write::XzEncoder::new(output, lvl);
            io::copy(&mut input, &mut enc)?;
            let mut out = enc.finish()?;
            out.flush()?;
        }
        Format::Bzip2 => {
            let lvl = match level {
                Some(l) => bzip2::Compression::new(l.clamp(1, 9) as u32),
                None => bzip2::Compression::default(),
            };
            let mut enc = bzip2::write::BzEncoder::new(output, lvl);
            io::copy(&mut input, &mut enc)?;
            let mut out = enc.finish()?;
            out.flush()?;
        }
        other => return Err(Error::UnsupportedFormat(other)),
    }
    Ok(())
}

fn decode<R: Read, W: Write>(
    input: R,
    output: W,
    fmt: Format,
    max_decompressed: Option<u64>,
) -> Result<()> {
    match max_decompressed {
        None => decode_stream(input, output, fmt),
        Some(limit) => {
            let mut capped = CappedWriter::new(output, limit);
            let res = decode_stream(input, &mut capped, fmt);
            if capped.tripped {
                Err(Error::MaxSizeExceeded(limit))
            } else {
                res
            }
        }
    }
}

fn decode_stream<R: Read, W: Write>(input: R, mut output: W, fmt: Format) -> Result<()> {
    match fmt {
        Format::Zstd => {
            let mut dec = zstd::stream::read::Decoder::new(input)?;
            io::copy(&mut dec, &mut output)?;
        }
        Format::Gzip => {
            let mut dec = flate2::read::GzDecoder::new(input);
            io::copy(&mut dec, &mut output)?;
        }
        Format::Xz => {
            let mut dec = xz2::read::XzDecoder::new(input);
            io::copy(&mut dec, &mut output)?;
        }
        Format::Bzip2 => {
            let mut dec = bzip2::read::BzDecoder::new(input);
            io::copy(&mut dec, &mut output)?;
        }
        other => return Err(Error::UnsupportedFormat(other)),
    }
    output.flush()?;
    Ok(())
}

/// Writer that short-circuits with an error once its byte budget is exhausted.
/// The decoder turns any write error into a transport error, so we stash a
/// `tripped` flag and convert it to [`Error::MaxSizeExceeded`] in [`decode`].
struct CappedWriter<W: Write> {
    inner: W,
    remaining: u64,
    tripped: bool,
}

impl<W: Write> CappedWriter<W> {
    fn new(inner: W, limit: u64) -> Self {
        Self { inner, remaining: limit, tripped: false }
    }
}

impl<W: Write> Write for CappedWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.len() as u64 > self.remaining {
            self.tripped = true;
            return Err(io::Error::other("decompressed output exceeded --max-size"));
        }
        let n = self.inner.write(buf)?;
        self.remaining -= n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    const PAYLOAD: &[u8] = b"the quick brown fox jumps over the lazy dog\n\
        the rain in spain falls mainly on the plain\n\
        pack my box with five dozen liquor jugs\n";

    fn roundtrip(fmt: Format) {
        let dir = tempdir().unwrap();
        let src = dir.path().join("sample.txt");
        fs::write(&src, PAYLOAD).unwrap();

        let compressed = compress_in_place(&src, fmt, None, Options::default()).unwrap();
        assert!(compressed.exists());
        assert!(!src.exists());
        assert_eq!(compressed.extension().unwrap(), fmt.suffix());

        let (restored, detected) = decompress_in_place(&compressed, Options::default()).unwrap();
        assert_eq!(detected, fmt);
        assert!(!compressed.exists());
        assert_eq!(restored, src);
        assert_eq!(fs::read(&restored).unwrap(), PAYLOAD);
    }

    #[test]
    fn roundtrip_zstd() { roundtrip(Format::Zstd); }
    #[test]
    fn roundtrip_gzip() { roundtrip(Format::Gzip); }
    #[test]
    fn roundtrip_xz() { roundtrip(Format::Xz); }
    #[test]
    fn roundtrip_bzip2() { roundtrip(Format::Bzip2); }

    #[test]
    fn decompress_with_misleading_extension() {
        let dir = tempdir().unwrap();
        let gz_src = dir.path().join("real.txt");
        fs::write(&gz_src, PAYLOAD).unwrap();
        let compressed = compress_in_place(&gz_src, Format::Gzip, None, Options::default()).unwrap();

        let lying = dir.path().join("lying.xz");
        fs::rename(&compressed, &lying).unwrap();

        let (restored, detected) = decompress_in_place(&lying, Options::default()).unwrap();
        assert_eq!(detected, Format::Gzip);
        assert_eq!(restored, dir.path().join("lying"));
        assert_eq!(fs::read(&restored).unwrap(), PAYLOAD);
    }

    #[test]
    fn compress_refuses_to_clobber() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("a.txt");
        fs::write(&src, PAYLOAD).unwrap();
        fs::write(dir.path().join("a.txt.zst"), b"existing").unwrap();

        match compress_in_place(&src, Format::Zstd, None, Options::default()) {
            Err(Error::OutputExists(_)) => {}
            other => panic!("expected OutputExists, got {other:?}"),
        }
        assert!(src.exists());
    }

    #[test]
    fn decompress_unknown_format_errors() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("plain.bin");
        fs::write(&src, b"not a compressed file at all, nope").unwrap();
        match decompress_in_place(&src, Options::default()) {
            Err(Error::UnknownFormat) => {}
            other => panic!("expected UnknownFormat, got {other:?}"),
        }
        assert!(src.exists());
    }

    // ---- new: Options (keep, force) ----

    #[test]
    fn keep_preserves_source_on_compress() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("a.txt");
        fs::write(&src, PAYLOAD).unwrap();

        let opts = Options { keep: true, ..Options::default() };
        let out = compress_in_place(&src, Format::Zstd, None, opts).unwrap();
        assert!(src.exists(), "keep=true must preserve source");
        assert!(out.exists());
    }

    #[test]
    fn keep_preserves_source_on_decompress() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("a.txt");
        fs::write(&src, PAYLOAD).unwrap();
        let compressed =
            compress_in_place(&src, Format::Gzip, None, Options::default()).unwrap();

        let opts = Options { keep: true, ..Options::default() };
        let (restored, _) = decompress_in_place(&compressed, opts).unwrap();
        assert!(compressed.exists(), "keep=true must preserve compressed source");
        assert!(restored.exists());
    }

    #[test]
    fn force_overwrites_existing_output_on_compress() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("a.txt");
        fs::write(&src, PAYLOAD).unwrap();
        let stale = dir.path().join("a.txt.zst");
        fs::write(&stale, b"stale contents").unwrap();

        let opts = Options { force: true, ..Options::default() };
        compress_in_place(&src, Format::Zstd, None, opts).unwrap();
        assert!(stale.exists());
        // output must have been truncated & rewritten, not left as "stale contents"
        assert_ne!(fs::read(&stale).unwrap(), b"stale contents");
    }

    #[test]
    fn force_overwrites_existing_output_on_decompress() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("a.txt");
        fs::write(&src, PAYLOAD).unwrap();
        let compressed =
            compress_in_place(&src, Format::Xz, None, Options::default()).unwrap();
        // simulate a stale `a.txt` in the way
        fs::write(dir.path().join("a.txt"), b"stale").unwrap();

        let opts = Options { force: true, ..Options::default() };
        decompress_in_place(&compressed, opts).unwrap();
        assert_eq!(fs::read(dir.path().join("a.txt")).unwrap(), PAYLOAD);
    }

    // ---- new: streaming ----

    #[test]
    fn compress_to_writer_then_decompress_to_writer() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("a.txt");
        fs::write(&src, PAYLOAD).unwrap();

        let mut buf: Vec<u8> = Vec::new();
        compress_to_writer(&src, Format::Zstd, None, &mut buf).unwrap();
        assert_eq!(&buf[..4], &[0x28, 0xB5, 0x2F, 0xFD]);
        assert!(src.exists(), "streaming must never touch source");

        let staged = dir.path().join("staged.zst");
        fs::write(&staged, &buf).unwrap();
        let mut plain = Vec::new();
        let fmt = decompress_to_writer(&staged, &mut plain, None).unwrap();
        assert_eq!(fmt, Format::Zstd);
        assert_eq!(plain, PAYLOAD);
    }

    // ---- new: test mode ----

    #[test]
    fn test_file_passes_valid_archive() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("a.txt");
        fs::write(&src, PAYLOAD).unwrap();
        let compressed =
            compress_in_place(&src, Format::Bzip2, None, Options::default()).unwrap();
        assert_eq!(test_file(&compressed).unwrap(), Format::Bzip2);
    }

    #[test]
    fn test_file_fails_on_corruption() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("a.txt");
        fs::write(&src, PAYLOAD).unwrap();
        let compressed =
            compress_in_place(&src, Format::Gzip, None, Options::default()).unwrap();

        // Flip a byte in the deflate stream (skip the 10-byte gzip header).
        let mut bytes = fs::read(&compressed).unwrap();
        let i = 12.min(bytes.len() - 1);
        bytes[i] ^= 0xFF;
        fs::write(&compressed, &bytes).unwrap();

        assert!(test_file(&compressed).is_err(), "corrupted archive must fail test");
    }

    // ---- new: decompression size cap ----

    #[test]
    fn max_size_aborts_when_output_exceeds_cap() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("bomb.txt");
        // PAYLOAD is ~130 bytes; cap at 10 → decoder must abort.
        fs::write(&src, PAYLOAD).unwrap();
        let compressed =
            compress_in_place(&src, Format::Zstd, None, Options::default()).unwrap();

        let mut sink = Vec::new();
        let err = decompress_to_writer(&compressed, &mut sink, Some(10)).unwrap_err();
        match err {
            Error::MaxSizeExceeded(10) => {}
            other => panic!("expected MaxSizeExceeded(10), got {other:?}"),
        }
    }

    #[test]
    fn max_size_passes_when_output_fits() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("small.txt");
        fs::write(&src, PAYLOAD).unwrap();
        let compressed =
            compress_in_place(&src, Format::Zstd, None, Options::default()).unwrap();

        let mut sink = Vec::new();
        decompress_to_writer(&compressed, &mut sink, Some(64 * 1024)).unwrap();
        assert_eq!(sink, PAYLOAD);
    }

    #[test]
    fn decompress_in_place_honors_max_size() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("a.txt");
        fs::write(&src, PAYLOAD).unwrap();
        let compressed =
            compress_in_place(&src, Format::Gzip, None, Options::default()).unwrap();

        let opts = Options { max_decompressed: Some(10), ..Options::default() };
        let res = decompress_in_place(&compressed, opts);
        assert!(matches!(res, Err(Error::MaxSizeExceeded(10))));
        // On abort we must not leave a partial output file behind, and the
        // compressed source must still be present (keep semantics on failure).
        assert!(!dir.path().join("a.txt").exists(), "partial output not cleaned up");
        assert!(compressed.exists(), "source removed on max-size failure");
    }

    // ---- new: symlink hardening (Unix only — Windows symlinks need admin) ----

    #[cfg(unix)]
    #[test]
    fn force_replaces_symlink_rather_than_clobbering_target() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().unwrap();
        let src = dir.path().join("a.txt");
        fs::write(&src, PAYLOAD).unwrap();

        // Victim file that must NOT be touched.
        let victim = dir.path().join("victim.conf");
        fs::write(&victim, b"DO NOT TOUCH\n").unwrap();

        // Pre-plant a symlink at the output path pointing at the victim.
        let out_path = dir.path().join("a.txt.zst");
        symlink(&victim, &out_path).unwrap();

        let opts = Options { force: true, ..Options::default() };
        compress_in_place(&src, Format::Zstd, None, opts).unwrap();

        // Victim must be untouched; the symlink must now be a regular file
        // containing zstd data.
        assert_eq!(fs::read(&victim).unwrap(), b"DO NOT TOUCH\n");
        let meta = fs::symlink_metadata(&out_path).unwrap();
        assert!(
            meta.file_type().is_file(),
            "output path must be a regular file, not a symlink"
        );
        let bytes = fs::read(&out_path).unwrap();
        assert_eq!(&bytes[..4], &[0x28, 0xB5, 0x2F, 0xFD]);
    }
}
