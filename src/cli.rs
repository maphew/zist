use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::format::Format;
use crate::io::{
    compress_in_place, compress_to_writer, decompress_in_place, decompress_to_writer, test_file,
    Options,
};

const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn main() -> ExitCode {
    let mut args = env::args_os();
    let argv0 = args.next().unwrap_or_else(|| OsString::from("zist"));
    let rest: Vec<OsString> = args.collect();
    match run(&argv0, &rest) {
        Ok(()) => ExitCode::SUCCESS,
        Err(code) => ExitCode::from(code),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Compress,
    Decompress,
}

#[derive(Debug, Default)]
struct Cli {
    mode: Option<Mode>,
    format: Option<Format>,
    level: Option<i32>,
    keep: bool,
    force: bool,
    stdout: bool,
    quiet: bool,
    verbose: bool,
    test: bool,
    recursive: bool,
    help: bool,
    version: bool,
    max_size: Option<u64>,
    files: Vec<PathBuf>,
}

// The binary's own name selects the default mode. `unzist` flips to
// decompress; everything else behaves like `zist`. `-d`, `-z`, and
// `--format` / `-F` always override these hints.
fn defaults_from_argv0(argv0: &Path) -> Mode {
    let stem = argv0.file_stem().and_then(|s| s.to_str()).unwrap_or("zist");
    match stem.to_ascii_lowercase().as_str() {
        "unzist" => Mode::Decompress,
        _ => Mode::Compress,
    }
}

fn program_name(argv0: &Path) -> String {
    argv0.file_stem().and_then(|s| s.to_str()).unwrap_or("zist").to_string()
}

fn run(argv0: &OsString, args: &[OsString]) -> Result<(), u8> {
    let argv0_path = Path::new(argv0);
    let default_mode = defaults_from_argv0(argv0_path);
    let prog = program_name(argv0_path);
    let cli = parse_args(args)?;

    if cli.help {
        print_help(cli.mode.unwrap_or(default_mode), &prog, Format::Zstd);
        return Ok(());
    }
    if cli.version {
        println!("{prog} {VERSION}");
        return Ok(());
    }

    let mode = cli.mode.unwrap_or(default_mode);
    if cli.files.is_empty() {
        return Err(err("no input files"));
    }
    if cli.quiet && cli.verbose {
        return Err(err("--quiet and --verbose are mutually exclusive"));
    }
    if cli.stdout && cli.test {
        return Err(err("--stdout and --test are mutually exclusive"));
    }

    let files = expand_globs(cli.files.clone());
    let files = if cli.recursive { expand_recursive(&files) } else { files };

    let mut had_error = false;
    for f in &files {
        if let Err(e) = process_one(f, mode, &cli) {
            eprintln!("{}: {}", f.display(), e);
            had_error = true;
        }
    }
    if had_error { Err(1) } else { Ok(()) }
}

fn process_one(path: &Path, mode: Mode, cli: &Cli) -> Result<(), Box<dyn std::error::Error>> {
    // -c implies -k (source never removed).
    let keep = cli.keep || cli.stdout;
    let opts = Options {
        keep,
        force: cli.force,
        max_decompressed: cli.max_size,
    };

    match mode {
        Mode::Compress => {
            let fmt = cli.format.unwrap_or(Format::Zstd);
            if cli.test {
                return Err("--test only applies in decompress mode".into());
            }
            if cli.stdout {
                let stdout = io::stdout();
                let mut lock = stdout.lock();
                compress_to_writer(path, fmt, cli.level, &mut lock)?;
                if cli.verbose {
                    eprintln!("{} -> <stdout> [{fmt}]", path.display());
                }
            } else {
                let out = compress_in_place(path, fmt, cli.level, opts)?;
                report(cli, &format!("{} -> {}", path.display(), out.display()));
            }
        }
        Mode::Decompress => {
            if cli.test {
                let fmt = test_file(path)?;
                report(cli, &format!("{}: {} OK", path.display(), fmt));
            } else if cli.stdout {
                let stdout = io::stdout();
                let mut lock = stdout.lock();
                let fmt = decompress_to_writer(path, &mut lock, cli.max_size)?;
                if cli.verbose {
                    eprintln!("{} [{fmt}] -> <stdout>", path.display());
                }
            } else {
                let (out, fmt) = decompress_in_place(path, opts)?;
                report(cli, &format!("{} [{fmt}] -> {}", path.display(), out.display()));
            }
        }
    }
    Ok(())
}

fn report(cli: &Cli, msg: &str) {
    if cli.quiet {
        return;
    }
    if cli.verbose {
        let _ = writeln!(io::stderr(), "{msg}");
    } else {
        // default: quiet-ish like gzip (gzip prints nothing on success). We
        // keep a single-line note on stdout so batch runs are traceable, but
        // -q silences it.
        println!("{msg}");
    }
}

// Expand glob patterns in-process so Windows shells (cmd, PowerShell, Nushell)
// get the same `zist *.txt` UX that POSIX shells give for free. A literal match
// on disk always wins, so filenames containing `[`/`?` on Unix still work.
// Patterns that match nothing are passed through unchanged, producing the usual
// "no such file" error with the pattern text the user typed.
fn expand_globs(inputs: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut out = Vec::with_capacity(inputs.len());
    for p in inputs {
        if p.exists() || !has_glob_meta(&p) {
            out.push(p);
            continue;
        }
        let Some(pat) = p.to_str() else {
            out.push(p);
            continue;
        };
        match glob::glob(pat) {
            Ok(iter) => {
                let start = out.len();
                for entry in iter.flatten() {
                    out.push(entry);
                }
                if out.len() == start {
                    out.push(p);
                }
            }
            Err(_) => out.push(p),
        }
    }
    out
}

fn has_glob_meta(p: &Path) -> bool {
    p.to_str().is_some_and(|s| s.contains(['*', '?', '[']))
}

fn expand_recursive(inputs: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::with_capacity(inputs.len());
    for p in inputs {
        walk_into(p, &mut out);
    }
    out
}

fn walk_into(p: &Path, acc: &mut Vec<PathBuf>) {
    // symlink_metadata (not metadata) so a planted symlink cannot redirect
    // recursion out of the requested tree. Symlinks encountered during descent
    // are skipped entirely — the user can still compress a symlink by naming
    // it explicitly on the command line.
    match fs::symlink_metadata(p) {
        Ok(m) if m.file_type().is_symlink() => {}
        Ok(m) if m.is_dir() => match fs::read_dir(p) {
            Ok(entries) => {
                let mut children: Vec<_> = entries.flatten().map(|e| e.path()).collect();
                children.sort();
                for c in children {
                    walk_into(&c, acc);
                }
            }
            Err(_) => acc.push(p.to_path_buf()),
        },
        _ => acc.push(p.to_path_buf()),
    }
}

// ---------------- argv parser ----------------

fn parse_args(args: &[OsString]) -> Result<Cli, u8> {
    let mut cli = Cli::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let Some(s) = a.to_str() else {
            cli.files.push(PathBuf::from(a));
            continue;
        };

        if s == "--" {
            for rest in it.by_ref() {
                cli.files.push(PathBuf::from(rest));
            }
            break;
        }

        if let Some(long) = s.strip_prefix("--") {
            parse_long(long, &mut cli, &mut it)?;
        } else if let Some(short) = s.strip_prefix('-') {
            if short.is_empty() {
                // bare `-`: treat as filename (we don't support stdin input yet).
                cli.files.push(PathBuf::from(a));
            } else {
                parse_short(short, &mut cli, &mut it)?;
            }
        } else {
            cli.files.push(PathBuf::from(a));
        }
    }
    Ok(cli)
}

fn parse_long(
    long: &str,
    cli: &mut Cli,
    it: &mut std::slice::Iter<'_, OsString>,
) -> Result<(), u8> {
    let (name, attached) = match long.split_once('=') {
        Some((n, v)) => (n, Some(v)),
        None => (long, None),
    };
    match name {
        "help" => cli.help = true,
        "version" => cli.version = true,
        "keep" => cli.keep = true,
        "stdout" => cli.stdout = true,
        "force" => cli.force = true,
        "quiet" => cli.quiet = true,
        "verbose" => cli.verbose = true,
        "test" => cli.test = true,
        "recursive" => cli.recursive = true,
        "decompress" | "uncompress" => cli.mode = Some(Mode::Decompress),
        "compress" => cli.mode = Some(Mode::Compress),
        "format" => {
            let v = take_value(name, attached, it)?;
            cli.format = Some(v.parse().map_err(|_| err("unknown format (zstd|gz|xz|bz2)"))?);
        }
        "level" => {
            let v = take_value(name, attached, it)?;
            cli.level = Some(v.parse().map_err(|_| err("--level must be an integer"))?);
        }
        "max-size" => {
            let v = take_value(name, attached, it)?;
            cli.max_size = Some(parse_size(&v).map_err(|msg| err(&msg))?);
        }
        other => return Err(err(&format!("unknown option: --{other}"))),
    }
    Ok(())
}

fn parse_short(
    short: &str,
    cli: &mut Cli,
    it: &mut std::slice::Iter<'_, OsString>,
) -> Result<(), u8> {
    let mut chars = short.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            'h' => cli.help = true,
            'V' => cli.version = true,
            'k' => cli.keep = true,
            'c' => cli.stdout = true,
            'f' => cli.force = true,
            'q' => cli.quiet = true,
            'v' => cli.verbose = true,
            't' => cli.test = true,
            'r' => cli.recursive = true,
            'd' => cli.mode = Some(Mode::Decompress),
            'z' => cli.mode = Some(Mode::Compress),
            'F' => {
                let attached: String = chars.collect();
                let v = if attached.is_empty() {
                    take_value("format", None, it)?
                } else {
                    attached
                };
                cli.format = Some(v.parse().map_err(|_| err("unknown format (zstd|gz|xz|bz2)"))?);
                break;
            }
            '1'..='9' => {
                cli.level = Some(c.to_digit(10).unwrap() as i32);
                // If there are trailing characters they should also be valid flags.
                // `-9kv` is accepted.
            }
            other => return Err(err(&format!("unknown option: -{other}"))),
        }
    }
    Ok(())
}

fn take_value(
    name: &str,
    attached: Option<&str>,
    it: &mut std::slice::Iter<'_, OsString>,
) -> Result<String, u8> {
    if let Some(v) = attached {
        return Ok(v.to_string());
    }
    match it.next() {
        Some(v) => v.to_str().map(String::from).ok_or_else(|| err("value must be UTF-8")),
        None => Err(err(&format!("--{name} requires a value"))),
    }
}

/// Parse a byte size like `100`, `10k`, `512M`, `2G`. Base 1024, suffix
/// case-insensitive. Returns a human-readable error message.
fn parse_size(s: &str) -> std::result::Result<u64, String> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Err("--max-size requires a value".into());
    }
    let bytes = trimmed.as_bytes();
    let (digits, mul) = match bytes[bytes.len() - 1] {
        b'k' | b'K' => (&trimmed[..trimmed.len() - 1], 1024u64),
        b'm' | b'M' => (&trimmed[..trimmed.len() - 1], 1024u64 * 1024),
        b'g' | b'G' => (&trimmed[..trimmed.len() - 1], 1024u64 * 1024 * 1024),
        b't' | b'T' => (&trimmed[..trimmed.len() - 1], 1024u64 * 1024 * 1024 * 1024),
        _ => (trimmed, 1u64),
    };
    let n: u64 = digits
        .trim()
        .parse()
        .map_err(|_| format!("--max-size: invalid size `{s}`"))?;
    n.checked_mul(mul)
        .ok_or_else(|| format!("--max-size: value too large: `{s}`"))
}

fn err(msg: &str) -> u8 {
    eprintln!("error: {msg}");
    2
}

fn print_help(mode: Mode, prog: &str, default_fmt: Format) {
    match mode {
        Mode::Compress => println!(
            "{prog} {VERSION} - compress files in place (format-agnostic)\n\n\
             usage: {prog} [OPTIONS] FILE...\n\n\
             options:\n\
             \x20 -F, --format {{zstd|gz|xz|bz2}}  compression format (default: {default_fmt})\n\
             \x20     --level N                   compression level\n\
             \x20 -1 .. -9                         level shortcut\n\
             \x20 -k, --keep                       keep source file\n\
             \x20 -c, --stdout                     write to stdout, keep source\n\
             \x20 -f, --force                      overwrite existing output\n\
             \x20 -v, --verbose                    emit per-file summary on stderr\n\
             \x20 -q, --quiet                      suppress per-file summary\n\
             \x20 -r, --recursive                  descend directories\n\
             \x20 -d, --decompress                 decompress (same as unzist)\n\
             \x20 -h, --help                       show this help\n\
             \x20 -V, --version                    print version\n"
        ),
        Mode::Decompress => println!(
            "{prog} {VERSION} - decompress files in place (format auto-detected)\n\n\
             usage: {prog} [OPTIONS] FILE...\n\n\
             options:\n\
             \x20 -k, --keep                       keep source file\n\
             \x20 -c, --stdout                     write to stdout, keep source\n\
             \x20 -f, --force                      overwrite existing output\n\
             \x20 -t, --test                       verify integrity; never writes output\n\
             \x20     --max-size SIZE             abort if decompressed output exceeds SIZE\n\
             \x20                                  (accepts k/M/G/T suffix, base 1024)\n\
             \x20 -v, --verbose                    emit per-file summary on stderr\n\
             \x20 -q, --quiet                      suppress per-file summary\n\
             \x20 -r, --recursive                  descend directories\n\
             \x20 -z, --compress                   compress (same as zist)\n\
             \x20 -h, --help                       show this help\n\
             \x20 -V, --version                    print version\n"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_size_plain_bytes() {
        assert_eq!(parse_size("0").unwrap(), 0);
        assert_eq!(parse_size("1024").unwrap(), 1024);
    }

    #[test]
    fn parse_size_accepts_suffixes() {
        assert_eq!(parse_size("1k").unwrap(), 1024);
        assert_eq!(parse_size("1K").unwrap(), 1024);
        assert_eq!(parse_size("2m").unwrap(), 2 * 1024 * 1024);
        assert_eq!(parse_size("3G").unwrap(), 3u64 * 1024 * 1024 * 1024);
        assert_eq!(parse_size("1t").unwrap(), 1024u64 * 1024 * 1024 * 1024);
    }

    #[test]
    fn parse_size_rejects_garbage() {
        assert!(parse_size("").is_err());
        assert!(parse_size("abc").is_err());
        assert!(parse_size("1x").is_err());
        assert!(parse_size("-5").is_err());
    }

    #[test]
    fn parse_size_rejects_overflow() {
        assert!(parse_size("99999999999T").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn walk_into_skips_symlinked_directory() {
        use std::os::unix::fs::symlink;
        use tempfile::tempdir;

        // Target dir outside the tree the user asked to recurse.
        let outside = tempdir().unwrap();
        fs::write(outside.path().join("secret.txt"), b"secret").unwrap();

        // Tree the user named: contains a planted symlink to `outside`.
        let root = tempdir().unwrap();
        fs::write(root.path().join("ok.txt"), b"ok").unwrap();
        symlink(outside.path(), root.path().join("trap")).unwrap();

        let mut acc = Vec::new();
        walk_into(root.path(), &mut acc);

        for p in &acc {
            assert!(
                !p.starts_with(outside.path()),
                "walk_into followed symlink out of tree: {}",
                p.display()
            );
            // No path should include the planted symlink component either —
            // we skip it entirely during descent.
            assert!(
                !p.components().any(|c| c.as_os_str() == "trap"),
                "walk_into descended through symlink: {}",
                p.display()
            );
        }
        assert!(acc.iter().any(|p| p.file_name().unwrap() == "ok.txt"));
    }

    #[cfg(unix)]
    #[test]
    fn walk_into_skips_symlink_to_regular_file() {
        use std::os::unix::fs::symlink;
        use tempfile::tempdir;

        let outside = tempdir().unwrap();
        let target = outside.path().join("target.txt");
        fs::write(&target, b"target").unwrap();

        let root = tempdir().unwrap();
        fs::write(root.path().join("real.txt"), b"real").unwrap();
        symlink(&target, root.path().join("ln.txt")).unwrap();

        let mut acc = Vec::new();
        walk_into(root.path(), &mut acc);

        // real.txt should be processed; ln.txt (symlink) should be skipped.
        assert!(acc.iter().any(|p| p.file_name().unwrap() == "real.txt"));
        assert!(
            !acc.iter().any(|p| p.file_name().unwrap() == "ln.txt"),
            "walk_into must not emit symlink entries during descent"
        );
    }
}
