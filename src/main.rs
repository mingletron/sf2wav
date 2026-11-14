use rayon::prelude::*;
use std::collections::HashSet;
use std::fs::File;
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

#[derive(Debug)]
struct Sf2Error(String);

impl std::error::Error for Sf2Error {}

impl From<std::io::Error> for Sf2Error {
    fn from(err: std::io::Error) -> Self {
        Sf2Error(err.to_string())
    }
}

impl std::fmt::Display for Sf2Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Size of one `shdr` record in bytes.
const SHDR_RECORD_SIZE: usize = 46;

// sfSampleType values from the SoundFont 2 specification (section 7.10).
// These are distinct values, not a two-bit field.
const SAMPLE_TYPE_MONO: u16 = 1;
const SAMPLE_TYPE_RIGHT: u16 = 2;
const SAMPLE_TYPE_LEFT: u16 = 4;
/// Set when the sample data lives in the sound card's ROM, not in the file.
const SAMPLE_TYPE_ROM: u16 = 0x8000;

/// Used when a header claims a sample rate of zero, which the spec forbids.
const FALLBACK_SAMPLE_RATE: u32 = 44100;

#[derive(Debug, Clone)]
struct ChunkHeader {
    id: [u8; 4],
    size: u32,
}

#[derive(Debug, Clone)]
struct SampleHeader {
    name: String,
    start: u32,
    end: u32,
    start_loop: u32,
    end_loop: u32,
    sample_rate: u32,
    original_key: u8,
    correction: i8,
    sample_link: u16,
    sample_type: u16,
}

impl SampleHeader {
    /// The sample type without the ROM flag.
    fn base_type(&self) -> u16 {
        self.sample_type & !SAMPLE_TYPE_ROM
    }

    fn is_mono(&self) -> bool {
        self.base_type() == SAMPLE_TYPE_MONO
    }

    fn is_left_channel(&self) -> bool {
        self.base_type() == SAMPLE_TYPE_LEFT
    }

    fn is_right_channel(&self) -> bool {
        self.base_type() == SAMPLE_TYPE_RIGHT
    }

    /// ROM samples have no data in the file, so there is nothing to extract.
    fn is_rom(&self) -> bool {
        self.sample_type & SAMPLE_TYPE_ROM != 0
    }

    /// True if `start..end` is a non-empty range inside the sample data.
    fn has_valid_range(&self, total_samples: usize) -> bool {
        self.start < self.end && self.end as usize <= total_samples
    }

    fn effective_sample_rate(&self) -> u32 {
        if self.sample_rate == 0 {
            FALLBACK_SAMPLE_RATE
        } else {
            self.sample_rate
        }
    }

    /// Loop points for the WAV `smpl` chunk, relative to the sample start.
    ///
    /// In SF2, `end_loop` is the first sample *after* the loop (the one that
    /// is equivalent to `start_loop`); the `smpl` chunk wants the last sample
    /// that is played, so the end comes back one lower.
    fn loop_points(&self) -> Option<(u32, u32)> {
        if self.start <= self.start_loop
            && self.start_loop < self.end_loop
            && self.end_loop <= self.end
        {
            Some((self.start_loop - self.start, self.end_loop - self.start - 1))
        } else {
            None
        }
    }

    /// MIDI note the sample was recorded at. Values above 127 mean
    /// "unpitched" (255) or are invalid, and fall back to middle C.
    fn unity_note(&self) -> u32 {
        if self.original_key <= 127 {
            self.original_key as u32
        } else {
            60
        }
    }

    /// Parse the `shdr` chunk.
    ///
    /// Every record except the terminal one is kept, valid or not:
    /// `sample_link` is an index into this list, so dropping entries here
    /// would make every later link point at the wrong sample. Unusable
    /// entries are skipped at extraction time instead.
    fn load_from_bytes(data: &[u8]) -> Result<Vec<SampleHeader>, Sf2Error> {
        if !data.len().is_multiple_of(SHDR_RECORD_SIZE) {
            return Err(Sf2Error("Invalid sample header size".to_string()));
        }

        let mut headers = Vec::with_capacity(data.len() / SHDR_RECORD_SIZE);
        for chunk in data.chunks(SHDR_RECORD_SIZE) {
            // The name is NUL terminated; whatever follows the first NUL is
            // leftover junk in many files.
            let name_bytes = &chunk[0..20];
            let name_len = name_bytes.iter().position(|&b| b == 0).unwrap_or(20);
            let name = String::from_utf8_lossy(&name_bytes[..name_len])
                .trim_end()
                .to_string();
            let u32_at =
                |i: usize| u32::from_le_bytes([chunk[i], chunk[i + 1], chunk[i + 2], chunk[i + 3]]);

            headers.push(SampleHeader {
                name,
                start: u32_at(20),
                end: u32_at(24),
                start_loop: u32_at(28),
                end_loop: u32_at(32),
                sample_rate: u32_at(36),
                original_key: chunk[40],
                correction: chunk[41] as i8,
                sample_link: u16::from_le_bytes([chunk[42], chunk[43]]),
                sample_type: u16::from_le_bytes([chunk[44], chunk[45]]),
            });
        }

        // The last record is the terminal "EOS" entry.
        if headers
            .last()
            .is_some_and(|h| h.name == "EOS" || h.name.is_empty())
        {
            headers.pop();
        }

        Ok(headers)
    }
}

#[derive(Debug)]
struct StereoPair {
    left_idx: usize,
    right_idx: usize,
    name: String,
}

fn name_has_suffix(name: &str, suffixes: &[&str]) -> bool {
    let lower = name.to_lowercase();
    suffixes.iter().any(|s| lower.ends_with(s))
}

/// Find left/right sample pairs.
///
/// `sample_link` is the zero-based index of the other channel's header.
/// Left and right come from `sample_type`; names and index order are only
/// used when the type does not say.
fn find_stereo_pairs(headers: &[SampleHeader]) -> Vec<StereoPair> {
    let mut pairs = Vec::new();
    let mut processed = vec![false; headers.len()];

    for (i, header) in headers.iter().enumerate() {
        if processed[i] || header.is_mono() || header.is_rom() {
            continue;
        }

        // Files with a junk sample type can still be paired by a non-zero
        // link. A link of zero is only believed when the type says stereo.
        let typed = header.is_left_channel() || header.is_right_channel();
        if !typed && header.sample_link == 0 {
            continue;
        }

        let linked = header.sample_link as usize;
        if linked >= headers.len() || linked == i || processed[linked] {
            continue;
        }
        let other = &headers[linked];
        if other.is_mono() || other.is_rom() {
            continue;
        }

        let (left_idx, right_idx) = if header.is_left_channel() || other.is_right_channel() {
            (i, linked)
        } else if header.is_right_channel() || other.is_left_channel() {
            (linked, i)
        } else if name_has_suffix(&header.name, &["-l", "_l"]) {
            (i, linked)
        } else if name_has_suffix(&header.name, &["-r", "_r"]) {
            (linked, i)
        } else {
            (i.min(linked), i.max(linked))
        };

        pairs.push(StereoPair {
            left_idx,
            right_idx,
            name: strip_stereo_suffix(&headers[left_idx].name).to_string(),
        });

        processed[i] = true;
        processed[linked] = true;
    }

    pairs
}

struct Sf2Parser<R: Read + Seek> {
    reader: R,
    len: u64,
}

impl<R: Read + Seek> Sf2Parser<R> {
    fn new(mut reader: R) -> Result<Self, Sf2Error> {
        let len = reader.seek(SeekFrom::End(0))?;
        reader.seek(SeekFrom::Start(0))?;
        Ok(Sf2Parser { reader, len })
    }

    fn read_chunk_header(&mut self) -> Result<ChunkHeader, Sf2Error> {
        let mut id = [0u8; 4];
        let mut size_buf = [0u8; 4];

        self.reader.read_exact(&mut id)?;
        self.reader.read_exact(&mut size_buf)?;

        let size = u32::from_le_bytes(size_buf);
        Ok(ChunkHeader { id, size })
    }

    /// Read a chunk body from the current position. The size comes from the
    /// file, so it is checked against what is actually left before anything
    /// is allocated.
    fn read_chunk_data(&mut self, size: u32) -> Result<Vec<u8>, Sf2Error> {
        let pos = self.reader.stream_position()?;
        if pos + size as u64 > self.len {
            return Err(Sf2Error(
                "Corrupt file: chunk extends past the end of the file".to_string(),
            ));
        }
        let mut data = vec![0u8; size as usize];
        self.reader.read_exact(&mut data)?;
        Ok(data)
    }

    /// Returns the 16-bit sample data and the sample headers.
    fn find_sample_data(&mut self) -> Result<(Vec<i16>, Vec<SampleHeader>), Sf2Error> {
        let riff_header = self.read_chunk_header()?;
        if &riff_header.id != b"RIFF" {
            return Err(Sf2Error("Not a RIFF file".to_string()));
        }

        let mut form_type = [0u8; 4];
        self.reader.read_exact(&mut form_type)?;
        if &form_type != b"sfbk" {
            return Err(Sf2Error("Not a SoundFont file".to_string()));
        }

        let mut samples: Vec<i16> = Vec::new();
        let mut sample_headers = Vec::new();

        // Chunks are walked by absolute position, in u64, so a wrong size in
        // one chunk cannot overflow a counter or run past the file.
        let riff_end = (8 + riff_header.size as u64).min(self.len);
        let mut pos = 12u64;

        while pos + 8 <= riff_end {
            self.reader.seek(SeekFrom::Start(pos))?;
            let chunk = self.read_chunk_header()?;
            let body = pos + 8;
            pos = body + chunk.size as u64 + (chunk.size & 1) as u64;

            if &chunk.id != b"LIST" || chunk.size < 4 {
                continue;
            }
            let mut list_type = [0u8; 4];
            self.reader.read_exact(&mut list_type)?;
            let wanted: &[u8; 4] = match &list_type {
                b"sdta" => b"smpl",
                b"pdta" => b"shdr",
                _ => continue,
            };

            let list_end = (body + chunk.size as u64).min(riff_end);
            let mut sub_pos = body + 4;
            while sub_pos + 8 <= list_end {
                self.reader.seek(SeekFrom::Start(sub_pos))?;
                let sub = self.read_chunk_header()?;
                sub_pos += 8 + sub.size as u64 + (sub.size & 1) as u64;

                if &sub.id != wanted {
                    continue;
                }
                let data = self.read_chunk_data(sub.size)?;
                if wanted == b"smpl" {
                    samples = data
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|&b| i16::from_le_bytes(b))
                        .collect();
                } else {
                    sample_headers = SampleHeader::load_from_bytes(&data)?;
                }
            }
        }

        if samples.is_empty() {
            return Err(Sf2Error("No sample data found".to_string()));
        }

        if sample_headers.is_empty() {
            return Err(Sf2Error("No sample headers found".to_string()));
        }

        Ok((samples, sample_headers))
    }
}

/// Loop and pitch information for the WAV `smpl` chunk.
#[derive(Debug, Clone, Copy)]
struct LoopInfo {
    /// First sample of the loop, in frames from the start of the file.
    start: u32,
    /// Last sample of the loop (inclusive), in frames.
    end: u32,
    /// MIDI note the sample plays at its recorded pitch.
    unity_note: u32,
}

/// `smpl` chunk body: 36 bytes of fields plus one 24 byte loop.
const SMPL_BODY_SIZE: u32 = 60;

fn write_wav<W: Write>(
    writer: &mut W,
    sample_data: &[i16],
    sample_rate: u32,
    channels: u16,
    loop_info: Option<LoopInfo>,
) -> Result<(), Sf2Error> {
    let block_align = channels * 2; // 2 bytes per sample * channels
    let byte_rate = sample_rate.saturating_mul(block_align as u32);

    // The smpl chunk takes its body plus an 8 byte chunk header.
    let smpl_total = if loop_info.is_some() {
        8 + SMPL_BODY_SIZE as u64
    } else {
        0
    };
    let data_size = sample_data.len() as u64 * 2;
    let riff_size = 36 + data_size + smpl_total;
    if riff_size > u32::MAX as u64 {
        return Err(Sf2Error("Sample is too large for a WAV file".to_string()));
    }

    // RIFF header
    writer.write_all(b"RIFF")?;
    writer.write_all(&(riff_size as u32).to_le_bytes())?;
    writer.write_all(b"WAVE")?;

    // fmt chunk
    writer.write_all(b"fmt ")?;
    writer.write_all(&16u32.to_le_bytes())?; // chunk size
    writer.write_all(&1u16.to_le_bytes())?; // audio format (PCM)
    writer.write_all(&channels.to_le_bytes())?;
    writer.write_all(&sample_rate.to_le_bytes())?;
    writer.write_all(&byte_rate.to_le_bytes())?;
    writer.write_all(&block_align.to_le_bytes())?;
    writer.write_all(&16u16.to_le_bytes())?; // bits per sample

    // data chunk
    writer.write_all(b"data")?;
    writer.write_all(&(data_size as u32).to_le_bytes())?;
    for &sample in sample_data {
        writer.write_all(&sample.to_le_bytes())?;
    }

    if let Some(info) = loop_info {
        write_smpl_chunk(writer, sample_rate, info)?;
    }

    Ok(())
}

fn write_smpl_chunk<W: Write>(
    writer: &mut W,
    sample_rate: u32,
    info: LoopInfo,
) -> Result<(), Sf2Error> {
    writer.write_all(b"smpl")?;
    writer.write_all(&SMPL_BODY_SIZE.to_le_bytes())?;

    // Manufacturer (0 = unknown)
    writer.write_all(&0u32.to_le_bytes())?;
    // Product (0 = unknown)
    writer.write_all(&0u32.to_le_bytes())?;
    // Sample period in nanoseconds, rounded
    let rate = sample_rate.max(1) as u64;
    let sample_period = ((1_000_000_000u64 + rate / 2) / rate) as u32;
    writer.write_all(&sample_period.to_le_bytes())?;
    // MIDI unity note
    writer.write_all(&info.unity_note.to_le_bytes())?;
    // MIDI pitch fraction (0)
    writer.write_all(&0u32.to_le_bytes())?;
    // SMPTE format (0)
    writer.write_all(&0u32.to_le_bytes())?;
    // SMPTE offset (0)
    writer.write_all(&0u32.to_le_bytes())?;
    // Number of sample loops (1)
    writer.write_all(&1u32.to_le_bytes())?;
    // Sampler data (0)
    writer.write_all(&0u32.to_le_bytes())?;

    // Loop structure (24 bytes)
    // Cue point ID (0)
    writer.write_all(&0u32.to_le_bytes())?;
    // Type (0 = forward loop)
    writer.write_all(&0u32.to_le_bytes())?;
    // Start: first sample of the loop
    writer.write_all(&info.start.to_le_bytes())?;
    // End: last sample of the loop, inclusive
    writer.write_all(&info.end.to_le_bytes())?;
    // Fraction (0)
    writer.write_all(&0u32.to_le_bytes())?;
    // Play count (0 = infinite)
    writer.write_all(&0u32.to_le_bytes())?;

    Ok(())
}

/// Write a WAV file through a buffer: `write_wav` emits one small write per
/// sample, which is a system call each on a bare `File`.
fn write_wav_file(
    path: &Path,
    sample_data: &[i16],
    sample_rate: u32,
    channels: u16,
    loop_info: Option<LoopInfo>,
) -> Result<(), Sf2Error> {
    let mut writer = BufWriter::new(File::create(path)?);
    write_wav(&mut writer, sample_data, sample_rate, channels, loop_info)?;
    writer.flush()?;
    Ok(())
}

#[derive(Debug, Clone)]
struct SampleMetadata {
    sf2_filename: String,
    name: String,
    filename: String,
    sample_rate: u32,
    channels: u16,
    num_samples: usize,
    duration_seconds: f64,
    start: u32,
    end: u32,
    start_loop: u32,
    end_loop: u32,
    has_loop: bool,
    original_key: u8,
    correction: i8,
    sample_type: String,
    is_stereo_pair: bool,
    linked_sample: Option<String>,
}

/// Quote a CSV field, doubling any quotes inside it.
fn csv_quote(field: &str) -> String {
    format!("\"{}\"", field.replace('"', "\"\""))
}

fn write_csv<W: Write>(writer: &mut W, metadata: &[SampleMetadata]) -> Result<(), Sf2Error> {
    // Write CSV header
    writeln!(
        writer,
        "SF2 File,Name,Filename,Sample Rate,Channels,Num Samples,Duration (seconds),Start,End,Start Loop,End Loop,Has Loop,Original Key,Correction,Sample Type,Is Stereo Pair,Linked Sample"
    )?;

    // Write CSV data rows
    for m in metadata {
        writeln!(
            writer,
            "{},{},{},{},{},\"{}\",{:.3},{},{},{},{},{},\"{}\",{},{},{},{}",
            csv_quote(&m.sf2_filename),
            csv_quote(&m.name),
            csv_quote(&m.filename),
            m.sample_rate,
            m.channels,
            m.num_samples,
            m.duration_seconds,
            m.start,
            m.end,
            m.start_loop,
            m.end_loop,
            m.has_loop,
            m.original_key,
            m.correction,
            csv_quote(&m.sample_type),
            m.is_stereo_pair,
            csv_quote(m.linked_sample.as_deref().unwrap_or(""))
        )?;
    }

    Ok(())
}

/// Remove a trailing `-L`/`-R`/`_L`/`_R` (any case). Only used for the name
/// of a stereo pair: a mono sample keeps its suffix, or unlinked `Foo-L` and
/// `Foo-R` would both become `Foo`.
fn strip_stereo_suffix(name: &str) -> &str {
    if name_has_suffix(name, &["-l", "-r", "_l", "_r"]) {
        &name[..name.len() - 2]
    } else {
        name
    }
}

/// Make a sample name safe to use as a file name (without extension).
fn sanitize_filename(name: &str) -> String {
    let mut result: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();

    // Remove .wav extension if present (will be added later)
    if result.to_lowercase().ends_with(".wav") {
        result.truncate(result.len() - 4);
    }

    // Windows refuses names that end in a space or a dot
    result.trim_end_matches([' ', '.']).to_string()
}

/// Hands out WAV file names that are unique within one output folder, so
/// two samples with the same name never overwrite each other.
#[derive(Default)]
struct FileNamer {
    /// Lower-cased names already used: macOS and Windows file systems are
    /// usually case-insensitive.
    used: HashSet<String>,
}

impl FileNamer {
    /// `base` is the name without extension; `index` names a sample that
    /// has no usable name of its own.
    fn unique_wav_name(&mut self, base: &str, index: usize) -> String {
        let base = if base.is_empty() {
            format!("sample_{:03}", index)
        } else {
            base.to_string()
        };
        let mut candidate = base.clone();
        let mut n = 2;
        while !self.used.insert(candidate.to_lowercase()) {
            candidate = format!("{} ({})", base, n);
            n += 1;
        }
        format!("{}.wav", candidate)
    }
}

fn extract_samples(
    sf2_path: &Path,
    output_dir: &Path,
    sf2_filename: String,
) -> Result<(Vec<String>, Vec<SampleMetadata>), Sf2Error> {
    let file = File::open(sf2_path)?;
    let mut parser = Sf2Parser::new(file)?;

    let (all_samples, sample_headers) = parser.find_sample_data()?;
    let total = all_samples.len();

    // Ensure output directory exists
    std::fs::create_dir_all(output_dir)?;

    let mut extracted_files = Vec::new();
    let mut metadata_list = Vec::new();
    let mut namer = FileNamer::default();
    let mut processed_indices = HashSet::new();

    // Extract stereo pairs
    for pair in &find_stereo_pairs(&sample_headers) {
        let left_header = &sample_headers[pair.left_idx];
        let right_header = &sample_headers[pair.right_idx];

        if !left_header.has_valid_range(total) || !right_header.has_valid_range(total) {
            // Leave both unprocessed: whichever half is usable is still
            // extracted as a mono sample below.
            eprintln!(
                "Warning: Invalid sample range in stereo pair '{}', extracting channels separately",
                pair.name
            );
            continue;
        }
        processed_indices.insert(pair.left_idx);
        processed_indices.insert(pair.right_idx);

        let left_samples = &all_samples[left_header.start as usize..left_header.end as usize];
        let right_samples = &all_samples[right_header.start as usize..right_header.end as usize];

        // Use the shorter length to ensure both channels have same number of samples
        let num_frames = left_samples.len().min(right_samples.len());

        // Interleave samples: LRLRLR...
        let mut stereo_samples = Vec::with_capacity(num_frames * 2);
        for (&l, &r) in left_samples.iter().zip(right_samples) {
            stereo_samples.push(l);
            stereo_samples.push(r);
        }

        let filename = namer.unique_wav_name(&sanitize_filename(&pair.name), pair.left_idx);
        let sample_rate = left_header.effective_sample_rate();

        // Use the left channel's loop points, if they fit the shorter channel
        let loop_info = left_header
            .loop_points()
            .filter(|&(_, end)| (end as usize) < num_frames)
            .map(|(start, end)| LoopInfo {
                start,
                end,
                unity_note: left_header.unity_note(),
            });
        write_wav_file(
            &output_dir.join(&filename),
            &stereo_samples,
            sample_rate,
            2,
            loop_info,
        )?;

        extracted_files.push(filename.clone());
        println!(
            "Extracted (stereo): {} ({} Hz, {} frames)",
            filename, sample_rate, num_frames
        );

        metadata_list.push(SampleMetadata {
            sf2_filename: sf2_filename.clone(),
            name: left_header.name.clone(),
            filename,
            sample_rate,
            channels: 2,
            num_samples: stereo_samples.len(),
            duration_seconds: num_frames as f64 / sample_rate as f64,
            start: left_header.start,
            end: left_header.end,
            start_loop: left_header.start_loop,
            end_loop: left_header.end_loop,
            has_loop: loop_info.is_some(),
            original_key: left_header.original_key,
            correction: left_header.correction,
            sample_type: format!("0x{:04X}", left_header.sample_type),
            is_stereo_pair: true,
            linked_sample: Some(right_header.name.clone()),
        });
    }

    // Extract remaining samples as mono
    for (i, header) in sample_headers.iter().enumerate() {
        if processed_indices.contains(&i) {
            continue;
        }

        if header.is_rom() {
            println!("Skipped ROM sample: {}", header.name);
            continue;
        }

        if !header.has_valid_range(total) {
            // Unnamed empty records are padding, not worth a warning
            if !(header.name.is_empty() && header.start >= header.end) {
                eprintln!(
                    "Warning: Invalid sample range for '{}': {}-{}",
                    header.name, header.start, header.end
                );
            }
            continue;
        }

        let sample_slice = &all_samples[header.start as usize..header.end as usize];
        let filename = namer.unique_wav_name(&sanitize_filename(&header.name), i);
        let sample_rate = header.effective_sample_rate();
        if header.sample_rate == 0 {
            eprintln!(
                "Warning: '{}' has a sample rate of 0, using {} Hz",
                header.name, sample_rate
            );
        }

        let loop_info = header.loop_points().map(|(start, end)| LoopInfo {
            start,
            end,
            unity_note: header.unity_note(),
        });
        write_wav_file(
            &output_dir.join(&filename),
            sample_slice,
            sample_rate,
            1,
            loop_info,
        )?;

        extracted_files.push(filename.clone());
        println!(
            "Extracted (mono): {} ({} Hz, {} samples)",
            filename,
            sample_rate,
            sample_slice.len()
        );

        metadata_list.push(SampleMetadata {
            sf2_filename: sf2_filename.clone(),
            name: header.name.clone(),
            filename,
            sample_rate,
            channels: 1,
            num_samples: sample_slice.len(),
            duration_seconds: sample_slice.len() as f64 / sample_rate as f64,
            start: header.start,
            end: header.end,
            start_loop: header.start_loop,
            end_loop: header.end_loop,
            has_loop: loop_info.is_some(),
            original_key: header.original_key,
            correction: header.correction,
            sample_type: format!("0x{:04X}", header.sample_type),
            is_stereo_pair: false,
            linked_sample: None,
        });
    }

    Ok((extracted_files, metadata_list))
}

fn process_sf2_file(
    sf2_path: &Path,
    output_base: &Path,
) -> Result<(String, usize, Vec<SampleMetadata>), Sf2Error> {
    let file_name = sf2_path
        .file_stem()
        .ok_or_else(|| Sf2Error("Invalid filename".to_string()))?
        .to_string_lossy()
        .to_string();

    // Get the full filename with extension (e.g., "Anvil11.sf2")
    let sf2_filename = sf2_path
        .file_name()
        .ok_or_else(|| Sf2Error("Invalid filename".to_string()))?
        .to_string_lossy()
        .to_string();

    // Create a subdirectory for this SF2 file's samples
    let output_dir = output_base.join(&file_name);
    std::fs::create_dir_all(&output_dir)?;

    println!("Processing: {}", sf2_path.display());
    let (files, metadata) = extract_samples(sf2_path, &output_dir, sf2_filename)?;

    Ok((file_name, files.len(), metadata))
}

fn has_sf2_extension(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("sf2"))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();

    if args.len() < 2 {
        eprintln!("Usage: {} <input> [output_directory]", args[0]);
        eprintln!("  input: Path to a .sf2 file or directory containing .sf2 files");
        eprintln!("  output_directory: Directory for extracted samples (default: ./output)");
        eprintln!();
        eprintln!("Examples:");
        eprintln!(
            "  {} soundfont.sf2              # Extract to ./output/soundfont/",
            args[0]
        );
        eprintln!(
            "  {} ./sf2_files/               # Batch convert all .sf2 files",
            args[0]
        );
        eprintln!(
            "  {} ./sf2_files/ ./extracted/  # Specify output directory",
            args[0]
        );
        std::process::exit(1);
    }

    let input_path = Path::new(&args[1]);
    let output_base = if args.len() >= 3 {
        Path::new(&args[2]).to_path_buf()
    } else {
        std::env::current_dir()?.join("output")
    };

    if !input_path.exists() {
        eprintln!("Error: Path not found: {}", input_path.display());
        std::process::exit(1);
    }

    let mut sf2_files: Vec<PathBuf> = Vec::new();

    // Check if input is a directory or a file
    if input_path.is_dir() {
        // Collect all .sf2 files in the directory
        for entry in std::fs::read_dir(input_path)? {
            let path = entry?.path();
            if path.is_file() && has_sf2_extension(&path) {
                sf2_files.push(path);
            }
        }
        sf2_files.sort();
    } else if input_path.is_file() && has_sf2_extension(input_path) {
        sf2_files.push(input_path.to_path_buf());
    } else {
        eprintln!("Error: Input must be a .sf2 file or a directory containing .sf2 files");
        std::process::exit(1);
    }

    if sf2_files.is_empty() {
        eprintln!("Error: No .sf2 files found in {}", input_path.display());
        std::process::exit(1);
    }

    // Ensure output base directory exists
    std::fs::create_dir_all(&output_base)?;

    println!("Found {} .sf2 file(s) to process", sf2_files.len());
    println!("Output directory: {}", output_base.display());
    println!(
        "Using {} threads for parallel processing",
        rayon::current_num_threads()
    );
    println!();

    // Process files in parallel
    let results: Vec<_> = sf2_files
        .par_iter()
        .map(|sf2_path| process_sf2_file(sf2_path, &output_base))
        .collect();

    // Aggregate results
    let mut total_samples = 0;
    let mut successful = 0;
    let mut failed = 0;
    let mut all_metadata: Vec<SampleMetadata> = Vec::new();

    for (sf2_path, result) in sf2_files.iter().zip(results) {
        match result {
            Ok((name, count, metadata)) => {
                println!("  ✓ {}: {} samples extracted", name, count);
                total_samples += count;
                successful += 1;
                all_metadata.extend(metadata);
            }
            Err(e) => {
                eprintln!("  ✗ {}: {}", sf2_path.display(), e);
                failed += 1;
            }
        }
    }

    // Write consolidated CSV file for all samples
    if !all_metadata.is_empty() {
        let csv_path = output_base.join("samples.csv");
        let mut csv_file = BufWriter::new(File::create(&csv_path)?);
        write_csv(&mut csv_file, &all_metadata)?;
        csv_file.flush()?;
        println!("  CSV metadata written to: {}", csv_path.display());
    }

    println!("========================================");
    println!("Batch conversion complete!");
    println!("  Successful: {} files", successful);
    if failed > 0 {
        println!("  Failed: {} files", failed);
    }
    println!("  Total samples extracted: {}", total_samples);
    println!("  Output directory: {}", output_base.display());

    // Scripts need to be able to tell that something went wrong
    if failed > 0 {
        std::process::exit(1);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn header(name: &str, start: u32, end: u32, link: u16, sample_type: u16) -> SampleHeader {
        SampleHeader {
            name: name.to_string(),
            start,
            end,
            start_loop: 0,
            end_loop: 0,
            sample_rate: 44100,
            original_key: 60,
            correction: 0,
            sample_link: link,
            sample_type,
        }
    }

    fn shdr_record(h: &SampleHeader) -> Vec<u8> {
        let mut rec = vec![0u8; SHDR_RECORD_SIZE];
        rec[..h.name.len()].copy_from_slice(h.name.as_bytes());
        rec[20..24].copy_from_slice(&h.start.to_le_bytes());
        rec[24..28].copy_from_slice(&h.end.to_le_bytes());
        rec[28..32].copy_from_slice(&h.start_loop.to_le_bytes());
        rec[32..36].copy_from_slice(&h.end_loop.to_le_bytes());
        rec[36..40].copy_from_slice(&h.sample_rate.to_le_bytes());
        rec[40] = h.original_key;
        rec[41] = h.correction as u8;
        rec[42..44].copy_from_slice(&h.sample_link.to_le_bytes());
        rec[44..46].copy_from_slice(&h.sample_type.to_le_bytes());
        rec
    }

    fn chunk(id: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut out = id.to_vec();
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(body);
        if body.len() % 2 == 1 {
            out.push(0);
        }
        out
    }

    /// A minimal but well-formed SF2 file.
    fn build_sf2(pcm: &[i16], headers: &[SampleHeader]) -> Vec<u8> {
        let pcm_bytes: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
        let mut shdr: Vec<u8> = headers.iter().flat_map(shdr_record).collect();
        shdr.extend(shdr_record(&header("EOS", 0, 0, 0, 0)));

        let mut body = b"sfbk".to_vec();
        let info = [&b"INFO"[..], &chunk(b"ifil", &[2, 0, 1, 0])].concat();
        body.extend(chunk(b"LIST", &info));
        let sdta = [&b"sdta"[..], &chunk(b"smpl", &pcm_bytes)].concat();
        body.extend(chunk(b"LIST", &sdta));
        let pdta = [&b"pdta"[..], &chunk(b"shdr", &shdr)].concat();
        body.extend(chunk(b"LIST", &pdta));
        chunk(b"RIFF", &body)
    }

    /// Four 100-sample blocks, each a constant, so a WAV's first frame says
    /// which blocks it was made from.
    fn block_pcm() -> Vec<i16> {
        let mut pcm = Vec::new();
        for value in [1000i16, 2000, 3000, 4000] {
            pcm.extend(std::iter::repeat_n(value, 100));
        }
        pcm
    }

    struct Wav {
        riff_size: u32,
        file_len: usize,
        channels: u16,
        sample_rate: u32,
        samples: Vec<i16>,
        /// (unity note, loop start, loop end)
        smpl: Option<(u32, u32, u32)>,
    }

    fn u32_at(b: &[u8], i: usize) -> u32 {
        u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
    }

    fn parse_wav(b: &[u8]) -> Wav {
        assert_eq!(&b[0..4], b"RIFF");
        assert_eq!(&b[8..12], b"WAVE");
        assert_eq!(&b[12..16], b"fmt ");
        assert_eq!(&b[36..40], b"data");
        let data_size = u32_at(b, 40) as usize;
        let samples = b[44..44 + data_size]
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect();
        let rest = &b[44 + data_size..];
        let smpl = if rest.is_empty() {
            None
        } else {
            assert_eq!(&rest[0..4], b"smpl");
            assert_eq!(u32_at(rest, 4), 60);
            assert_eq!(rest.len(), 68, "smpl chunk is 8 + 60 bytes");
            assert_eq!(u32_at(rest, 8 + 28), 1, "one loop");
            Some((
                u32_at(rest, 8 + 12),
                u32_at(rest, 8 + 44),
                u32_at(rest, 8 + 48),
            ))
        };
        Wav {
            riff_size: u32_at(b, 4),
            file_len: b.len(),
            channels: u16::from_le_bytes([b[22], b[23]]),
            sample_rate: u32_at(b, 24),
            samples,
            smpl,
        }
    }

    /// Run the whole extraction on an in-memory SF2 and return the output
    /// directory plus the result.
    fn extract(
        test_name: &str,
        pcm: &[i16],
        headers: &[SampleHeader],
    ) -> (PathBuf, Vec<String>, Vec<SampleMetadata>) {
        let dir =
            std::env::temp_dir().join(format!("sf2wav_test_{}_{}", std::process::id(), test_name));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let sf2_path = dir.join("test.sf2");
        std::fs::write(&sf2_path, build_sf2(pcm, headers)).unwrap();
        let out = dir.join("out");
        let (files, metadata) = extract_samples(&sf2_path, &out, "test.sf2".to_string()).unwrap();
        (out, files, metadata)
    }

    fn read_wav(dir: &Path, name: &str) -> Wav {
        parse_wav(&std::fs::read(dir.join(name)).unwrap())
    }

    #[test]
    fn test_sanitize_filename() {
        assert_eq!(sanitize_filename("normal_name"), "normal_name");
        assert_eq!(sanitize_filename("name with spaces"), "name with spaces");
        assert_eq!(sanitize_filename("name/with/slashes"), "name_with_slashes");
        assert_eq!(
            sanitize_filename("name\\with\\backslashes"),
            "name_with_backslashes"
        );
        assert_eq!(sanitize_filename("name:with:colons"), "name_with_colons");
        assert_eq!(
            sanitize_filename("name*with*asterisks"),
            "name_with_asterisks"
        );
        assert_eq!(
            sanitize_filename("name?with?questions"),
            "name_with_questions"
        );
        assert_eq!(sanitize_filename("name\"with\"quotes"), "name_with_quotes");
        assert_eq!(
            sanitize_filename("name<with>brackets"),
            "name_with_brackets"
        );
        assert_eq!(sanitize_filename("name|with|pipe"), "name_with_pipe");
        assert_eq!(sanitize_filename("file.wav"), "file");
        assert_eq!(sanitize_filename("file.WAV"), "file");
        assert_eq!(sanitize_filename("file.wav.wav"), "file.wav");
        assert_eq!(sanitize_filename(".wav"), "");
        assert_eq!(sanitize_filename("test.wav.txt"), "test.wav.txt"); // Only removes .wav at end
        assert_eq!(sanitize_filename("trailing dot. "), "trailing dot");
    }

    #[test]
    fn test_sanitize_filename_keeps_stereo_suffix() {
        // A mono sample keeps its suffix: only a real pair loses it
        assert_eq!(sanitize_filename("Test-L"), "Test-L");
        assert_eq!(sanitize_filename("Test_r"), "Test_r");
    }

    #[test]
    fn test_strip_stereo_suffix() {
        assert_eq!(strip_stereo_suffix("Test-L"), "Test");
        assert_eq!(strip_stereo_suffix("Test-R"), "Test");
        assert_eq!(strip_stereo_suffix("Test_l"), "Test");
        assert_eq!(strip_stereo_suffix("Test_r"), "Test");
        assert_eq!(strip_stereo_suffix("Test"), "Test");
        assert_eq!(strip_stereo_suffix("Cello"), "Cello");
        assert_eq!(sanitize_filename(strip_stereo_suffix("Test.wav-L")), "Test");
    }

    #[test]
    fn test_file_namer_never_repeats() {
        let mut namer = FileNamer::default();
        assert_eq!(namer.unique_wav_name("Piano", 0), "Piano.wav");
        assert_eq!(namer.unique_wav_name("Piano", 1), "Piano (2).wav");
        assert_eq!(namer.unique_wav_name("piano", 2), "piano (3).wav");
        assert_eq!(namer.unique_wav_name("", 7), "sample_007.wav");
        assert_eq!(namer.unique_wav_name("", 7), "sample_007 (2).wav");
    }

    #[test]
    fn test_sample_header_load_from_bytes() {
        let mut h = header("TestSample", 100, 500, 3, SAMPLE_TYPE_LEFT);
        h.start_loop = 150;
        h.end_loop = 450;
        h.original_key = 64;
        h.correction = -5;

        let headers = SampleHeader::load_from_bytes(&shdr_record(&h)).unwrap();
        assert_eq!(headers.len(), 1);
        assert_eq!(headers[0].name, "TestSample");
        assert_eq!(headers[0].start, 100);
        assert_eq!(headers[0].end, 500);
        assert_eq!(headers[0].start_loop, 150);
        assert_eq!(headers[0].end_loop, 450);
        assert_eq!(headers[0].sample_rate, 44100);
        assert_eq!(headers[0].original_key, 64);
        assert_eq!(headers[0].correction, -5);
        assert_eq!(headers[0].sample_link, 3);
        assert_eq!(headers[0].sample_type, SAMPLE_TYPE_LEFT);
    }

    #[test]
    fn test_sample_header_drops_only_the_terminal_record() {
        for terminal in ["EOS", ""] {
            let mut data = shdr_record(&header("ValidSample", 100, 500, 0, 1));
            data.extend(shdr_record(&header(terminal, 0, 0, 0, 0)));
            let headers = SampleHeader::load_from_bytes(&data).unwrap();
            assert_eq!(headers.len(), 1);
            assert_eq!(headers[0].name, "ValidSample");
        }
    }

    #[test]
    fn test_sample_header_keeps_indices_stable() {
        // An empty-named or zero-length record in the middle must stay, or
        // every sample_link after it points at the wrong header
        let mut data = shdr_record(&header("", 0, 100, 0, 1));
        data.extend(shdr_record(&header("Empty", 50, 50, 0, 1)));
        data.extend(shdr_record(&header("Last", 100, 200, 0, 1)));
        data.extend(shdr_record(&header("EOS", 0, 0, 0, 0)));
        let headers = SampleHeader::load_from_bytes(&data).unwrap();
        assert_eq!(headers.len(), 3);
        assert_eq!(headers[2].name, "Last");
    }

    #[test]
    fn test_sample_header_name_stops_at_first_nul() {
        let mut rec = shdr_record(&header("Kick", 0, 100, 0, 1));
        rec[5..10].copy_from_slice(b"junk!");
        let headers = SampleHeader::load_from_bytes(&rec).unwrap();
        assert_eq!(headers[0].name, "Kick");
    }

    #[test]
    fn test_invalid_sample_header_size() {
        let data = vec![0u8; 45];
        assert!(SampleHeader::load_from_bytes(&data).is_err());
    }

    #[test]
    fn test_sample_type_decoding() {
        assert!(header("m", 0, 1, 0, 1).is_mono());
        assert!(header("r", 0, 1, 0, 2).is_right_channel());
        assert!(header("l", 0, 1, 0, 4).is_left_channel());
        assert!(!header("l", 0, 1, 0, 4).is_right_channel());
        assert!(!header("r", 0, 1, 0, 2).is_left_channel());
        let rom_left = header("rl", 0, 1, 0, 0x8004);
        assert!(rom_left.is_rom() && rom_left.is_left_channel());
        assert!(!header("l", 0, 1, 0, 4).is_rom());
    }

    #[test]
    fn test_loop_points() {
        let mut h = header("Loop", 100, 500, 0, 1);
        assert_eq!(h.loop_points(), None, "0..0 is outside the sample");

        h.start_loop = 150;
        h.end_loop = 450;
        // end_loop is one past the loop in SF2, inclusive in smpl
        assert_eq!(h.loop_points(), Some((50, 349)));

        h.start_loop = 100;
        h.end_loop = 500;
        assert_eq!(h.loop_points(), Some((0, 399)), "whole sample loops");

        h.end_loop = 501;
        assert_eq!(h.loop_points(), None, "runs past the end");
        h.start_loop = 99;
        h.end_loop = 450;
        assert_eq!(h.loop_points(), None, "starts before the sample");
        h.start_loop = 300;
        h.end_loop = 300;
        assert_eq!(h.loop_points(), None, "empty loop");

        // The very first sample in a file may loop from offset 0
        let mut first = header("First", 0, 100, 0, 1);
        first.end_loop = 100;
        assert_eq!(first.loop_points(), Some((0, 99)));
    }

    #[test]
    fn test_find_stereo_pairs() {
        // sample_link is a zero-based index, and links go both ways
        let headers = vec![
            header("Test-L", 0, 100, 1, 4),
            header("Test-R", 100, 200, 0, 2),
        ];
        let pairs = find_stereo_pairs(&headers);
        assert_eq!(pairs.len(), 1, "Should find 1 stereo pair");
        assert_eq!(pairs[0].left_idx, 0);
        assert_eq!(pairs[0].right_idx, 1);
        assert_eq!(pairs[0].name, "Test");
    }

    #[test]
    fn test_find_stereo_pairs_right_stored_first() {
        // No hint in the names: the sample type decides
        let headers = vec![header("PadA", 0, 100, 1, 2), header("PadB", 100, 200, 0, 4)];
        let pairs = find_stereo_pairs(&headers);
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].left_idx, 1, "type 4 is the left channel");
        assert_eq!(pairs[0].right_idx, 0, "type 2 is the right channel");
        assert_eq!(pairs[0].name, "PadB");
    }

    #[test]
    fn test_find_stereo_pairs_not_adjacent() {
        let headers = vec![
            header("A-L", 0, 100, 2, 4),
            header("Solo", 100, 200, 0, 1),
            header("A-R", 200, 300, 0, 2),
        ];
        let pairs = find_stereo_pairs(&headers);
        assert_eq!(pairs.len(), 1);
        assert_eq!((pairs[0].left_idx, pairs[0].right_idx), (0, 2));
    }

    #[test]
    fn test_find_stereo_pairs_falls_back_to_names() {
        // Junk sample types, but a link and names to go on
        let headers = vec![
            header("Pad_R", 0, 100, 1, 0),
            header("Pad_L", 100, 200, 0, 0),
        ];
        let pairs = find_stereo_pairs(&headers);
        assert_eq!(pairs.len(), 1);
        assert_eq!((pairs[0].left_idx, pairs[0].right_idx), (1, 0));
    }

    #[test]
    fn test_find_stereo_pairs_ignores_mono_and_bad_links() {
        // Mono samples are never paired, whatever their link says
        let mono = vec![
            header("Mono1", 0, 100, 1, 1),
            header("Mono2", 100, 200, 0, 1),
        ];
        assert!(find_stereo_pairs(&mono).is_empty());
        // No type and no link
        let untyped = vec![header("A", 0, 100, 0, 0), header("B", 100, 200, 0, 0)];
        assert!(find_stereo_pairs(&untyped).is_empty());
        // Link out of range, link to self, link to a mono sample
        let bad = vec![
            header("X-L", 0, 100, 9, 4),
            header("Y-L", 100, 200, 1, 4),
            header("Z-L", 200, 300, 3, 4),
            header("Mono", 300, 400, 0, 1),
        ];
        assert!(find_stereo_pairs(&bad).is_empty());
    }

    #[test]
    fn test_write_wav() {
        let samples: Vec<i16> = vec![0, 1000, -1000, 32767, -32768];
        let mut output = Vec::new();
        write_wav(&mut output, &samples, 44100, 1, None).unwrap();

        let wav = parse_wav(&output);
        assert_eq!(wav.channels, 1);
        assert_eq!(wav.sample_rate, 44100);
        assert_eq!(wav.samples, samples);
        assert_eq!(wav.riff_size as usize, wav.file_len - 8);
        assert!(wav.smpl.is_none());
        // Audio format PCM, 16 bits
        assert_eq!(u16::from_le_bytes([output[20], output[21]]), 1);
        assert_eq!(u16::from_le_bytes([output[34], output[35]]), 16);
    }

    #[test]
    fn test_write_wav_empty_samples() {
        let mut output = Vec::new();
        write_wav(&mut output, &[], 44100, 1, None).unwrap();
        let wav = parse_wav(&output);
        assert!(wav.samples.is_empty());
        assert_eq!(wav.riff_size as usize, wav.file_len - 8);
    }

    #[test]
    fn test_write_stereo_wav() {
        let samples: Vec<i16> = vec![100, 200, 300, 400, 500, 600];
        let mut output = Vec::new();
        write_wav(&mut output, &samples, 48000, 2, None).unwrap();

        let wav = parse_wav(&output);
        assert_eq!(wav.channels, 2);
        assert_eq!(wav.samples, samples);
        assert!(wav.smpl.is_none(), "no smpl chunk without loop points");
        // byte rate and block align
        assert_eq!(u32_at(&output, 28), 48000 * 4);
        assert_eq!(u16::from_le_bytes([output[32], output[33]]), 4);
    }

    #[test]
    fn test_write_wav_with_loop_points() {
        let samples: Vec<i16> = vec![0, 1000, 2000, 3000, 4000, 5000];
        let mut output = Vec::new();
        let info = LoopInfo {
            start: 1,
            end: 4,
            unity_note: 72,
        };
        write_wav(&mut output, &samples, 44100, 1, Some(info)).unwrap();

        let wav = parse_wav(&output);
        assert_eq!(wav.smpl, Some((72, 1, 4)));
        // The RIFF size must cover the smpl chunk and its header
        assert_eq!(wav.riff_size as usize, wav.file_len - 8);
        // 1e9 / 44100 = 22675.7, rounded
        let smpl = output.len() - 68;
        assert_eq!(u32_at(&output, smpl + 8 + 8), 22676);
    }

    #[test]
    fn test_csv_quotes_are_escaped() {
        assert_eq!(csv_quote("plain"), "\"plain\"");
        assert_eq!(csv_quote("Say \"hi\""), "\"Say \"\"hi\"\"\"");
    }

    #[test]
    fn test_parser_rejects_non_sf2() {
        let mut parser = Sf2Parser::new(Cursor::new(b"RIFF\x04\0\0\0junk".to_vec())).unwrap();
        assert!(parser.find_sample_data().is_err());
        let mut parser = Sf2Parser::new(Cursor::new(b"nope".to_vec())).unwrap();
        assert!(parser.find_sample_data().is_err());
    }

    #[test]
    fn test_parser_rejects_chunk_past_end_of_file() {
        // A smpl chunk that claims 4 GB must fail cleanly, not allocate it
        let mut body = b"sfbk".to_vec();
        body.extend_from_slice(b"LIST");
        body.extend_from_slice(&0xFFFF_FFF0u32.to_le_bytes());
        body.extend_from_slice(b"sdta");
        body.extend_from_slice(b"smpl");
        body.extend_from_slice(&0xFFFF_FF00u32.to_le_bytes());
        body.extend_from_slice(&[0u8; 16]);
        let mut file = b"RIFF".to_vec();
        file.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        file.extend(body);
        let mut parser = Sf2Parser::new(Cursor::new(file)).unwrap();
        assert!(parser.find_sample_data().is_err());
    }

    #[test]
    fn test_parser_reads_samples_and_headers() {
        let pcm = block_pcm();
        let sf2 = build_sf2(&pcm, &[header("One", 0, 100, 0, 1)]);
        let mut parser = Sf2Parser::new(Cursor::new(sf2)).unwrap();
        let (samples, headers) = parser.find_sample_data().unwrap();
        assert_eq!(samples, pcm);
        assert_eq!(headers.len(), 1);
        assert_eq!(headers[0].name, "One");
    }

    #[test]
    fn test_extract_stereo_and_mono() {
        let mut left = header("Pad-L", 0, 100, 1, 4);
        left.start_loop = 10;
        left.end_loop = 90;
        left.original_key = 48;
        let right = header("Pad-R", 100, 200, 0, 2);
        let mut solo = header("Solo", 200, 300, 0, 1);
        solo.start_loop = 210;
        solo.end_loop = 290;

        let (out, files, metadata) = extract("stereo_and_mono", &block_pcm(), &[left, right, solo]);
        assert_eq!(files, ["Pad.wav", "Solo.wav"]);

        let pad = read_wav(&out, "Pad.wav");
        assert_eq!(pad.channels, 2);
        assert_eq!(pad.samples.len(), 200);
        assert_eq!(&pad.samples[..2], [1000, 2000], "left then right");
        assert_eq!(pad.smpl, Some((48, 10, 89)));
        assert_eq!(pad.riff_size as usize, pad.file_len - 8);

        let solo = read_wav(&out, "Solo.wav");
        assert_eq!(solo.channels, 1);
        assert_eq!(solo.samples, vec![3000; 100]);
        assert_eq!(solo.smpl, Some((60, 10, 89)));

        assert_eq!(metadata.len(), 2);
        assert!(metadata[0].is_stereo_pair && metadata[0].has_loop);
        assert_eq!(metadata[0].linked_sample.as_deref(), Some("Pad-R"));
        assert_eq!(metadata[0].sample_type, "0x0004");
        assert!(!metadata[1].is_stereo_pair);
    }

    #[test]
    fn test_extract_right_channel_stored_first() {
        let headers = [header("PadA", 0, 100, 1, 2), header("PadB", 100, 200, 0, 4)];
        let (out, files, _) = extract("right_first", &block_pcm(), &headers);
        assert_eq!(files, ["PadB.wav"]);
        let wav = read_wav(&out, "PadB.wav");
        assert_eq!(&wav.samples[..2], [2000, 1000], "left is the type 4 sample");
    }

    #[test]
    fn test_extract_links_survive_unusable_headers() {
        // An unnamed sample and an empty one sit before the pair
        let headers = [
            header("", 0, 100, 0, 1),
            header("Gap", 0, 0, 0, 1),
            header("Str-L", 100, 200, 3, 4),
            header("Str-R", 200, 300, 2, 2),
            header("Other", 300, 400, 0, 1),
        ];
        let (out, files, _) = extract("stable_links", &block_pcm(), &headers);
        assert_eq!(files, ["Str.wav", "sample_000.wav", "Other.wav"]);
        assert_eq!(&read_wav(&out, "Str.wav").samples[..2], [2000, 3000]);
        assert_eq!(read_wav(&out, "sample_000.wav").samples[0], 1000);
        assert_eq!(read_wav(&out, "Other.wav").samples[0], 4000);
    }

    #[test]
    fn test_extract_never_overwrites() {
        let headers = [
            header("Wide-L", 0, 100, 0, 1),
            header("Wide-R", 100, 200, 0, 1),
            header("Same", 200, 300, 0, 1),
            header("Same", 300, 400, 0, 1),
        ];
        let (out, files, _) = extract("no_overwrite", &block_pcm(), &headers);
        assert_eq!(
            files,
            ["Wide-L.wav", "Wide-R.wav", "Same.wav", "Same (2).wav"]
        );
        assert_eq!(read_wav(&out, "Wide-L.wav").samples[0], 1000);
        assert_eq!(read_wav(&out, "Wide-R.wav").samples[0], 2000);
        assert_eq!(read_wav(&out, "Same.wav").samples[0], 3000);
        assert_eq!(read_wav(&out, "Same (2).wav").samples[0], 4000);
    }

    #[test]
    fn test_extract_survives_bad_headers() {
        let mut rate0 = header("Zero", 200, 300, 0, 1);
        rate0.sample_rate = 0;
        rate0.start_loop = 210;
        rate0.end_loop = 290;
        let headers = [
            header("Bad-L", 0, 100, 1, 4),
            header("Bad-R", 100, 99999, 0, 2), // runs off the data
            rate0,
            header("Rom", 300, 400, 0, 0x8001),
        ];
        let (out, files, metadata) = extract("bad_headers", &block_pcm(), &headers);
        // The usable half of the broken pair is kept as a mono file
        assert_eq!(files, ["Bad-L.wav", "Zero.wav"]);
        assert_eq!(read_wav(&out, "Bad-L.wav").samples, vec![1000; 100]);
        let zero = read_wav(&out, "Zero.wav");
        assert_eq!(zero.sample_rate, FALLBACK_SAMPLE_RATE);
        assert_eq!(zero.smpl, Some((60, 10, 89)));
        assert!(metadata.iter().all(|m| m.duration_seconds.is_finite()));
    }

    #[test]
    fn test_has_sf2_extension() {
        assert!(has_sf2_extension(Path::new("a.sf2")));
        assert!(has_sf2_extension(Path::new("a.SF2")));
        assert!(!has_sf2_extension(Path::new("a.sf3")));
        assert!(!has_sf2_extension(Path::new("sf2")));
    }
}
