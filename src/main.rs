use palette::{Hsv, IntoColor, Srgb};
use rgb::ComponentMap;
use rgb::FromSlice;
use smart_leds::{RGB, RGB8};
use std::net::AddrParseError;
use std::ops::Range;
use std::pin::Pin;
use thiserror::Error;
use tokio::net::UdpSocket;
use tokio::time::{self, Instant, Sleep};

const AVFACT_MIN: f32 = 1_f32;
const AVFACT_MAX: f32 = 3_f32;
const HUE_HUBIP: &str = "192.168.12.49";
const HUE_USERNAME: &str = "";
const HUE_CLIENTKEY: &str = "";

type Result<T> = std::result::Result<T, Error>;

#[derive(Error, Debug)]
pub(crate) enum Error {
    #[error("SPI Error")]
    SpiError(#[from] rppal::spi::Error),
    #[error("IO Error")]
    IOError(#[from] std::io::Error),
    #[error("Format Error")]
    FormatError(#[from] std::fmt::Error),
    #[error("Image Error")]
    ImageError(#[from] image::ImageError),
    #[error("Image Error")]
    ParseError(#[from] AddrParseError),
    #[error("Reqwest Error")]
    ReqweestError(#[from] reqwest::Error),
    #[error("WebRTC Error")]
    WebRTCError(#[from] webrtc_dtls::Error),
    #[error("WebRTC Error")]
    WebRTCUtilError(#[from] webrtc_util::Error),
}

mod strip_transport;
use strip_transport::StripTransport;

struct Strip {
    stream: StripTransport,
    leds: Vec<RGB<f32>>,
    pending: bool,
    rainbow: f32,
}

impl Strip {
    fn set_led(&mut self, idx: usize, c: &[u8]) {
        let rgb: RGB8 = c.as_rgb()[0];
        self.leds[idx] = rgb.into();
        self.pending = true;
    }

    async fn write(&mut self) -> Result<()> {
        self.pending = false;

        if self.rainbow != 0.0 {
            self.stream
                .write(self.leds.iter().enumerate().map(|(idx, led)| {
                    let mut hsv: Hsv = Srgb::new(led.r, led.g, led.b).into_format().into_color();
                    hsv.hue += self.rainbow * (idx as f32 / self.leds.len() as f32);
                    let rgb: Srgb = hsv.into_color();
                    let out: Srgb<u8> = rgb.into_format();
                    RGB8::new(out.red, out.green, out.blue)
                }))
                .await
        } else {
            self.stream
                .write(self.leds.iter().map(|c| c.map(|ch| ch as u8)))
                .await
        }
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let leds: u32 = 105;

    async fn transport(mode: &str, leds: u32) -> Result<StripTransport> {
        Ok(match mode {
            "spi" => StripTransport::ws2812()?,
            "wled" => StripTransport::udp_str("192.168.12.76:21324")
                .await?
                .sample(30..75, 15),
            "rpi" => StripTransport::udp_str("192.168.12.75:21324").await?,
            "dbg" => StripTransport::debug_image(1024, leds as u32),

            "study" => StripTransport::hue(HUE_HUBIP, HUE_USERNAME, HUE_CLIENTKEY, 7)
                .await?
                .sample(40..65, 1),

            "bathroom" => StripTransport::hue(HUE_HUBIP, HUE_USERNAME, HUE_CLIENTKEY, 200)
                .await?
                .sample(30..75, 4),

            "conservatory" => StripTransport::hue(HUE_HUBIP, HUE_USERNAME, HUE_CLIENTKEY, 201)
                .await?
                .sample(30..75, 8),
            _ => panic!("unknown"),
        })
    }

    let target = StripTransport::composite(vec![
        transport("spi", leds).await?,
        transport("wled", leds).await?,
        //transport("study", leds).await?,
    ]);

    println!("Setting up strip for {:?}", target);

    let mut strip = Strip {
        stream: target,
        leds: vec![RGB::<f32>::default(); leds as usize],
        pending: false,
        rainbow: 0.0,
    };

    let sock = UdpSocket::bind("0.0.0.0:21324").await?;
    println!("Listening on {:?}", sock.local_addr()?);
    let mut buf = [0; 490 * 3 + 2];

    let mut write_interval = time::interval(time::Duration::from_secs(1));
    let mut flush_interval = time::interval(time::Duration::from_secs_f64(1.0 / 60.0));
    flush_interval.set_missed_tick_behavior(time::MissedTickBehavior::Delay);

    let mut fade_interval = time::interval(time::Duration::from_secs_f64(1.0 / 60.0));
    fade_interval.set_missed_tick_behavior(time::MissedTickBehavior::Delay);

    let current_timeout = time::sleep(time::Duration::from_secs(86400));
    tokio::pin!(current_timeout);

    let mut audvis = false;
    let mut avleds = vec![<RGB<f32>>::default(); strip.leds.len()];
    let mut avfact = 0_f32;

    loop {
        tokio::select! {
            biased;
            _ = write_interval.tick() => strip.pending = true,
            _ = flush_interval.tick(), if strip.pending => strip.write().await?,
            _ = fade_interval.tick(), if audvis => {
                audvis_tick(&mut avleds);
                if strip.leds != avleds {
                    strip.leds.copy_from_slice(&avleds);
                    strip.pending = true;
                }
            },
            Ok((len, _)) = sock.recv_from(&mut buf) => match &buf[..len] {
                [mode @ 1..=2, timeout, payload @ ..] => {
                    update_timeout(current_timeout.as_mut(), *timeout);

                    match mode {
                        1 => update_warls(&mut strip, payload),
                        2 => update_drgb(&mut strip, payload),
                        _ => println!("warn: unknown data mode {}", mode),
                    }

                    if audvis {
                        audvis_process(&strip.leds, &mut avleds, &mut avfact);
                        audvis_tick(&mut avleds);
                        fade_interval.reset();
                        strip.leds.copy_from_slice(&avleds);
                    }
                },
                b"warn" => {
                    strip.leds.iter_mut()
                        .for_each(|led| {
                            led.r = 0.;
                            led.g = 0.;
                            led.b = 255.;
                        });
                    println!("Warn!");
                    loop {
                        tokio::select! {
                            _ = flush_interval.tick(), if strip.pending => {
                                strip.write().await?;
                            },
                        _ = fade_interval.tick() => {
                                strip.leds.iter_mut()
                        .filter(|led| led.iter().any(|f| f >= 10.) )
                                    .for_each(|led| {
                                        strip.pending |= fade_led(led, 0.10)
                                    });
                                if !strip.pending { break }
                                strip.write().await?;
                            }
                        }
                    }
                    println!("Warned!");
                }
                b"hue" => {
                    if let StripTransport::Composite(mut targets) = strip.stream {
                        let has_hue = targets.iter().any(is_hue);

                        println!("targets(hue? {}): {:?}", has_hue, targets);
                        if has_hue {
                            targets.retain(|target| !is_hue(target));
                        } else {
                            targets.push(transport("study", 1).await?);
                        }
                        strip.stream = StripTransport::composite(targets);
                    }
                    println!("-> targets: {:?}", strip.stream);
                }
                b"audvis" => {
                    audvis ^= true;
                    if audvis { avfact = AVFACT_MIN; }
                    println!("AudVis: {:?}", audvis);
                }
                b"rainbow" => {
                    if strip.rainbow > 0.0 {
                        strip.rainbow = 0.0
                    } else {
                        strip.rainbow = 270.0
                    }
                    println!("Rainbow: {:?}", strip.rainbow);
                }
                [b'r', n] => {
                    strip.rainbow = 30.0 * (*n as f32);
                    println!("Rainbow: {:?}", strip.rainbow);
                }
                [b'r', d1 @ b'0'..=b'9', d2 @ b'0'..=b'9'] => {
                    let n = 10 * (d1 - b'0') + (d2 - b'0');
                    strip.rainbow = 30.0 * (n as f32);
                    println!("Rainbow: {:?}", strip.rainbow);
                }
                unhandled => println!("Unhandled data: {:?}", unhandled),
            },
            _ = &mut current_timeout => {
                tokio::select! {
                    _ = fade_interval.tick() => {
                        strip.leds.iter_mut()
                        .filter(|led| led.iter().any(|f| f >= 0_f32) )
                        .for_each(|led| strip.pending |= fade_led(led, 0.10));

                        if !strip.pending {
                            update_timeout(current_timeout.as_mut(), 255);
                            avleds.copy_from_slice(&strip.leds);
                        }
                    }
                }
            },
        }
    }
}

fn is_hue(target: &StripTransport) -> bool {
    if let StripTransport::Sampled(s) = target {
        matches!(s.base.as_ref(), StripTransport::Hue(_))
    } else {
        matches!(target, StripTransport::Hue(_))
    }
}

fn fade_led(led: &mut RGB<f32>, factor: f32) -> bool {
    if factor <= 0.0 {
        return false;
    }
    let f = led.iter().reduce(f32::max).unwrap_or_default() * factor;

    let updated = led.map(|ch| 0.001_f32.max(ch - f));
    let changed = updated != *led;
    *led = updated;
    changed
}

fn update_timeout(timeout: Pin<&mut Sleep>, duration_secs: u8) {
    match duration_secs {
        255 => timeout.reset(Instant::now() + time::Duration::from_secs(86400)),
        _ => timeout.reset(Instant::now() + time::Duration::from_secs(duration_secs as u64)),
    };
}

fn update_warls(strip: &mut Strip, buf: &[u8]) {
    buf.chunks(4).for_each(|c| match c[0] as usize {
        i if i < strip.leds.len() => strip.set_led(i, &c[1..]),
        _ => (),
    });
}

fn update_drgb(strip: &mut Strip, buf: &[u8]) {
    strip.pending = true;
    buf.chunks(3)
        .take(strip.leds.len())
        .enumerate()
        .for_each(|(i, c)| strip.set_led(i, c));
}

fn ratio_range(range: Range<f32>, length: usize) -> Range<usize> {
    let bound = |f: f32| {
        if f <= 0.0 {
            0
        } else if f >= 1.0 {
            length
        } else {
            (f * length as f32) as usize
        }
    };
    bound(range.start)..bound(range.end)
}

fn audvis_process(rawleds: &[RGB<f32>], avleds: &mut [RGB<f32>], avfact: &mut f32) {
    let chval = |leds: &[RGB<f32>], range: Range<f32>| {
        let r = ratio_range(range, rawleds.len());
        let weight = (r.end - r.start) as f32;
        leds[r].iter().map(|l| l.r).sum::<f32>() / weight
    };

    let ctr = <RGB<f32>>::new(
        chval(&rawleds, 0.0..0.15) * 1.2 * *avfact,
        chval(&rawleds, 0.25..0.50) * 0.9 * *avfact,
        chval(&rawleds, 0.50..1.0) * *avfact,
    );

    *avfact += 0.001;
    let chmax = 255_f32 / ctr.iter().reduce(f32::max).unwrap_or_default();
    *avfact = avfact.min(chmax).clamp(AVFACT_MIN, AVFACT_MAX);

    avleds[rawleds.len() / 2] = ctr;
}

fn audvis_tick(avleds: &mut [RGB<f32>]) {
    let l = avleds.len();
    for idx in 0..l / 2 {
        avleds[idx] = avleds[idx + 1].clone();
        fade_led(&mut avleds[idx], 1.0 / (l) as f32);
        avleds[avleds.len() - idx - 1] = avleds[idx];
    }
}
