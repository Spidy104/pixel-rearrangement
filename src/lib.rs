pub mod animate;

use image::{RgbImage, imageops::FilterType};
use std::{
    error::Error,
    fs::{self, File},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    sync::Mutex,
    thread,
    time::Instant,
};

pub type Result<T> = std::result::Result<T, Box<dyn Error>>;

pub struct Config {
    pub source: PathBuf,
    pub target: PathBuf,
    pub out: PathBuf,
    pub width: u32,
    pub height: u32,
    pub passes: usize,
    pub spatial: f32,
    pub seed: u64,
    pub fps: u32,
    pub seconds: u32,
    pub scale: u32,
    pub max_bytes: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            source: "1000071110.jpg".into(),
            target: "kali-clean.png".into(),
            out: "output".into(),
            width: 128,
            height: 176,
            passes: 40,
            spatial: 0.002,
            seed: 42,
            fps: 25,
            seconds: 12,
            scale: 4,
            max_bytes: 9_500_000,
        }
    }
}

pub fn load(path: &Path) -> Result<image::DynamicImage> {
    let mut decoder = image::ImageReader::open(path)?.into_decoder()?;
    let orientation = image::ImageDecoder::orientation(&mut decoder)?;
    let mut decoded = image::DynamicImage::from_decoder(decoder)?;
    decoded.apply_orientation(orientation);
    Ok(decoded)
}

pub fn run(c: &Config) -> Result<()> {
    if c.width == 0 || c.height == 0 || !(1..=100).contains(&c.fps) || c.fps * c.seconds < 2 {
        return Err("need a positive grid, fps 1..100 and at least two frames".into());
    }
    let source = load(&c.source)?.to_rgb8();
    let source = image::imageops::resize(&source, c.width, c.height, FilterType::Nearest);
    let target = load(&c.target)?
        .resize_to_fill(c.width, c.height, FilterType::Triangle)
        .to_rgb8();

    let start = Instant::now();
    let mut m = Matching::new(features(&source, c.spatial), features(&target, c.spatial));
    let accepted = m.refine(c.passes, c.seed);
    let map = m.map();
    eprintln!(
        "{}x{}: matched in {:.3}s; cost {:.4}; {accepted} swaps",
        c.width,
        c.height,
        start.elapsed().as_secs_f64(),
        m.cost()
    );
    drop(m);

    let end = rearrange(&source, &map);
    verify(&source, &end, &map)?;
    fs::create_dir_all(&c.out)?;
    source.save(c.out.join("source.png"))?;
    target.save(c.out.join("target.png"))?;
    end.save(c.out.join("rearranged.png"))?;
    let mut file = BufWriter::new(File::create(c.out.join("mapping.bin"))?);
    file.write_all(&c.width.to_le_bytes())?;
    file.write_all(&c.height.to_le_bytes())?;
    for &j in &map {
        file.write_all(&u64::from(j).to_le_bytes())?;
    }
    file.flush()?;
    animate::animate(c, &source, &map)?;
    eprintln!(
        "saved {}; permutation and exact RGB counts verified",
        c.out.display()
    );
    Ok(())
}

pub type Feature = [f32; 5];

#[derive(Clone, Copy)]
pub struct Item {
    pub f: Feature,
    pub id: u32,
}

pub fn features(image: &RgbImage, spatial: f32) -> Vec<Item> {
    let linear: [f32; 256] = std::array::from_fn(|v| {
        let v = v as f32 / 255.0;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    });
    let (w, h) = image.dimensions();
    let (dx, dy) = ((w - 1).max(1) as f32, (h - 1).max(1) as f32);
    let weight = spatial.sqrt();
    image
        .enumerate_pixels()
        .enumerate()
        .map(|(id, (x, y, c))| {
            let [r, g, b] = c.0.map(|v| linear[v as usize]);
            let l = (0.41222146 * r + 0.53633255 * g + 0.051445995 * b).cbrt();
            let m = (0.2119035 * r + 0.6806995 * g + 0.10739696 * b).cbrt();
            let s = (0.08830246 * r + 0.28171885 * g + 0.6299787 * b).cbrt();
            Item {
                f: [
                    0.21045426 * l + 0.7936178 * m - 0.004072047 * s,
                    1.9779985 * l - 2.4285922 * m + 0.4505937 * s,
                    0.025904037 * l + 0.78277177 * m - 0.80867577 * s,
                    x as f32 / dx * weight,
                    y as f32 / dy * weight,
                ],
                id: id as u32,
            }
        })
        .collect()
}

fn distance(a: &Feature, b: &Feature) -> f32 {
    a.iter().zip(b).map(|(a, b)| (a - b) * (a - b)).sum()
}

pub struct Matching {
    src: Vec<Item>,
    dst: Vec<Item>,
}

const BLOCK: usize = 16384;
const WINDOW: u64 = 64;
const EPOCH: usize = 5;

impl Matching {
    pub fn new(mut src: Vec<Item>, mut dst: Vec<Item>) -> Self {
        assert_eq!(src.len(), dst.len());
        partition(&mut src, &mut dst, 0);
        Self { src, dst }
    }

    pub fn cost(&self) -> f64 {
        self.src
            .iter()
            .zip(&self.dst)
            .map(|(s, d)| f64::from(distance(&s.f, &d.f)))
            .sum()
    }

    pub fn map(&self) -> Vec<u32> {
        let mut map = vec![0; self.src.len()];
        for (s, d) in self.src.iter().zip(&self.dst) {
            map[s.id as usize] = d.id;
        }
        map
    }

    pub fn refine(&mut self, passes: usize, seed: u64) -> usize {
        let n = self.src.len();
        let mut accepted = 0;
        for pass in 0..passes {
            if pass > 0 && pass % EPOCH == 0 {
                self.reorder(splitmix(seed ^ pass as u64));
            }
            let shift = if pass % 2 == 1 && n > BLOCK {
                BLOCK / 2
            } else {
                0
            };
            let (head, tail) = self.dst.split_at_mut(shift);
            let mut jobs = vec![(0, head)];
            jobs.extend(
                tail.chunks_mut(BLOCK)
                    .zip((shift..).step_by(BLOCK))
                    .map(|(d, b)| (b, d)),
            );
            let src = &self.src[..];
            accepted += parallel(jobs, |(base, dst)| {
                let seed = splitmix(seed ^ ((pass as u64) << 40) ^ base as u64);
                refine_block(&src[base..base + dst.len()], dst, seed)
            })
            .into_iter()
            .sum::<usize>();
        }
        accepted
    }

    fn reorder(&mut self, seed: u64) {
        let mut rng = Rng::new(seed);
        let mut gauss =
            || (0..4).map(|_| (rng.next_u64() >> 40) as f32).sum::<f32>() / 16777216. - 2.;
        let dir = [gauss(), gauss(), gauss()];
        let mut keys: Vec<(f32, u32)> = self
            .src
            .iter()
            .map(|x| x.f[0] * dir[0] + x.f[1] * dir[1] + x.f[2] * dir[2])
            .zip(0..)
            .collect();
        keys.sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        self.src = keys.iter().map(|&(_, k)| self.src[k as usize]).collect();
        self.dst = keys.iter().map(|&(_, k)| self.dst[k as usize]).collect();
    }
}

fn refine_block(src: &[Item], dst: &mut [Item], seed: u64) -> usize {
    let m = dst.len();
    if m < 2 {
        return 0;
    }
    let mut rng = Rng::new(seed);
    let window = WINDOW.min(m as u64 - 1);
    let below = |bits: u64, n: u64| (((bits & 0xffff_ffff) * n) >> 32) as usize;
    let wrap = |k: usize| if k >= m { k - m } else { k };
    let mut accepted = 0;
    for i in 0..m {
        let (r1, r2) = (rng.next_u64(), rng.next_u64());
        let candidates = [
            wrap(i + 1 + below(r1, window)),
            wrap(i + 1 + below(r2, window)),
            below(r1 >> 32, m as u64),
            below(r2 >> 32, m as u64),
        ];
        for k in candidates {
            if k == i {
                continue;
            }
            let (si, sk, ti, tk) = (&src[i].f, &src[k].f, &dst[i].f, &dst[k].f);
            let delta: f32 = (0..5).map(|d| (si[d] - sk[d]) * (ti[d] - tk[d])).sum();
            if delta < -1e-10 {
                dst.swap(i, k);
                accepted += 1;
            }
        }
    }
    accepted
}

fn partition(a: &mut [Item], b: &mut [Item], depth: u32) {
    let n = a.len();
    if n <= 1 {
        return;
    }
    let mut spread = [0.0f32; 5];
    for items in [&*a, &*b] {
        let mut lo = [f32::INFINITY; 5];
        let mut hi = [f32::NEG_INFINITY; 5];
        for item in items {
            for d in 0..5 {
                lo[d] = lo[d].min(item.f[d]);
                hi[d] = hi[d].max(item.f[d]);
            }
        }
        for d in 0..5 {
            spread[d] += (hi[d] - lo[d]).powi(2);
        }
    }
    let axis = (0..5)
        .max_by(|&i, &j| spread[i].total_cmp(&spread[j]))
        .unwrap();
    let key = |x: &Item, y: &Item| x.f[axis].total_cmp(&y.f[axis]).then(x.id.cmp(&y.id));
    let mid = n / 2;
    a.select_nth_unstable_by(mid, key);
    b.select_nth_unstable_by(mid, key);
    let (a0, a1) = a.split_at_mut(mid);
    let (b0, b1) = b.split_at_mut(mid);
    if depth < 4 && n >= 1 << 15 {
        thread::scope(|s| {
            s.spawn(|| partition(a0, b0, depth + 1));
            partition(a1, b1, depth + 1);
        });
    } else {
        partition(a0, b0, depth + 1);
        partition(a1, b1, depth + 1);
    }
}

pub fn rearrange(source: &RgbImage, map: &[u32]) -> RgbImage {
    let mut out = RgbImage::new(source.width(), source.height());
    let raw = out.as_mut();
    for (pixel, &slot) in source.pixels().zip(map) {
        let slot = slot as usize * 3;
        raw[slot..slot + 3].copy_from_slice(&pixel.0);
    }
    out
}

pub fn verify(source: &RgbImage, output: &RgbImage, map: &[u32]) -> Result<()> {
    if !validate(map) {
        return Err("internal error: assignment is not a permutation".into());
    }
    let histogram = |img: &RgbImage| {
        let mut pixels: Vec<_> = img
            .pixels()
            .map(|c| u32::from_be_bytes([0, c[0], c[1], c[2]]))
            .collect();
        pixels.sort_unstable();
        pixels
    };
    if histogram(source) != histogram(output) {
        return Err("internal error: RGB multiset changed".into());
    }
    Ok(())
}

fn validate(map: &[u32]) -> bool {
    let mut seen = vec![false; map.len()];
    for &j in map {
        let j = j as usize;
        if j >= map.len() || seen[j] {
            return false;
        }
        seen[j] = true;
    }
    true
}

pub(crate) fn threads() -> usize {
    thread::available_parallelism().map_or(1, |n| n.get())
}

pub(crate) fn parallel<T: Send, R: Send>(jobs: Vec<T>, f: impl Fn(T) -> R + Sync) -> Vec<R> {
    let n = jobs.len();
    let queue = Mutex::new(jobs.into_iter().enumerate());
    let work = || {
        let mut done = Vec::new();
        loop {
            let next = queue.lock().unwrap().next();
            let Some((i, job)) = next else { return done };
            done.push((i, f(job)));
        }
    };
    let mut results: Vec<(usize, R)> = thread::scope(|s| {
        let helpers: Vec<_> = (1..threads().min(n)).map(|_| s.spawn(work)).collect();
        let mut all = work();
        for h in helpers {
            all.extend(h.join().expect("worker panicked"));
        }
        all
    });
    results.sort_unstable_by_key(|&(i, _)| i);
    results.into_iter().map(|(_, r)| r).collect()
}

pub(crate) fn splitmix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9e3779b97f4a7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
    z ^ (z >> 31)
}

pub(crate) struct Rng(u64);
impl Rng {
    pub(crate) fn new(seed: u64) -> Self {
        Self(seed)
    }
    pub(crate) fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e3779b97f4a7c15);
        splitmix(self.0)
    }
}
