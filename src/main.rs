use palette::{Hsv, IntoColor, Srgb};
use rppal::spi::{Bus, Mode, SlaveSelect, Spi};
use smart_leds::{SmartLedsWrite, RGB8};
use std::ops::Range;
use std::pin::Pin;
use tokio::net::UdpSocket;
use tokio::time::{self, Instant, Sleep};
use ws2812_spi::hosted::Ws2812;

const NUM_LEDS: usize = 105;

struct Strip {
    stream: Ws2812<Spi>,
    leds: [RGB8; NUM_LEDS],
    pending: bool,
    rainbow: f32,
}

impl Strip {
    fn set_led(&mut self, idx: usize, c: &[u8]) {
        self.leds[idx].r = c[0];
        self.leds[idx].g = c[1];
        self.leds[idx].b = c[2];
        self.pending = true;
    }

    fn write(&mut self) -> Result<(), <ws2812_spi::hosted::Ws2812<Spi> as SmartLedsWrite>::Error> {
        self.pending = false;

        if self.rainbow != 0.0 {
            self.stream.write(self.leds.iter().enumerate().map(|(idx, led)| {
                let mut hsv: Hsv = Srgb::new(led.r, led.g, led.b).into_format().into_color();
                hsv.hue += self.rainbow * (idx as f32 / NUM_LEDS as f32);
                let rgb: Srgb = hsv.into_color();
                let out: Srgb<u8> = rgb.into_format();
                RGB8::new(out.red, out.green, out.blue)
            }))
        } else {
            self.stream.write(self.leds.iter().cloned())
        }
    }

    fn flush(&mut self) -> Result<(), <ws2812_spi::hosted::Ws2812<Spi> as SmartLedsWrite>::Error> {
        if self.pending {
            self.write()?;
        }
        Ok(())
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let spi = Spi::new(Bus::Spi0, SlaveSelect::Ss0, 3_000_000, Mode::Mode0)?;

    let mut strip = Strip {
        stream: Ws2812::new(spi),
        leds: [RGB8::default(); NUM_LEDS],
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

    let current_timeout = time::sleep(time::Duration::from_secs(86400));
    tokio::pin!(current_timeout);

    let mut audvis = false;
    let mut avleds = [RGB8::default(); NUM_LEDS];

    loop {
        tokio::select! {
            biased;
            _ = write_interval.tick() => strip.write()?,
            _ = flush_interval.tick(), if strip.pending => strip.flush()?,
            _ = fade_interval.tick(), if audvis => {
                for idx in 0..NUM_LEDS/2 {
                    avleds[idx] = avleds[idx+1].clone();
                    fade_led(&mut avleds[idx], 0.001);
                    avleds[NUM_LEDS - idx - 1] = avleds[idx];
                }
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
                        let chval = |leds: &[RGB8], range: Range<f32>, mult: f32| {
                            fn bound(f: f32) -> usize {
                                if f <= 0.0 {
                                    0
                                } else if f >= 1.0 {
                                    NUM_LEDS
                                } else {
                                    (f * NUM_LEDS as f32) as usize
                                }
                            }
                            let r = bound(range.start)..bound(range.end);
                            let weight = NUM_LEDS as f32 * (range.end - range.start) / mult;
                            let value = leds[r].iter().map(|l| {
                                let hsv: palette::Hsl = Srgb::new(l.r, l.g, l.b).into_format().into_color();
                                hsv.lightness
                            }).sum::<f32>() / weight;

                            (value * 255.0).min(255.0) as u8
                        };

                        let ctr = RGB8::new(
                            chval(&strip.leds, 0.0..0.25, 1.2),
                            chval(&strip.leds, 0.25..0.50, 0.9),
                            chval(&strip.leds, 0.50..1.0, 1.0),
                        );
                        
                        avleds[NUM_LEDS/2] = ctr;
                        strip.leds.copy_from_slice(&avleds);
                    }
                },
                b"audvis" => {
                    audvis ^= true;
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
                        .filter(|led| { led.r > 0 || led.g > 0 || led.b > 0 })
                        .for_each(|led| {
                            fade_led(led, 0.10);
                            strip.pending = true
                        });

                        if !strip.pending {
                            update_timeout(current_timeout.as_mut(), 255);
                        }
                    }
                }
            },
        }
    }
}

fn fade_led(led: &mut RGB8, factor: f32) {
    if factor <= 0.0 { return }
    if led.r > 0 {
        led.r -= 1.max((led.r as f32 * factor) as u8)
    }
    if led.g > 0 {
        led.g -= 1.max((led.g as f32 * factor) as u8)
    }
    if led.b > 0 {
        led.b -=  1.max((led.b as f32 * factor) as u8)
    }
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
