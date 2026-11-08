# Pixel rearrangement

Rearrange the RGB pixels of a portrait into an approximation of the Kali image, then animate every pixel flying to its new place in a 12-second MP4 that fits WhatsApp's 10 MB limit. Everything runs in Rust: decoding, matching, verification and video encoding. It uses std threads and three crates: `image`, `openh264` (Cisco's H.264 encoder, compiled from bundled source by cargo) and `mp4` (a pure-Rust muxer). Install `nasm` before building so openh264 gets its SIMD assembly; without it the encode is about 3× slower.

## Run

```sh
cargo build --release
./target/release/pixel-rearrangement   # writes output/
./target/release/audit                  # benchmark, memory estimate, MP4 duration and size check
```

Images aren't included in the repo. Put a source photo and a target image in the project folder and point `source` and `target` in `src/main.rs` at them. Building needs a C++ compiler (for openh264's bundled source), and `nasm` is recommended.

To change the photos, FPS or length, edit `src/main.rs`. It's an 11-line `Config`; every other setting has a default in `src/lib.rs`.

Outputs in `output/`: `source.png`, `target.png`, `rearranged.png`, `mapping.bin` and `rearrangement.mp4` (H.264, 12 s, ≤ 9.5 MB). `mapping.bin` holds a little-endian `u32` width and `u32` height, followed by one `u64` destination slot for each row-major source pixel.

## Layout

| File | Purpose |
| --- | --- |
| `src/main.rs` | Source image, target image, FPS and seconds. Nothing else. |
| `src/lib.rs` | `Config`, the pipeline (`run`), the matcher, the permutation/RGB-multiset checks, a tiny std-thread job queue, and the RNG. |
| `src/animate.rs` | Particle paths, parallel frame rendering (6 workers) and the H.264 MP4 writer. |
| `src/audit.rs` | `audit` binary: benchmark CSV, memory estimate, and MP4 frame/duration/size check. Writes nothing. |

## Algorithm

The objective is to minimize `Σ ‖Oklab_src − Oklab_dst‖² + λ‖pos_src − pos_dst‖²` over permutations. Pixels keep their exact RGB bytes; Oklab is used only for scoring.

1. **Initialize (recursive median partition).** Split both feature clouds at the median of the axis with the largest combined spread, and recurse. Equal halves give a bijection, even with duplicate colours. The top levels run on threads.
2. **Inline layout.** Position `k` holds the source feature `src[k]` *and* the target feature `dst[k]` it is currently assigned to. A swap exchanges two `dst` entries, so the swap test `(sᵢ − sₖ)·(tᵢ − tₖ) < 0` reads only nearby memory. There are no index indirections and no inverse arrays.
3. **Block-parallel greedy swaps.** Positions are cut into 16 384-position blocks (768 KiB, so they stay in L2 cache). Each pixel tries 2 partners within ±64 in the current order and 2 random partners in its block, and accepts only strict improvements. Blocks are disjoint, so they run in parallel, and boundaries shift by half a block on odd passes. Seeds depend on (seed, pass, block), so output is byte-identical for any thread count.
4. **Sliced re-sorting.** Every 5 passes, positions are re-sorted by the projection of the source colour onto a random direction. This is the slicing idea from sliced optimal transport ([Bonneel et al.](https://perso.liris.cnrs.fr/nbonneel/spot/), [sliced OT for pixel matching](https://rreusser.github.io/notebooks/sliced-optimal-transport)). Each new ordering puts different colour-similar pixels next to each other, so block-local passes keep finding long-range improvements.
5. **Verify.** The result must be a bijection, and the sorted packed-RGB multiset of the output must equal the source's.

Animation: the default grid is 128 × 176 with 4 × 4 particles on a 768 × 1056 canvas. Paths (spread → curved Bézier → settle, with staggered starts) are computed on the fly from a stateless hash, so no per-particle path table is stored. Particles snap to multiples of their size, so each 4 × 4 particle fills exactly one 4 × 4 luma block and one 2 × 2 chroma block and encodes as flat colour instead of noise. Frames are rendered in RGB in parallel (6 workers), converted to YUV 4:2:0 and encoded with openh264. Timeline: about 0.8 s on the portrait, 9.7 s of flight and 1.5 s on the result; hold frames are rendered once and repeated. The MP4 is a constant 25 fps: 300 frames, exactly **12.000 s**.

Size cap: `Config::max_bytes` (9.5 MB) sets the starting bitrate. After encoding, the file is measured: if it's over the cap the bitrate is lowered, and if it's well under (below 85%) the bitrate is raised, up to 4 tries. A file over the cap is never kept. openh264's rate control is only approximate, which is why the loop measures the real file.

Why 128 × 176 with 4 × 4 particles: at 12 s and 10 MB, the flying cloud is the expensive part. Measured PSNR of a mid-flight frame against the exact render: 256 × 352 with 2 × 2 particles, 15.8 dB (a smear); 160 × 220 with 4 × 4, 18.2 dB; 128 × 176 with 4 × 4, **23.5 dB** (crisp particles). The final image is chunkier but reads fine at phone size.

## Measured (this machine: i5-1335U, 2 P + 8 E cores; release build, seed 42, 40 passes)

Matcher (`audit`, refine time only; old = previous single-file version):

| Grid | Old refine | New refine | Old cost | New cost | Old core mem | New core mem |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 128 × 176 | 30 ms | 24–32 ms | 973.78 | **972.70** | 2.23 MiB | 1.20 MiB |
| 256 × 352 | 187 ms | 60–78 ms | 4126.72 | **4119.79** | 8.94 MiB | 4.81 MiB |
| 512 × 704 | 2142 ms | 180–240 ms | 17354.49 | **17327.95** | 35.75 MiB | 19.25 MiB |

Core matcher storage dropped from 104 to 52 B/pixel (`u32` ids, no rank or inverse arrays). That figure is an estimate of the matcher's arrays, not process RSS.

Full default run (`/usr/bin/time -v`):

| | Original (GIF) | Now (MP4) |
| --- | ---: | ---: |
| Wall time | 5.05 s | 6.7 s (two encodes to fill the size budget) |
| Animation encode | 4.78 s | 6.6 s |
| Duration | 18.00 s | 12.000 s |
| Size | 107.0 MiB | 8.73 MB |
| Peak RSS | 21.8 MB | 51.8 MB |

Peak RSS is higher because 6 workers each hold a full RGB canvas plus its YUV frame while the encoder runs; the worker count is one constant in `src/animate.rs`.

## Exactness and visual limits

`rearranged.png` has exactly the same RGB multiset as `source.png`; the source is nearest-resized to the grid, so no new colours appear. The guarantee applies to decoded RGB, not JPEG bytes. No permutation can create colours the portrait lacks, so the result keeps the portrait's red/pink palette. The MP4 is lossy (H.264, 4:2:0), and mid-flight frames can contain overlapping particles.

<details>
<summary>Reference cleanup provenance and prompt</summary>

Used the built-in `image_gen` tool, then composited only the requested background patch in Rust. The project asset is `kali-clean.png`; the original JPEG remains unchanged. This one-time reference cleanup is separate from the Rust pixel-permutation algorithm.

Final prompt:

> Use case: precise-object-edit. Edit target: the supplied photograph of the Goddess Kali idol. Remove ONLY the white Bengali word 'সুপ্রভাত' overlaid across the top RIGHT of the photograph, at approximately x=420..610 y=15..80 of the 716x976 original. Reconstruct the dark red canopy and pale mint wall behind those letters naturally. Keep the left-hand 'শুভ দীপাবলী' greeting unchanged. Preserve Kali's face, crown, all ornaments, figures, shrine, framing, proportions, lighting, and the rest of the photograph as faithfully as possible. Do not add text or decoration. This is a narrow text-removal edit for use as an image-processing target, not a restyling.

</details>
