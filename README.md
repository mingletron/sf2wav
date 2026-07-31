# sf2wav

A fast, multithreaded Rust tool for extracting WAV files from SoundFont (.sf2) files.

## Features

- **Batch Processing**: Convert entire directories of SF2 files at once
- **Multithreaded**: Automatically uses all CPU cores for parallel processing
- **Organized Output**: Creates separate folders for each SoundFont's samples
- **Preserves Metadata**: Maintains sample rates and other audio properties
- **Fast**: Built in Rust for maximum performance

## Installation

### From Source

```bash
# Clone or download this repository
cd sf2wav
cargo build --release
```

The binary will be at `target/release/sf2wav`

### Install Globally

```bash
cargo install --path .
```

## Usage

### Extract samples from a single SF2 file

```bash
./target/release/sf2wav soundfont.sf2
./target/release/sf2wav soundfont.sf2 ./output
```

### Batch convert all SF2 files in a directory

```bash
./target/release/sf2wav /path/to/sf2_files/
./target/release/sf2wav /path/to/sf2_files/ ./extracted_samples
```

## Output Structure

When processing a directory, the tool creates the following structure:

```
output/
├── AccGtr02/
│   ├── Sample 001.wav
│   ├── Sample 002.wav
│   └── ...
├── HornFrMute11/
│   ├── HornFrMute11-1.wav
│   ├── HornFrMute11-2.wav
│   └── ...
└── BamFlte01/
    └── BamFlte01.wav
```

Each SF2 file gets its own folder, and all samples from that SoundFont are extracted as individual WAV files.

## How It Works

The tool parses the SF2 file format:
1. Reads the RIFF chunk structure
2. Extracts sample data from the `smpl` chunk
3. Reads sample headers from the `shdr` chunk to identify individual samples
4. Saves each sample as a 16-bit PCM WAV file

## Performance

- Processes multiple SF2 files in parallel using Rayon
- Automatically detects and uses the optimal number of threads
- To control thread count, set the `RAYON_NUM_THREADS` environment variable:

```bash
RAYON_NUM_THREADS=4 ./target/release/sf2wav /path/to/sf2_files/
```

## Building

```bash
# Debug build
cargo build

# Release build (optimized)
cargo build --release

# Run tests
cargo test
```

## Technical Details

- **Audio Format**: 16-bit PCM, mono WAV files
- **Sample Rates**: Preserved from original SF2 file
- **Dependencies**: Only Rayon for parallel processing
- **Platforms**: macOS, Linux, Windows (any Rust-supported platform)

## Limitations

- Only extracts monophonic samples (stereo samples are split into L/R)
- Does not preserve loop points in the WAV output
- Does not convert sample rates (preserves original rates)

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request.

## License

MIT

## Acknowledgments

- SoundFont 2.0 specification by the MIDI Manufacturers Association
- Built with [Rust](https://www.rust-lang.org/) and [Rayon](https://github.com/rayon-rs/rayon)
