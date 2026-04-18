# zist

Format-agnostic, in-place (de)compression wrappers in Rust. One binary, two
names: `zist` compresses, `unzist` decompresses. Flags and conventions follow
`gzip`, so muscle memory transfers.

```
zist big.log                # -> big.log.zst
gzist big.log               # -> big.log.gz
xzist -9 big.log            # -> big.log.xz, level 9
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
- **argv[0] dispatch:** the binary looks at its own name.
  - `zist` → compress, zstd (default)
  - `unzist` → decompress (magic-byte detection)
  - `gzist` / `xzist` / `bzist` → compress as gzip / xz / bz2

  `-d` / `-z` switch mode, `--format` overrides the argv[0] hint.
- **Windows-friendly globs:** `zist *.log` works from `cmd`, PowerShell, or
  Nushell — globs are expanded in-process when the shell doesn't do it.
- **Safe in-place:** original is removed only after the new file is fully
  written; `-k` keeps the source.

## Install

From source:

```sh
cargo install --path .
```

This installs all five binaries (`zist`, `unzist`, `gzist`, `xzist`, `bzist`)
into `~/.cargo/bin/`.

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
      --format FMT          zstd | gz | xz | bz2 (compress only; default zstd)
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
