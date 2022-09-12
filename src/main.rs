use palette::{Hsv, IntoColor, Srgb};
use rgb::{ComponentMap, FromSlice};
use rppal::spi::{Bus, Mode, SlaveSelect, Spi};
use smart_leds::{SmartLedsWrite, RGB, RGB8};
use std::ops::Range;
use std::pin::Pin;
use std::str::FromStr;
use tokio::net::UdpSocket;
use tokio::time::{self, Instant, Sleep};
use ws2812_spi::hosted::Ws2812;

const NUM_LEDS: usize = 105;
const DBGIMG_W: u32 = 1576;
const AVFACT_MIN: f32 = 1_f32;
const AVFACT_MAX: f32 = 3_f32;

struct Strip {
    stream: Ws2812<Spi>,
    leds: [RGB<f32>; NUM_LEDS],
    pending: bool,
    rainbow: f32,
}

impl Strip {
    fn set_led(&mut self, idx: usize, c: &[u8]) {
        let rgb: RGB8 = c.as_rgb()[0];
        self.leds[idx] = rgb.into();
        self.pending = true;
    }

    fn write(&mut self) -> Result<(), <ws2812_spi::hosted::Ws2812<Spi> as SmartLedsWrite>::Error> {
        self.pending = false;

        if self.rainbow != 0.0 {
            self.stream
                .write(self.leds.iter().enumerate().map(|(idx, led)| {
                    let mut hsv: Hsv = Srgb::new(led.r, led.g, led.b).into_format().into_color();
                    hsv.hue += self.rainbow * (idx as f32 / NUM_LEDS as f32);
                    let rgb: Srgb = hsv.into_color();
                    let out: Srgb<u8> = rgb.into_format();
                    RGB8::new(out.red, out.green, out.blue)
                }))
        } else {
            self.stream
                .write(self.leds.iter().map(|c| c.map(|ch| ch as u8)))
        }
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let spi = Spi::new(Bus::Spi0, SlaveSelect::Ss0, 3_000_000, Mode::Mode0)?;

    let mut strip = Strip {
        stream: Ws2812::new(spi),
        leds: [RGB {
            r: 0_f32,
            g: 0.,
            b: 0.,
        }; NUM_LEDS],
        pending: false,
        rainbow: 0.0,
    };

    let sock = UdpSocket::bind("0.0.0.0:21324").await?;
    println!("Listening on {:?}", sock.local_addr()?);
    let mut buf = [0; 490 * 3 + 2];

    let mut write_interval = time::interval(time::Duration::from_secs(1));
    let mut flush_interval = time::interval(time::Duration::from_secs_f64(1.0 / 120.0));
    flush_interval.set_missed_tick_behavior(time::MissedTickBehavior::Delay);

    let mut fade_interval = time::interval(time::Duration::from_secs_f64(1.0 / 60.0));
    fade_interval.set_missed_tick_behavior(time::MissedTickBehavior::Delay);

    let mut proxy_interval = time::interval(time::Duration::from_secs_f64(1.0 / 20.0));
    proxy_interval.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
    let proxy: Option<std::net::SocketAddr> = Some(std::net::SocketAddr::V4(
        std::net::SocketAddrV4::from_str("192.168.12.76:21324")?,
    ));

    let current_timeout = time::sleep(time::Duration::from_secs(86400));
    tokio::pin!(current_timeout);

    let mut audvis = false;
    let mut avleds = [<RGB<f32>>::default(); NUM_LEDS];
    let mut avfact = 0_f32;

    let dbg = false;
    let mut dbgimg = image::RgbImage::new(DBGIMG_W, NUM_LEDS as u32);
    let mut dbgimg_y = 0;
    let mut dbgimg_n = 0;

    loop {
        tokio::select! {
            biased;
            _ = write_interval.tick() => strip.pending = true,
            _ = flush_interval.tick(), if strip.pending => {
                strip.write()?;
                match proxy {
                    Some(dest) => proxy_strip(&strip, &sock, dest).await?,
                _ => ()
                }
            },
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
                        _ => (),
                    }

                    if audvis {
                        audvis_process(&strip.leds, &mut avleds, &mut avfact);
                        audvis_tick(&mut avleds);
                        fade_interval.reset();
                        strip.leds.copy_from_slice(&avleds);
                    }

                    if dbg {
                        strip.leds.iter().enumerate()
                            .for_each(|(i, l)| dbgimg.put_pixel(dbgimg_y, i as u32, image::Rgb([l.r as u8, l.g as u8, l.b as u8])));
                        dbgimg_y = (dbgimg_y + 1) % DBGIMG_W;
                        if dbgimg_y == 0 {
                            let file = &format!("dbg/dbg-{:03}.png", dbgimg_n);
                            dbgimg.save(file)?;
                            println!("Saved {}", file);
                            dbgimg_n += 1;
                        }
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
                                strip.write()?;
                                match proxy {
                                    Some(dest) => proxy_strip(&strip, &sock, dest).await?,
                                _ => ()
                                }
                            },
                        _ = fade_interval.tick() => {
                                strip.leds.iter_mut()
                                    .filter(|led| led.iter().any(|f| f >= 10.) )
                                    .for_each(|led| {
                                        strip.pending |= fade_led(led, 0.10)
                                    });
                                if !strip.pending { break }
                                strip.write()?;
                            }
                        }
                    }
                    println!("Warned!");
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
                _ => println!("Unhandled data type {:?}", buf[0]),
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

async fn proxy_strip(
    strip: &Strip,
    sock: &UdpSocket,
    dest: std::net::SocketAddr,
) -> Result<(), std::io::Error> {
    let mut pxybuf = [u8::default(); 2 + 15 * 3];
    let srcleds = &strip.leds[30..NUM_LEDS - 30];
    pxybuf[0] = 2;
    pxybuf[1] = 5;
    pxybuf[2..]
        .iter_mut()
        .zip(
            srcleds
                .chunks(srcleds.len() / 15)
                .map(|leds| {
                    leds.iter()
                        .fold([0_f32; 3], |mut out, led| {
                            out[0] += led.r;
                            out[1] += led.g;
                            out[2] += led.b;
                            out
                        })
                        .map(|c| (c / leds.len() as f32) as u8)
                })
                .flatten(),
        )
        .for_each(|(dst, itm)| *dst = itm);
    sock.send_to(&pxybuf, dest).await?;
    Ok(())
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
            NUM_LEDS
        } else {
            (f * length as f32) as usize
        }
    };
    bound(range.start)..bound(range.end)
}

fn audvis_process(rawleds: &[RGB<f32>], avleds: &mut [RGB<f32>], avfact: &mut f32) {
    let chval = |leds: &[RGB<f32>], range: Range<f32>| {
        let r = ratio_range(range, NUM_LEDS);
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

    avleds[NUM_LEDS / 2] = ctr;
}

fn audvis_tick(avleds: &mut [RGB<f32>]) {
    for idx in 0..NUM_LEDS / 2 {
        avleds[idx] = avleds[idx + 1].clone();
        fade_led(&mut avleds[idx], 0.02);
        avleds[NUM_LEDS - idx - 1] = avleds[idx];
    }
}
