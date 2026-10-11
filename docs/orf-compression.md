# Olympus and OM System ORF / ORI

PhotoCraft reads the measured uncompressed and predictive-compressed Bayer layouts in
Olympus / OM System ORF files, including the normal-resolution `.ORI` stored beside some
high-resolution captures. Storage and recorded precision determine the decoder; the camera
model and filename extension do not select a codec.

The implementation is in [orf.rs](../crates/raw/src/orf.rs) and
[orf_compressed.rs](../crates/raw/src/orf_compressed.rs). This note records the observed
compressed profile, its source basis and the limits of the qualification.

## Sources and measured rules

Public Olympus patents supplied useful coding families:

| Source | Contribution to the experiments |
|---|---|
| [US7333035B2](https://patents.google.com/patent/US7333035B2/en) | Eight spatial predictors, including the median-edge predictor and averaging; combinations are permitted |
| [US7715639B2](https://patents.google.com/patent/US7715639B2/en) | Filtered prediction errors and adaptive Rice coding |
| [JP2006211155A](https://patents.google.com/patent/JP2006211155A/en) | Separating lower bits and applying weighted correction to the remaining error |
| [US7259698B2](https://patents.google.com/patent/US7259698B2/en) | Capped unary escape coding, including a 12-zero / 15-bit example |

These publications do not specify the complete ORF byte stream. Headers, bit order, state
resets, parameter choices, correction recurrence, predictor threshold and final padding below
were established by direct measurements of CC0 camera files. In particular, the observed
escape retains a one-bit marker that the patent's delimiter-free example omits. No
third-party RAW decoder implementation or binary decoder oracle was used for this work.

Prepared with Codex assistance. The authoring context includes public discussions of
LightCraft #360; its rejected decoder and source-derived implementation explanations were
not used. This disclosure and the numerical checks are not independent provenance
certification or a claim of maintainer approval.

The earlier uncompressed paths use direct file measurements and the accepted prose in
[LightCraft #651](https://github.com/storytold/lightcraft/pull/651) for padded blocks and
[LightCraft #600](https://github.com/storytold/lightcraft/pull/600) for two-field compact
storage. Their decoder implementations were not read or copied.

## Compressed profile

The measured container is little-endian TIFF with `Compression=1`, `BitsPerSample=16`, one
unsigned sample per pixel and one strip. Maker-note `ImageProcessing.ValidBits` supplies
the actual precision, 12 or 14. Width is even and both dimensions are at least two. CFA,
crop, black levels and white balance come from file metadata; `OLYMPUS` and `OM SYSTEM`
maker-note headers are recognized.

In this compressed profile, `ImageProcessing.BlackLevel2` (`0x0600`) remains in 12-bit
units. Use its levels unchanged for 12-bit samples and multiply them by four for 14-bit
samples before black subtraction. This rule comes from paired-file measurements below;
the public tag table does not specify the units. White level remains `2^ValidBits-1`.

| Precision | Strip header bytes |
|---|---|
| 12 bit | `00 00 00 00 01 00 00` |
| 14 bit | `00 00 00 00 00 01 00 00` |

The remaining bits are read most-significant first, one symbol per sensor sample in row
order, with no alignment between rows. Two states serve the even and odd columns. Both
start each row with previous magnitude `m=0`, small-magnitude run `r=0` and correction
`b=0`. Let `bit_length(0)=0`.

1. Select `k=max(bit_length(m),2)` when `r>=3`; otherwise select
   `k=max(bit_length(m)-2,4)`.
2. Read one sign bit, then two low bits `l`. Count zero bits, stopping at a one or twelve
   zeros. Below twelve, that count is quotient `q` and the one terminates it. At twelve,
   read a new `q` in `15-k` bits, require a one marker and require `q>=12`.
3. Read `k` remainder bits `t`; the new magnitude is `m=(q<<k)|t`, required below `2^15`.
   There is no subtraction from the escaped quotient. Set `r=r+1` if `m<=16`, otherwise
   reset it to zero; only whether it reaches three matters.
4. Set `v=m` for sign zero, otherwise `v=-m-1`. With the old correction `b`, compute
   `u=v+b`, residual `e=4*u+l`, then `b=floor((3*u+b)/32)`. Negative division uses
   mathematical floor.

Prediction uses same-colour neighbours two positions apart: `A=(x-2,y)`, `B=(x,y-2)`
and `C=(x-2,y-2)`. In the first two rows, predict zero for the first two columns and
`A` elsewhere. In later rows, the first two columns use `B`. For the interior, with
`L=min(A,B)` and `H=max(A,B)`:

```text
C >= H                         -> L
C <= L                         -> H
L < C < H and max(H-C,C-L)<=32  -> floor((A+B)/2)
otherwise                      -> A+B-C
```

The reconstructed sample is prediction plus `e`, required to fit the recorded precision;
it is neither wrapped nor clipped. After exactly `width*height` symbols, only zero to seven
one bits may remain. Truncation, invalid escape markers, extra bytes and invalid padding
are errors. A valid full compressed stream takes precedence over coincidentally matching
uncompressed byte lengths; otherwise the existing exact uncompressed layouts remain available.

## Qualification and reproducible checks

On 2026-10-10, all 104 ORF / ORI originals listed as CC0 in the
[raw.pixls.us catalog](https://raw.pixls.us/json/getrepository.php?set=all) were downloaded and checked against its
published SHA256 values. Local release decoding and development completed for all 104:
17 uncompressed files and 87 compressed files, of which 83 record 12-bit precision and four
record 14-bit precision. The 17 previously developed outputs were unchanged. The checked-in
[qualification manifest](orf-corpus.json) records each original URL, license, input hash,
sensor dimensions, precision and full decoded-sensor hash.

All 87 compressed sensor arrays matched the full row-major little-endian `u16` SHA256
digests produced by a separate C++ research implementation. That implementation also
consumed the exact symbol counts with the measured final padding. This cross-implementation
agreement checks the Rust port; both implementations use the same inferred format rules.
Independent byte extraction also supplies matching full-sample hashes for 14 of the 17
uncompressed files; the remaining three have regression digests from PhotoCraft.

A separate scene check compared E-M5 II catalog samples #2856 (`CB252215.ORF`, padded
high-resolution) and #2857 (`CB252215.ORI`, compressed). They record the same timestamp,
exposure, aperture and ISO. Direct padded-block extraction, with each file's CFA and crop,
gave per-colour correlations of **0.9906–0.9968** over 60,208 comparison points. Those points
were separate from the grid used to choose alignment. A 1,024-pixel displacement control
gave **-0.0557 to -0.0214**. Multi-shot synthesis prevents exact corresponding-sample
equality; this supports scene structure rather than serving as a bit-exact reference decoder.

Four OM-1 Mark II / OM-3 capture pairs provide a separate precision check: 14-bit high-res
files #7267, #7269, #7801 and #7803 each have a 12-bit normal-resolution counterpart
(#7268, #7270, #7802 and #7804) with matching recorded timestamp, exposure, ISO and white
balance. Across their 16 CFA-site comparisons, the median `high14 - 4*normal12` residual
was **-2.14 to +2.19** sample codes. Keeping the roughly 254-code metadata black level
unscaled would leave an approximately 762-code black-offset error. Scaling that level
addresses the offset in development; it leaves the decoded sensor samples unchanged and
does not add camera colour calibration.

The checked-in corpus pins include five compressed representatives: E-5, E-M1, E-M5 II
ORI, OM-1 and OM-1 Mark II 14-bit high-resolution. Tests check full-sensor CRC32 fingerprints,
dimensions, crop, CFA, black/white levels, white balance and embedded previews. CRC32 is a
regression fingerprint; input downloads remain SHA256-pinned. The other ORF corpus tests
check uncompressed layout boundaries and metadata. Source URLs, hashes and credits are in
[corpus_pins.rs](../xtask/src/corpus_pins.rs), [pixls.sha256](../xtask/pixls.sha256) and
[ATTRIBUTION.md](../ATTRIBUTION.md).

```sh
cargo xtask corpus --pixls
cargo test -p photocraft-raw --features corpus --test raw_corpus
cargo test -p photocraft-raw --test vendor_codecs orf
cargo run --release -p photocraft-raw --example rawinfo -- --sensor-u16le /tmp/CB252215.u16le corpus/pixls/CB252215.ORI
shasum -a 256 corpus/pixls/CB252215.ORI /tmp/CB252215.u16le
mkdir -p /tmp/orf-preview
cargo run --release -p photocraft-raw --example rawinfo -- --png /tmp/orf-preview corpus/pixls/CB252215.ORI
```

Compare the two hashes with manifest entry #2857. `--sensor-u16le` writes every decoded
sensor sample before crop, black subtraction, white balance or development, then skips
development. Any manifest original can be checked the same way. The 104-file qualification
was a local run; the smaller pinned corpus is the CI subset, enabled by the existing corpus
job's `cargo xtask test-corpus --pixls`. Local checks passed 133 raw/corpus tests and 412
I/O tests, with the focused raw import/export tests rerun after the black-level correction.
Five native Metal workflows passed (E-M1, E-M5 II ORI, OM-1, OM-1 II 50 MP and OM-3
81 MP), including reruns of both 14-bit files after that correction: sensor development,
profile retention, exposure/temperature changes, crop/undo/redo, native save/reopen and
byte-identical 16-bit PNG exports before/after reopening. Original file hashes were unchanged.
The release RAW/I/O corpus run passed 569 tests. Strict raw Clippy, architecture layering,
workflow syntax and the full WebAssembly check passed. Seven release decodes of a 24 MP
synthetic image gave medians of 358 ms (12 bit) and 401 ms (14 bit) on Apple M1 Pro,
with complete output equality checked for every run; encoding and comparison are excluded.

## Limits

Coverage describes these observed layouts and files, not every camera or recording mode.
Compressed variants with other precision, headers, byte order or multiple strips are
unsupported, as are non-Bayer ORFs. Invalid compressed streams are rejected. Since compressed
and uncompressed files share TIFF coding fields, a failed compressed interpretation at an
exact known uncompressed size falls back to that uncompressed layout; damaged bytes can be
ambiguous in that case. For an unsupported layout, the I/O layer can use an available embedded
preview. Resource limits still apply to large captures.

ORF colour calibration remains unchanged: white-balanced camera channels use the existing
linear-sRGB fallback, with a warning, before conversion to the working profile. No camera
colour matrix or tone curve was added. A successful sensor decode therefore does not promise
the saturation, brightness or rendering of the camera JPEG or a calibrated raw converter.
