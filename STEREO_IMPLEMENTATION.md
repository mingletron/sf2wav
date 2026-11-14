# Stereo Sample Extraction - Implementation Notes

Stereo samples in an SF2 file are detected, paired, interleaved and written
as two-channel WAV files. Everything else is written as mono.

## SF2 Stereo Sample Format

From the SoundFont 2 specification, section 7.10 (the `shdr` sub-chunk).

### Sample Type Values

`sample_type` (`sfSampleType`) is an enumeration, not a two-bit field:

| Value | Meaning |
|-------|---------|
| `0x0001` | Mono sample |
| `0x0002` | Right sample of a stereo pair |
| `0x0004` | Left sample of a stereo pair |
| `0x0008` | Linked sample (not fully defined by the spec) |
| `0x8001`, `0x8002`, `0x8004`, `0x8008` | The same, but the data is in ROM, not in the file |

### Sample Linking

- A stereo sample is stored as **two separate mono samples** in the `smpl`
  chunk, each with its own header.
- `sample_link` (`wSampleLink`) is the **zero-based index** of the other
  channel's sample header. Index 0 is a valid link, so a link of 0 cannot be
  used on its own to mean "not linked": the sample type says that.
- Left and right are usually consecutive, but need not be, and either can
  come first.

```
shdr:   [0] Pad-L  type=4  link=1
        [1] Pad-R  type=2  link=0
        [2] Solo   type=1  link=0   (mono: the link is ignored)
```

## Implementation

### `SampleHeader::load_from_bytes`

Keeps every record except the terminal `EOS` one. Records that cannot be
extracted (zero length, out of range) stay in the list, because
`sample_link` indexes it: removing one would shift every later link onto
the wrong sample. They are skipped at extraction time.

### `find_stereo_pairs`

For each sample that is not mono, not ROM and not already paired:

1. It is a candidate if its type is left or right, or (for files with junk
   types) if its link is non-zero.
2. The linked header must exist, be a different sample, be unpaired, and
   not be mono or ROM.
3. Left and right are decided by `sample_type` of either sample. Only if
   neither says are name suffixes (`-L`/`-R`/`_L`/`_R`) used, and after
   that the lower index is taken as left.

The pair is named after the left sample with its stereo suffix removed.

### `extract_samples`

- Pairs whose two ranges are both inside the sample data are interleaved
  `LRLRLR...` and written with 2 channels. If the lengths differ, the
  shorter one is used.
- If either range is invalid the pair is abandoned with a warning and each
  usable half is written as a mono file.
- Stereo files take the sample rate, original key and loop points of the
  left channel. The loop is dropped if it does not fit the shorter channel.
- Only a pair loses its `-L`/`-R` suffix. Unpaired samples keep their full
  names, and `FileNamer` adds ` (2)`, ` (3)`... on any collision, so two
  samples never write to the same file.

### Loop points

`SampleHeader::loop_points` accepts a loop when
`start <= start_loop < end_loop <= end`. SF2's `end_loop` is the first
sample after the loop; the WAV `smpl` chunk wants the last sample played,
so the value written is `end_loop - start - 1`.

### CSV Metadata

- `Channels` (1 or 2)
- `Is Stereo Pair` (true/false)
- `Linked Sample` (name of the right channel, empty for mono)
- `Sample Type` (hex value of the left channel's `sample_type`)

## Tests

`cargo test` builds small SF2 files in memory and checks the WAVs that
come out: correct pairing with zero-based links, a pair stored right-first,
links that survive unusable headers before them, no overwriting, and
broken ranges, zero sample rates and ROM samples.
