# pixel-rearrangement

Take every pixel of one photo, shuffle them around, and rebuild a completely different picture with them. Nothing gets painted or blended. Every colour in the final image comes straight from the first photo, just moved somewhere else. Then the whole thing gets animated: each pixel flies from where it started to where it ended up.

I made this to turn a portrait into the Goddess Kali for Kali Puja, and the output is a short video small enough to send on WhatsApp.

> **Sneak peek:** it's Diwali today and I'm fasting, so I'm a little dizzy writing this. If something here doesn't quite make sense, blame the empty stomach. 🪔

## Running it

You need Rust, a C++ compiler, and ideally `nasm` (the video encoder runs about 3× faster with it).

```sh
cargo build --release
./target/release/pixel-rearrangement
```

The photos aren't in this repo. Drop your own two images into the folder and point `src/main.rs` at them:

```rust
run(&Config {
    source: "your-photo.jpg".into(),   // the pixels you have
    target: "your-target.png".into(),  // the picture you want to rebuild
    fps: 25,
    seconds: 12,
    ..Config::default()
})
```

Everything ends up in `output/`:

- `rearrangement.mp4`: the animation (12 seconds, always under 10 MB)
- `rearranged.png`: the final rebuilt picture
- `source.png` / `target.png`: the two inputs at the size used for matching
- `mapping.bin`: where every pixel went

There's also `./target/release/audit`, which runs a speed benchmark and checks the video's length and size. It doesn't write anything.

## How it works

The problem is matching N source pixels to N spots in the target so the colours line up as well as possible. Doing that perfectly is way too slow for tens of thousands of pixels, so it's done in two rough steps:

1. **A quick first guess.** Split both images' colours in half (darker half to darker half, brighter to brighter) and keep splitting until every pixel has a partner. Fast, and already pretty close.
2. **Clean it up with swaps.** Pick two pixels and ask whether they'd both fit better if they traded places. If yes, swap. Repeat this a lot, across all CPU cores at once. Every few rounds, the pixels get re-sorted along a random colour direction (a trick borrowed from [sliced optimal transport](https://rreusser.github.io/notebooks/sliced-optimal-transport)), so they keep meeting new swap partners.

Colours are compared in [Oklab](https://bottosson.github.io/posts/oklab/), a colour space where "looks similar" actually means similar to the human eye. At the end, the program checks that the result really is the exact same set of pixels, just rearranged.

For the video, each pixel becomes a little 4×4 square. They burst outward, curve across the screen, and settle into place. The layout is chosen so the video compresses well, and the program measures the file and re-encodes if needed so it always fits under WhatsApp's 10 MB limit.

## Files

- `src/main.rs`: which photos to use, and how long the video is
- `src/lib.rs`: the settings and the pixel matching
- `src/animate.rs`: the particle animation and video encoding
- `src/audit.rs`: benchmark and checks

## Good to know

- The result can only use colours the source photo already has. If the target is green and your photo has none, it can't make green, and that's the point.
- The video is compressed, so it's not pixel-perfect mid-flight. `rearranged.png` is the exact result.
- Want a different look? Change `width`, `height` and `scale` in the config. Smaller grids with bigger particles look cleaner in the video; bigger grids keep more detail in the final picture.
