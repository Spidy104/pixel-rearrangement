use image::imageops::FilterType;
use pixel_rearrangement::{Config, Item, Matching, Result, features, load, rearrange, verify};
use std::{fs::File, io::BufReader, time::Instant};

fn main() -> Result<()> {
    let mut c = Config::default();
    if let Some(target) = std::env::args().nth(1) {
        c.target = target.into();
    }
    let source = load(&c.source)?.to_rgb8();
    let target = load(&c.target)?;

    println!(
        "grid,pixels,init_ms,refine_ms,verify_ms,initial_cost,final_cost,accepted,core_mib_estimate"
    );
    for w in [128_u32, 256, 512] {
        let h = w * c.height / c.width;
        let s = image::imageops::resize(&source, w, h, FilterType::Nearest);
        let t = target.resize_to_fill(w, h, FilterType::Triangle).to_rgb8();
        let (a, b) = (features(&s, c.spatial), features(&t, c.spatial));

        let start = Instant::now();
        let mut m = Matching::new(a, b);
        let init_ms = start.elapsed().as_secs_f64() * 1000.;
        let initial = m.cost();
        let start = Instant::now();
        let accepted = m.refine(c.passes, c.seed);
        let refine_ms = start.elapsed().as_secs_f64() * 1000.;
        let final_cost = m.cost();

        let map = m.map();
        let start = Instant::now();
        verify(&s, &rearrange(&s, &map), &map)?;
        let verify_ms = start.elapsed().as_secs_f64() * 1000.;
        let n = map.len();
        let bytes = n * (2 * size_of::<Item>() + size_of::<u32>());
        println!(
            "{w}x{h},{n},{init_ms:.2},{refine_ms:.2},{verify_ms:.2},{initial:.6},{final_cost:.6},{accepted},{:.2}",
            bytes as f64 / 1048576.
        );
    }

    let path = c.out.join("rearrangement.mp4");
    match File::open(&path) {
        Ok(file) => {
            let size = file.metadata()?.len();
            let mp4 = mp4::Mp4Reader::read_header(BufReader::new(file), size)?;
            let frames: u32 = mp4.tracks().values().map(|t| t.sample_count()).sum();
            println!(
                "mp4: {frames} frames, {:.3}s, {:.2} MB, under {:.1} MB cap: {}",
                mp4.duration().as_secs_f64(),
                size as f64 / 1e6,
                c.max_bytes as f64 / 1e6,
                if size <= c.max_bytes { "yes" } else { "NO" }
            );
        }
        Err(_) => println!(
            "mp4: {} not found; run the main binary first",
            path.display()
        ),
    }
    Ok(())
}
