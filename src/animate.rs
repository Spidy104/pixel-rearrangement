use crate::{Config, Result, parallel, splitmix};
use image::RgbImage;
use openh264::{
    OpenH264API,
    encoder::{BitRate, Encoder, EncoderConfig, FrameRate, FrameType, QpRange, RateControlMode},
    formats::{RgbSliceU8, YUVBuffer},
};
use std::{fs::File, io::BufWriter, time::Instant};

struct Video {
    encoder: Encoder,
    mp4: mp4::Mp4Writer<BufWriter<File>>,
    size: (u16, u16),
    fps: u32,
    track: bool,
    time: u64,
}

impl Video {
    fn new(c: &Config, width: u32, height: u32, bitrate: u32) -> Result<Self> {
        let config = EncoderConfig::new()
            .max_frame_rate(FrameRate::from_hz(c.fps as f32))
            .rate_control_mode(RateControlMode::Bitrate)
            .bitrate(BitRate::from_bps(bitrate))
            .qp(QpRange::new(10, 51))
            .skip_frames(false);
        let file = BufWriter::new(File::create(c.out.join("rearrangement.mp4"))?);
        let mp4 = mp4::Mp4Writer::write_start(
            file,
            &mp4::Mp4Config {
                major_brand: "isom".parse()?,
                minor_version: 512,
                compatible_brands: ["isom", "iso2", "avc1", "mp41"]
                    .iter()
                    .map(|b| b.parse())
                    .collect::<std::result::Result<_, _>>()?,
                timescale: 1000,
            },
        )?;
        Ok(Self {
            encoder: Encoder::with_api_config(OpenH264API::from_source(), config)?,
            mp4,
            size: (u16::try_from(width)?, u16::try_from(height)?),
            fps: c.fps,
            track: false,
            time: 0,
        })
    }

    fn push(&mut self, yuv: &YUVBuffer) -> Result<()> {
        let stream = self.encoder.encode(yuv)?;
        let is_sync = matches!(stream.frame_type(), FrameType::IDR | FrameType::I);
        let (mut sps, mut pps, mut sample) = (Vec::new(), Vec::new(), Vec::new());
        for l in 0..stream.num_layers() {
            let layer = stream.layer(l).ok_or("missing layer")?;
            for n in 0..layer.nal_count() {
                let nal = layer.nal_unit(n).ok_or("missing nal")?;
                let start = nal
                    .iter()
                    .position(|&b| b != 0)
                    .map_or(nal.len(), |i| i + 1);
                let nal = &nal[start..];
                match nal.first().map(|b| b & 0x1f) {
                    Some(7) => sps = nal.to_vec(),
                    Some(8) => pps = nal.to_vec(),
                    Some(_) => {
                        sample.extend_from_slice(&(nal.len() as u32).to_be_bytes());
                        sample.extend_from_slice(nal);
                    }
                    None => {}
                }
            }
        }
        if !self.track {
            self.mp4.add_track(&mp4::TrackConfig {
                track_type: mp4::TrackType::Video,
                timescale: self.fps,
                language: "und".into(),
                media_conf: mp4::MediaConfig::AvcConfig(mp4::AvcConfig {
                    width: self.size.0,
                    height: self.size.1,
                    seq_param_set: sps,
                    pic_param_set: pps,
                }),
            })?;
            self.track = true;
        }
        self.mp4.write_sample(
            1,
            &mp4::Mp4Sample {
                start_time: self.time,
                duration: 1,
                rendering_offset: 0,
                is_sync,
                bytes: sample.into(),
            },
        )?;
        self.time += 1;
        Ok(())
    }

    fn finish(mut self) -> Result<()> {
        self.mp4.write_end()?;
        Ok(())
    }
}

struct Scene {
    w: usize,
    scale: u32,
    pad_x: f32,
    pad_y: f32,
    ow: u32,
    oh: u32,
    center_x: f32,
    center_y: f32,
    seed: u64,
}

impl Scene {
    fn at(&self, id: usize) -> (f32, f32) {
        (
            self.pad_x + (id % self.w) as f32 * self.scale as f32,
            self.pad_y + (id / self.w) as f32 * self.scale as f32,
        )
    }

    fn draw(&self, map: &[u32], t: f32, mut put: impl FnMut(usize, usize)) {
        let (max_x, max_y) = (self.ow - self.scale, self.oh - self.scale);
        let ease = |v: f32| v * v * (3. - 2. * v);
        for (i, &j) in map.iter().enumerate() {
            let h = splitmix(self.seed ^ (i as u64).wrapping_mul(0xd1b5_4a32_d192_ed03));
            let delay = ((h >> 32) % 1000) as f32 * 0.00012;
            let u = ((t - delay) / (1. - delay)).clamp(0., 1.);
            let (x0, y0) = self.at(i);
            let (x1, y1) = self.at(j as usize);
            let (x, y) = if u == 0. {
                (x0, y0)
            } else if u == 1. {
                (x1, y1)
            } else {
                let spread = |x: f32, c: f32| c + (x - c) * 1.45;
                let (sx, sy) = (spread(x0, self.center_x), spread(y0, self.center_y));
                let (dx, dy) = (spread(x1, self.center_x), spread(y1, self.center_y));
                if u < 0.2 {
                    let u = ease(u / 0.2);
                    (x0 + (sx - x0) * u, y0 + (sy - y0) * u)
                } else if u > 0.8 {
                    let u = ease((u - 0.8) / 0.2);
                    (dx + (x1 - dx) * u, dy + (y1 - dy) * u)
                } else {
                    let bend = (h % 1000) as f32 / 999. - 0.5;
                    let cx = ((sx + dx) * 0.5 - (dy - sy) * bend).clamp(0., max_x as f32);
                    let cy = ((sy + dy) * 0.5 + (dx - sx) * bend).clamp(0., max_y as f32);
                    let u = ease((u - 0.2) / 0.6);
                    let v = 1. - u;
                    (
                        v * v * sx + 2. * v * u * cx + u * u * dx,
                        v * v * sy + 2. * v * u * cy + u * u * dy,
                    )
                }
            };
            let (x, y) = ((x.round() as u32).min(max_x), (y.round() as u32).min(max_y));
            let x = (x - x % self.scale) as usize;
            let y = (y - y % self.scale) as usize;
            for dy in 0..self.scale as usize {
                let row = (y + dy) * self.ow as usize + x;
                for dx in 0..self.scale as usize {
                    put(row + dx, i);
                }
            }
        }
    }
}

pub fn animate(c: &Config, source: &RgbImage, map: &[u32]) -> Result<()> {
    let (w, h) = source.dimensions();
    let (pad_x, pad_y) = (w / 4 * c.scale, h / 4 * c.scale);
    let ow = (w * c.scale + 2 * pad_x + 1) & !1;
    let oh = (h * c.scale + 2 * pad_y + 1) & !1;
    let scene = Scene {
        w: w as usize,
        scale: c.scale,
        pad_x: pad_x as f32,
        pad_y: pad_y as f32,
        ow,
        oh,
        center_x: (ow - c.scale) as f32 * 0.5,
        center_y: (oh - c.scale) as f32 * 0.5,
        seed: c.seed,
    };

    let frames = c.fps * c.seconds;
    let ts: Vec<f32> = (0..frames)
        .map(|f| ((f as f32 / (frames - 1) as f32 - 0.07) / 0.81).clamp(0., 1.))
        .collect();
    let mut starts: Vec<u32> = vec![0];
    starts.extend((1..frames).filter(|&f| ts[f as usize] != ts[f as usize - 1]));
    let group_end = |g: usize| starts.get(g + 1).copied().unwrap_or(frames);
    let rgb = source.as_raw();
    let render = |t: f32| {
        let mut canvas = [10u8, 10, 15].repeat(ow as usize * oh as usize);
        scene.draw(map, t, |o, i| {
            canvas[o * 3..o * 3 + 3].copy_from_slice(&rgb[i * 3..i * 3 + 3]);
        });
        YUVBuffer::from_rgb8_source(RgbSliceU8::new(&canvas, (ow as usize, oh as usize)))
    };
    let encode = |bitrate: u32| -> Result<u64> {
        let mut video = Video::new(c, ow, oh, bitrate)?;
        let round = crate::threads().min(6);
        let groups = starts.len();
        for lo in (0..groups).step_by(round) {
            let batch = parallel((lo..(lo + round).min(groups)).collect(), |g| {
                (render(ts[starts[g] as usize]), group_end(g) - starts[g])
            });
            for (yuv, repeat) in &batch {
                for _ in 0..*repeat {
                    video.push(yuv)?;
                }
            }
        }
        video.finish()?;
        Ok(std::fs::metadata(c.out.join("rearrangement.mp4"))?.len())
    };

    let start = Instant::now();
    let cap = c.max_bytes as f64;
    let mut bitrate = (cap * 8. / f64::from(c.seconds) * 0.9) as u32;
    let mut best: Option<(u32, u64)> = None;
    let mut last = 0;
    for _ in 0..4 {
        let size = encode(bitrate)?;
        last = bitrate;
        if size <= c.max_bytes && best.is_none_or(|(_, s)| size > s) {
            best = Some((bitrate, size));
        }
        if size <= c.max_bytes && size as f64 >= cap * 0.85 {
            break;
        }
        bitrate = (f64::from(bitrate) * cap * 0.93 / size as f64) as u32;
    }
    let (bitrate, size) = best.ok_or("could not fit the mp4 under the size cap")?;
    if bitrate != last {
        encode(bitrate)?;
    }
    eprintln!(
        "mp4: {:.2}s, {}s ({frames} frames) at {ow}x{oh}, {:.2} Mbit/s, {:.2} MB",
        start.elapsed().as_secs_f64(),
        c.seconds,
        f64::from(bitrate) / 1e6,
        size as f64 / 1e6,
    );
    Ok(())
}
