# zist

Format-agnostic, in-place (de)compression wrappers in Rust. `zist`
compresses, `unzist` decompresses. Flags and conventions follow `gzip`, so
muscle memory transfers.

```
zist big.log                # -> big.log.zst
zist -F gz big.log          # -> big.log.gz
zist -F xz -9 big.log       # -> big.log.xz, level 9
unzist big.log.zst          # -> big.log (format detected by magic bytes)
unzist -t archive.tar.gz    # verify integrity, no writes
```

## Features

- **Formats:** zstd (default), gzip, xz, bz2 — encode and decode.
- **Auto-detect on decompress:** `unzist` reads magic bytes. `.zst`, `.gz`,
  `.xz`, `.bz2` all Just Work; lz4, lzip, and legacy `compress` are recognized
  (decode not yet implemented).
- **gzip-compatible flags:** `-k -c -f -v -q -t -r -1..-9 -d -z`, short-option
  bundling (`-kv`, `-9cv`).
- **Simple command surface:** `zist` compresses, `unzist` decompresses.
  `-d` / `-z` switch mode, and `--format` / `-F` choose the compression
  format when compressing.
- **Portable builds:** release binaries statically link bzip2 and xz so they
  don't rely on those runtime libraries being installed separately.
- **Windows-friendly globs:** `zist *.log` works from `cmd`, PowerShell, or
  Nushell — globs are expanded in-process when the shell doesn't do it.
- **Safe in-place:** original is removed only after the new file is fully
  written; `-k` keeps the source.

## Install

From source:

```sh
cargo install --path .
```

This installs `zist` and `unzist` into `~/.cargo/bin/`.

Examples:

```sh
zist file.txt              # default zstd
zist -F gz file.txt        # gzip
zist --format bz2 file.txt # bzip2
unzist file.txt.gz
```

## Usage

```
zist [OPTIONS] FILE...
unzist [OPTIONS] FILE...

  -k, --keep                keep source file
  -c, --stdout              write to stdout, keep source
  -f, --force               overwrite existing output
  -v, --verbose             per-file summary on stderr
  -q, --quiet               suppress per-file summary
  -r, --recursive           descend into directories
  -t, --test                verify archive integrity (decompress only)
  -d, --decompress          force decompress mode
  -z, --compress            force compress mode
  -F, --format FMT          zstd | gz | xz | bz2 (compress only; default zstd)
      --level N             compression level
  -1 .. -9                  level shortcut
  -h, --help                show help
  -V, --version             print version
```

Exit codes: `0` ok, `1` at least one file failed (batch continues), `2` usage
error.

## Build & test

```sh
cargo build --release
cargo test
```

Produces `target/release/zist` and `target/release/unzist`.

## Releases

GitHub Actions builds and tests native binaries for:

- `x86_64-unknown-linux-gnu`
- `x86_64-apple-darwin`
- `aarch64-apple-darwin`
- `x86_64-pc-windows-msvc`

Tagging `v*` publishes archives containing `zist`, `unzist`, `README.md`, and
`LICENSE`.

Download prebuilt binaries from the GitHub Releases page:
<https://github.com/maphew/zist/releases>

### Windows

The default MSVC toolchain needs Visual Studio Build Tools with the Windows
SDK component. If you already have MinGW, the GNU toolchain is a smaller
setup:

```sh
rustup toolchain install stable-x86_64-pc-windows-gnu
rustup override set stable-x86_64-pc-windows-gnu     # per-workspace
```

## License

MIT — see [LICENSE](LICENSE).
