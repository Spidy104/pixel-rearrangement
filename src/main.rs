use pixel_rearrangement::{Config, Result, run};

fn main() -> Result<()> {
    run(&Config {
        source: "1000071110.jpg".into(),
        target: "kali-clean.png".into(),
        fps: 25,
        seconds: 12,
        ..Config::default()
    })
}
