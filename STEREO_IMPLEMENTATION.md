# Stereo Sample Extraction - Implementation Analysis

## Current State

The implementation **already supports stereo sample extraction**. Stereo samples in the SF2 file are detected, paired, interleaved, and written as stereo WAV files (2 channels). Mono samples are extracted as mono WAV files (1 channel).

### Key Components

1. **SampleHeader struct** parses stereo-related fields from the SF2 `shdr` chunk:
   - `sample_link: u16` — 1-based index of the linked sample (other channel in stereo pair)
   - `sample_type: u16` — bit field indicating mono/left/right/ROM

2. **SampleHeader helper methods**:
   - `is_stereo()` — returns true if `sample_link > 0`
   - `is_left_channel()` — checks `(sample_type & 0x0003) == 2`
   - `is_right_channel()` — checks `(sample_type & 0x0003) == 1`
   - `is_mono()` — checks `(sample_type & 0x0003) == 0`

3. **StereoPair struct**:
   - Holds `left_idx`, `right_idx`, and `name` (base name from left channel)

4. **`find_stereo_pairs()`** function:
   - Iterates all sample headers, matches left/right pairs
   - Uses `sample_type` bits first, then falls back to name-based detection (`-L`/`-R`/`_L`/`_R` suffixes)
   - If no L/R indicator in names, uses lower index as left
   - Tracks `processed` indices to avoid double-pairing

5. **`extract_samples()`** function:
   - First loop: extracts stereo pairs → interleaved `LRLRLR...` samples → writes **stereo WAV** (2 channels)
   - Second loop: extracts remaining mono samples → writes **mono WAV** (1 channel)
   - Both paths support loop points via an optional `smpl` chunk in the WAV

6. **`write_wav()`** function:
   - Accepts `channels: u16` parameter (1 or 2)
   - Computes `byte_rate`, `block_align` from channel count
   - Writes RIFF/WAVE header, fmt chunk, data chunk
   - Optionally writes `smpl` chunk for loop metadata

7. **`sanitize_filename()`** function:
   - Strips `-L`/`-R`/`_L`/`_R` suffixes from stereo sample names
   - Removes `.wav` extension if present
   - Replaces invalid filename characters with `_`

8. **`SampleMetadata` struct**:
   - Tracks `channels`, `is_stereo_pair`, `linked_sample`, and `sample_type` for CSV output

## SF2 Stereo Sample Format

According to the SoundFont 2.0 specification:

### Sample Type Values

The `sample_type` field uses these bit values:
- **Bit 0-1** (value & 0x0003):
  - `0` = Mono sample
  - `1` = Right sample (of stereo pair)
  - `2` = Left sample (of stereo pair)
  - `3` = Reserved

- **Bit 2** (value & 0x0004):
  - `0` = Unlinked sample
  - `1` = Linked sample (part of a group)

- **Bit 15** (value & 0x8000):
  - `0` = RAM sample
  - `1` = ROM sample

Common values:
- `0x0000` = Mono RAM sample
- `0x0001` = Right channel RAM sample
- `0x0002` = Left channel RAM sample
- `0x8000` = Mono ROM sample
- `0x8001` = Right channel ROM sample
- `0x8002` = Left channel ROM sample

### Sample Linking

- Stereo samples are stored as **two separate mono samples** in the `smpl` chunk
- The `sample_link` field contains a **1-based index** of the linked sample header
- The code converts this to 0-based via `(header.sample_link as usize).saturating_sub(1)`
- Left and right samples are typically consecutive but not guaranteed

### Data Layout

```
smpl chunk: [Left Channel Samples][Right Channel Samples]
                              ^               ^
                              |               |
                        Sample A         Sample B
                        (sample_type=2)   (sample_type=1)
                        sample_link=B     sample_link=A
```

## Implementation Details

### Stereo Pair Detection

The `find_stereo_pairs` function uses a multi-step approach:

1. **Type-based matching**: Checks `sample_type` bits to determine left/right
2. **Name-based fallback**: If type bits are ambiguous, checks for `-L`/`-R`/`_L`/`_R` suffixes
3. **Index-based fallback**: If no L/R indicator in names, lower index is treated as left

### Sample Interleaving

Stereo samples are interleaved in `LRLRLR...` order:

```rust
let stereo_samples: Vec<i16> = (0..num_frames)
    .flat_map(|i| vec![left_samples[i], right_samples[i]])
    .collect();
```

The shorter channel length is used to ensure both channels have the same number of frames.

### WAV Output

- **Stereo WAV**: 2 channels, `block_align = 4`, `byte_rate = sample_rate * 4`
- **Mono WAV**: 1 channel, `block_align = 2`, `byte_rate = sample_rate * 2`
- Both support an optional `smpl` chunk for loop metadata

### Loop Point Handling

Loop points are validated before use:
- `start_loop` must be `> 0` and within `[start, end)`
- `end_loop` must be `> 0` and within `[start, end)`
- Values are converted to relative offsets from sample start
- Stereo pairs use the **left channel's** loop points

### Edge Cases Handled

1. **Mismatched sample lengths**: Uses the shorter length; logs a warning if either range is invalid
2. **Invalid `sample_link`**: Validates linked index is within bounds, not self-referential, and not already processed
3. **Broken links**: Skips the pair with a warning; the sample is not extracted
4. **Empty names**: Generates `sample_XXX` names for unnamed samples
5. **Terminal record**: Filters out headers with empty name or name "EOS" and those where `start >= end`
6. **Odd-size chunks**: Handles padding bytes in SF2 file format

### CSV Metadata

A CSV file is generated with columns tracking:
- `Channels` (1 or 2)
- `Is Stereo Pair` (true/false)
- `Linked Sample` (name of paired channel, empty for mono)
- `Sample Type` (hex value of `sample_type` field)

## Testing Considerations

1. **Stereo pair detection**: Verify with SF2 files containing known stereo samples
2. **Interleaved output**: Confirm WAV files play correctly in stereo audio players
3. **Mono samples**: Ensure mono samples still extract correctly alongside stereo
4. **Loop metadata**: Validate `smpl` chunk is written correctly in the WAV

## File Structure Reference

| File | Description |
|------|-------------|
| `src/main.rs` | All logic: parsing, stereo detection, WAV writing, CSV metadata |
| `STEREO_IMPLEMENTATION.md` | This documentation |
