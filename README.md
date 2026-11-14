# sf2wav

A fast, multithreaded Rust tool for extracting WAV files from SoundFont (.sf2) files.

[![Tests](https://github.com/mingletron/sf2wav/actions/workflows/tests.yml/badge.svg)](https://github.com/mingletron/sf2wav/actions/workflows/tests.yml)

## Features

- **Batch Processing**: Convert entire directories of SF2 files at once
- **Stereo Support**: Automatically detects and extracts stereo samples as proper stereo WAV files
- **Loop Point Preservation**: Preserves loop points from SF2 files into WAV files using the `smpl` chunk standard
- **CSV Export**: Generates a comprehensive CSV file with sample metadata (sample rate, duration, loop points, MIDI info, etc.)
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

Found 2 .sf2 file(s) to process
Output directory: ./output
Using 8 threads for parallel processing

Processing: /path/to/sf2_files/BassSynth12.sf2
Extracted (stereo): BassSynth12-1.wav (44100 Hz, 60928 frames)
Extracted (stereo): BassSynth12-2.wav (44100 Hz, 62976 frames)
Extracted (mono): BassSynth12-3.wav (44100 Hz, 63488 samples)

Processing: /path/to/sf2_files/Piano.sf2
Extracted (mono): Piano-001.wav (44100 Hz, 45632 samples)
Extracted (mono): Piano-002.wav (44100 Hz, 51200 samples)
  ✓ BassSynth12: 3 samples extracted
  ✓ Piano: 2 samples extracted
  CSV metadata written to: ./output/samples.csv
========================================
Batch conversion complete!
  Successful: 2 files
  Total samples extracted: 5
  Output directory: ./output
```

### Example CSV Content

```csv
SF2 File,Name,Filename,Sample Rate,Channels,Num Samples,Duration (seconds),Start,End,Start Loop,End Loop,Has Loop,Original Key,Correction,Sample Type,Is Stereo Pair,Linked Sample
"BassSynth12.sf2","BassSynth12-1.wav-L","BassSynth12-1.wav",44100,2,"121856",1.382,0,60928,0,60927,true,"53",23,"0x0004",true,"BassSynth12-1.wav-R"
"BassSynth12.sf2","BassSynth12-2.wav-L","BassSynth12-2.wav",44100,2,"125952",1.428,121948,184924,121948,184923,true,"60",23,"0x0004",true,"BassSynth12-2.wav-R"
```

## Output Structure

When processing a directory, the tool creates the following structure:

```
output/
├── samples.csv               (consolidated metadata for all samples)
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
- **CSV metadata**: A single `samples.csv` file is generated at the output root with comprehensive sample information for ALL converted SF2 files

### CSV Metadata File

The tool generates one `samples.csv` file at the root of the output directory containing:

| Column | Description |
|--------|-------------|
| SF2 File | Original SF2 filename |
| Name | Original sample name from SF2 |
| Filename | Output WAV filename |
| Sample Rate | Sample rate in Hz |
| Channels | Number of channels (1=mono, 2=stereo) |
| Num Samples | Total number of sample values written (frames x channels) |
| Duration (seconds) | Sample duration |
| Start | Start point in the SF2 file |
| End | End point in the SF2 file |
| Start Loop | Loop start point, as stored in the SF2 file |
| End Loop | Loop end point, as stored in the SF2 file (one past the last looped sample) |
| Has Loop | Whether the loop points are valid and were written to the WAV |
| Original Key | MIDI note number |
| Correction | Pitch correction in cents |
| Sample Type | SF2 sample type (hex) |
| Is Stereo Pair | Whether sample is part of a stereo pair |
| Linked Sample | Name of linked sample (for stereo pairs) |

## How It Works

The tool parses the SF2 file format:
1. Reads the RIFF chunk structure
2. Extracts sample data from the `smpl` chunk
3. Reads sample headers from the `shdr` chunk to identify individual samples
4. Detects stereo sample pairs using the `sample_link` field in sample headers
5. Reads loop points (`start_loop` and `end_loop`) from sample headers
6. Saves samples as 16-bit PCM WAV files (mono or stereo as appropriate)
7. Writes loop points to WAV files using the standard `smpl` chunk format

### Stereo Sample Handling

SoundFont files can contain stereo samples stored as pairs of mono samples. The tool:
- Detects stereo pairs via the `sample_link` field in sample headers (the zero-based index of the other channel)
- Identifies left and right from the `sample_type` field (4 = left, 2 = right), falling back to name suffixes (`-L`/`-R`, `_L`/`_R`) only when the type does not say
- Interleaves left and right channel data (LRLRLR...) into proper stereo WAV files
- Removes the stereo suffix from the file name of a pair; unpaired samples keep their full names
- If one half of a pair is unusable, the other half is still extracted as a mono file

### Loop Point Preservation

Loop points define a section of the sample that should repeat during playback. The tool:
- Reads loop start and end points from SF2 sample headers
- Writes loop points to WAV files using the standard `smpl` chunk format, relative to the start of the sample. The SF2 end point is one past the loop, the `smpl` end point is the last looped sample, so it is written one lower
- Sets the `smpl` chunk's MIDI unity note from the sample's original key
- The `smpl` chunk is supported by many DAWs (Logic Pro, Ableton Live, etc.)
- If the loop points are not a valid range inside the sample, no `smpl` chunk is added

### File Names

Characters that are not allowed in file names are replaced with `_`. If two samples in one SoundFont would get the same file name, the later ones get ` (2)`, ` (3)` and so on, so nothing is overwritten. Samples with no name are called `sample_NNN`.

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
- **Stereo Detection**: Automatic via the `sample_link` and `sample_type` fields
- **Loop Points**: Preserved using WAV `smpl` chunk (de facto standard)
- **CSV Export**: Comma-separated values with quoted strings, compatible with Excel and Google Sheets
- **Dependencies**: Only Rayon for parallel processing
- **Platforms**: macOS, Linux, Windows (any Rust-supported platform)

## Limitations

- Does not convert sample rates (preserves original rates)
- Stereo sample pairing depends on correct SF2 metadata (some poorly-formed files may not pair correctly)
- Whether a sample actually loops is decided per instrument in an SF2 file (the `sampleModes` generator), which this tool does not read. Loop points are written whenever the sample header has a valid loop range, even if no instrument uses it
- ROM samples (data held in a sound card rather than the file) are skipped
- 24-bit sample data (`sm24` chunk) is ignored; output is always 16-bit
- A sample rate of 0 in a header is replaced with 44100 Hz, with a warning

The exit code is 1 if any SF2 file could not be processed.

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request.

## License

MIT

## Acknowledgments

- SoundFont 2.0 specification by the MIDI Manufacturers Association
- Built with [Rust](https://www.rust-lang.org/) and [Rayon](https://github.com/rayon-rs/rayon)
