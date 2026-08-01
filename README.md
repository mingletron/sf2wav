# sf2wav

A fast, multithreaded Rust tool for extracting WAV files from SoundFont (.sf2) files.

[![Tests](https://github.com/mingletron/sf2wav/actions/workflows/tests.yml/badge.svg)](https://github.com/mingletron/sf2wav/actions/workflows/tests.yml)

## Features

- **Batch Processing**: Convert entire directories of SF2 files at once
- **Stereo Support**: Automatically detects and extracts stereo samples as proper stereo WAV files
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

### Example Output

```bash
$ ./target/release/sf2wav /path/to/sf2_files/

Found 5 .sf2 file(s) to process
Output directory: ./output
Using 8 threads for parallel processing

Processing: /path/to/sf2_files/BassSynth12.sf2
Extracted (stereo): BassSynth12-1.wav (44100 Hz, 60928 frames)
Extracted (stereo): BassSynth12-2.wav (44100 Hz, 62976 frames)
Extracted (mono): BassSynth12-3.wav (44100 Hz, 63488 samples)

Processing: /path/to/sf2_files/Piano.sf2
Extracted (mono): Piano-001.wav (44100 Hz, 45632 samples)
Extracted (mono): Piano-002.wav (44100 Hz, 51200 samples)

========================================
Batch conversion complete!
  Successful: 2 files
  Total samples extracted: 5
  Output directory: ./output
```

## Output Structure

When processing a directory, the tool creates the following structure:

```
output/
├── AccGtr02/
│   ├── Sample 001.wav        (mono)
│   ├── Sample 002.wav        (mono)
│   └── ...
├── BassSynth12/
│   ├── BassSynth12-1.wav     (stereo)
│   ├── BassSynth12-2.wav     (stereo)
│   └── ...
└── HornFrMute11/
    ├── HornFrMute11-1.wav    (mono)
    └── ...
```

Each SF2 file gets its own folder. Samples are automatically detected as mono or stereo:
- **Mono samples**: Extracted as single-channel WAV files
- **Stereo samples**: Detected via SF2 sample linking and extracted as proper two-channel WAV files with interleaved L/R data

## How It Works

The tool parses the SF2 file format:
1. Reads the RIFF chunk structure
2. Extracts sample data from the `smpl` chunk
3. Reads sample headers from the `shdr` chunk to identify individual samples
4. Detects stereo sample pairs using the `sample_link` field in sample headers
5. Saves samples as 16-bit PCM WAV files (mono or stereo as appropriate)

### Stereo Sample Handling

SoundFont files can contain stereo samples stored as pairs of mono samples. The tool:
- Detects stereo pairs via the `sample_link` field in sample headers
- Identifies left/right channels by name suffixes (`-L`/`-R`, `_L`/`_R`)
- Interleaves left and right channel data (LRLRLR...) into proper stereo WAV files
- Automatically removes stereo suffixes from output filenames

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

# Run tests with output
cargo test -- --nocapture
```

## Technical Details

- **Audio Format**: 16-bit PCM WAV files (mono or stereo)
- **Sample Rates**: Preserved from original SF2 file
- **Stereo Detection**: Automatic via `sample_link` field and name analysis
- **Dependencies**: Only Rayon for parallel processing
- **Platforms**: macOS, Linux, Windows (any Rust-supported platform)

## Limitations

- Does not preserve loop points in the WAV output
- Does not convert sample rates (preserves original rates)
- Stereo sample pairing depends on correct SF2 metadata (some poorly-formed files may not pair correctly)

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request.

## License

MIT

## Acknowledgments

- SoundFont 2.0 specification by the MIDI Manufacturers Association
- Built with [Rust](https://www.rust-lang.org/) and [Rayon](https://github.com/rayon-rs/rayon)
