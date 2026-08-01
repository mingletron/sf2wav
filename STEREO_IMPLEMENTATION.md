# Stereo Sample Extraction - Implementation Analysis

## Current State

The current implementation extracts all samples as mono WAV files, ignoring the stereo information in the SF2 file.

### Key Observations

1. **SampleHeader struct already parses stereo fields:**
   - `sample_link: u16` - Index of the linked sample (other channel)
   - `sample_type: u16` - Type of sample (mono, left, right, linked)

2. **These fields are currently unused:**
   - Line 32-33: Fields are parsed but marked as `#[allow(dead_code)]`
   - The extraction logic treats all samples as independent mono samples

3. **WAV writing is mono-only:**
   - Line 214: `write_all(&1u16.to_le_bytes())` hardcodes 1 channel
   - Line 202: `byte_rate = sample_rate * 2` assumes mono (2 bytes per sample)

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
- The `sample_link` field contains the **index** (0-based) of the linked sample header
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

## Required Changes

### 1. Define Sample Type Constants/Enum

```rust
#[derive(Debug, Clone, Copy, PartialEq)]
enum SampleType {
    Mono = 0,
    Right = 1,
    Left = 2,
    // ROM variants have bit 15 set
}

impl SampleType {
    fn from_u16(value: u16) -> Self {
        match value & 0x0003 {  // Mask off lower 2 bits
            0 => SampleType::Mono,
            1 => SampleType::Right,
            2 => SampleType::Left,
            _ => SampleType::Mono, // Reserved, treat as mono
        }
    }
    
    fn is_stereo_channel(&self) -> bool {
        matches!(self, SampleType::Left | SampleType::Right)
    }
}
```

### 2. Identify Stereo Pairs

Create a function to pair up stereo samples:

```rust
fn find_stereo_pairs(headers: &[SampleHeader]) -> Vec<StereoPair> {
    let mut pairs = Vec::new();
    let mut processed = vec![false; headers.len()];
    
    for (i, header) in headers.iter().enumerate() {
        if processed[i] {
            continue;
        }
        
        let sample_type = SampleType::from_u16(header.sample_type);
        
        // Check if this is a stereo channel
        if sample_type.is_stereo_channel() && header.sample_link > 0 {
            let linked_index = (header.sample_link - 1) as usize; // Convert to 0-based
            
            if linked_index < headers.len() && !processed[linked_index] {
                let (left, right) = if sample_type == SampleType::Left {
                    (i, linked_index)
                } else {
                    (linked_index, i)
                };
                
                pairs.push(StereoPair {
                    left_index: left,
                    right_index: right,
                    name: headers[left].name.clone(),
                });
                
                processed[i] = true;
                processed[linked_index] = true;
            }
        }
    }
    
    pairs
}
```

### 3. Extract and Interleave Stereo Samples

```rust
fn extract_stereo_wav(
    left_samples: &[i16],
    right_samples: &[i16],
    sample_rate: u32,
) -> Vec<u8> {
    let num_frames = left_samples.len().min(right_samples.len());
    let mut stereo_data = Vec::with_capacity(num_frames * 4); // 4 bytes per frame
    
    for i in 0..num_frames {
        // Interleave: LRLRLR
        stereo_data.extend_from_slice(&left_samples[i].to_le_bytes());
        stereo_data.extend_from_slice(&right_samples[i].to_le_bytes());
    }
    
    // Create WAV file with 2 channels
    create_stereo_wav(&stereo_data, sample_rate)
}
```

### 4. Modify WAV Writing for Stereo

```rust
fn write_wav<W: Write>(
    writer: &mut W,
    sample_data: &[i16],
    sample_rate: u32,
    channels: u16,  // New parameter
) -> Result<(), Sf2Error> {
    let num_samples = sample_data.len() / channels as usize;
    let byte_rate = sample_rate * channels as u32 * 2; // 16-bit * channels
    let data_size = sample_data.len() * 2;
    let block_align = channels * 2; // 2 bytes per sample * channels
    
    // RIFF header
    writer.write_all(b"RIFF")?;
    writer.write_all(&(36u32 + data_size as u32).to_le_bytes())?;
    writer.write_all(b"WAVE")?;
    
    // fmt chunk
    writer.write_all(b"fmt ")?;
    writer.write_all(&16u32.to_le_bytes())?;
    writer.write_all(&1u16.to_le_bytes())?; // PCM
    writer.write_all(&channels.to_le_bytes())?; // Number of channels
    writer.write_all(&sample_rate.to_le_bytes())?;
    writer.write_all(&byte_rate.to_le_bytes())?;
    writer.write_all(&block_align.to_le_bytes())?;
    writer.write_all(&16u16.to_le_bytes())?; // 16-bit
    
    // data chunk
    writer.write_all(b"data")?;
    writer.write_all(&(data_size as u32).to_le_bytes())?;
    
    for &sample in sample_data {
        writer.write_all(&sample.to_le_bytes())?;
    }
    
    Ok(())
}
```

### 5. Update Extraction Logic

```rust
fn extract_samples(sf2_path: &Path, output_dir: &Path) -> Result<Vec<String>, Sf2Error> {
    // ... existing parsing code ...
    
    let stereo_pairs = find_stereo_pairs(&sample_headers);
    let mut processed_indices = HashSet::new();
    
    // Extract stereo pairs
    for pair in stereo_pairs {
        processed_indices.insert(pair.left_index);
        processed_indices.insert(pair.right_index);
        
        let left_samples = &all_samples[pair.left_start..pair.left_end];
        let right_samples = &all_samples[pair.right_start..pair.right_end];
        
        // Interleave samples
        let stereo_samples: Vec<i16> = left_samples.iter()
            .zip(right_samples.iter())
            .flat_map(|(l, r)| vec![*l, *r])
            .collect();
        
        // Write stereo WAV
        write_wav(&mut output_file, &stereo_samples, header.sample_rate, 2)?;
    }
    
    // Extract remaining mono samples
    for (i, header) in sample_headers.iter().enumerate() {
        if processed_indices.contains(&i) {
            continue;
        }
        // ... existing mono extraction code ...
    }
}
```

## Edge Cases to Handle

1. **Mismatched sample lengths:** Left and right channels might have different lengths
   - Solution: Use the shorter length, or pad with zeros

2. **Sample rate mismatch:** Left and right should have same sample rate
   - Solution: Verify they match, use left channel's rate

3. **Broken links:** `sample_link` points to invalid index
   - Solution: Validate index, fall back to mono extraction

4. **Multiple stereo pairs with same name:**
   - Solution: Add suffix like "piano-L.wav", "piano-R.wav" or combine as "piano.wav" (stereo)

## Testing Strategy

1. **Unit tests:**
   - Test stereo pair detection
   - Test sample interleaving
   - Test WAV header generation for stereo

2. **Integration tests:**
   - Test with SF2 files containing known stereo samples
   - Verify output WAV files are valid stereo
   - Check channel count in output files

3. **Test files:**
   - Need SF2 files with verified stereo samples
   - Piano, strings, and pad sounds often have stereo samples

## Estimated Effort

- **Implementation:** 2-4 hours
- **Testing:** 1-2 hours
- **Total:** 3-6 hours

## Benefits

1. **Proper audio quality:** Stereo samples maintain their spatial characteristics
2. **Compatibility:** Many SF2 files have stereo samples; extracting as mono loses information
3. **Standard compliance:** Stereo WAV files are more widely compatible with audio software

## Risks

1. **Complexity:** Stereo handling adds complexity to the extraction logic
2. **Performance:** Interleaving samples requires additional processing
3. **Backwards compatibility:** Need to ensure mono samples still work correctly
