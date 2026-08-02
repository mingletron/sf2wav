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

Found 5 .sf2 file(s) to process
Output directory: ./output
Using 8 threads for parallel processing

Processing: /path/to/sf2_files/BassSynth12.sf2
Extracted (stereo): BassSynth12-1.wav (44100 Hz, 60928 frames)
Extracted (stereo): BassSynth12-2.wav (44100 Hz, 62976 frames)
Extracted (mono): BassSynth12-3.wav (44100 Hz, 63488 samples)
  CSV metadata written to: ./output/BassSynth12/samples.csv

Processing: /path/to/sf2_files/Piano.sf2
Extracted (mono): Piano-001.wav (44100 Hz, 45632 samples)
Extracted (mono): Piano-002.wav (44100 Hz, 51200 samples)
  CSV metadata written to: ./output/Piano/samples.csv

========================================
Batch conversion complete!
  Successful: 2 files
  Total samples extracted: 5
  Output directory: ./output
```

### Example CSV Content

```csv
Name,Filename,Sample Rate,Channels,Num Samples,Duration (seconds),Start,End,Start Loop,End Loop,Has Loop,Original Key,Correction,Sample Type,Is Stereo Pair,Linked Sample
"BassSynth12-1.wav-L","BassSynth12-1.wav",44100,2,"121856",1.382,0,60928,0,60927,false,"53",23,"0x0004",true,"BassSynth12-1.wav-R"
"BassSynth12-2.wav-L","BassSynth12-2.wav",44100,2,"125952",1.428,121948,184924,121948,184923,true,"60",23,"0x0004",true,"BassSynth12-2.wav-R"
```

## Output Structure

When processing a directory, the tool creates the following structure:

```
output/
├── AccGtr02/
│   ├── Sample 001.wav        (mono)
│   ├── Sample 002.wav        (mono)
│   ├── samples.csv           (metadata)
│   └── ...
├── BassSynth12/
│   ├── BassSynth12-1.wav     (stereo)
│   ├── BassSynth12-2.wav     (stereo)
│   ├── samples.csv           (metadata)
│   └── ...
└── HornFrMute11/
    ├── HornFrMute11-1.wav    (mono)
    ├── samples.csv           (metadata)
    └── ...
```

Each SF2 file gets its own folder. Samples are automatically detected as mono or stereo:
- **Mono samples**: Extracted as single-channel WAV files
- **Stereo samples**: Detected via SF2 sample linking and extracted as proper two-channel WAV files with interleaved L/R data
- **CSV metadata**: A `samples.csv` file is generated with comprehensive sample information

### CSV Metadata File

The tool generates a `samples.csv` file in each output folder containing:

| Column | Description |
|--------|-------------|
| Name | Original sample name from SF2 |
| Filename | Output WAV filename |
| Sample Rate | Sample rate in Hz |
| Channels | Number of channels (1=mono, 2=stereo) |
| Num Samples | Total number of samples |
| Duration (seconds) | Sample duration |
| Start | Start point in the SF2 file |
| End | End point in the SF2 file |
| Start Loop | Loop start point (if defined) |
| End Loop | Loop end point (if defined) |
| Has Loop | Whether loop points are defined |
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
- Detects stereo pairs via the `sample_link` field in sample headers
- Identifies left/right channels by name suffixes (`-L`/`-R`, `_L`/`_R`)
- Interleaves left and right channel data (LRLRLR...) into proper stereo WAV files
- Automatically removes stereo suffixes from output filenames

### Loop Point Preservation

Loop points define a section of the sample that should repeat during playback. The tool:
- Reads loop start and end points from SF2 sample headers
- Writes loop points to WAV files using the standard `smpl` chunk format
- The `smpl` chunk is supported by many DAWs (Logic Pro, Ableton Live, etc.)
- If no loop points are defined in the SF2 file, no `smpl` chunk is added

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
- **Loop Points**: Preserved using WAV `smpl` chunk (de facto standard)
- **CSV Export**: Comma-separated values with quoted strings, compatible with Excel and Google Sheets
- **Dependencies**: Only Rayon for parallel processing
- **Platforms**: macOS, Linux, Windows (any Rust-supported platform)

## Limitations

- Does not convert sample rates (preserves original rates)
- Stereo sample pairing depends on correct SF2 metadata (some poorly-formed files may not pair correctly)
- Loop points are preserved only if defined in the SF2 file (many SF2 files don't define loop points)

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request.

## License

MIT

## Acknowledgments

- SoundFont 2.0 specification by the MIDI Manufacturers Association
- Built with [Rust](https://www.rust-lang.org/) and [Rayon](https://github.com/rayon-rs/rayon)
