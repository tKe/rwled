use std::pin::Pin;

use rppal::spi::{Bus, Mode, SlaveSelect, Spi};
use smart_leds::{SmartLedsWrite, RGB8};
use tokio::net::UdpSocket;
use tokio::time::{self, Instant, Sleep};
use ws2812_spi::hosted::Ws2812;

const NUM_LEDS: usize = 105;

struct Strip {
    stream: Ws2812<Spi>,
    leds: [RGB8; NUM_LEDS],
    pending: bool,
}

impl Strip {
    fn set_led(&mut self, idx: usize, c: &[u8]) {
        self.pending = true;
        self.leds[idx].r = c[0];
        self.leds[idx].g = c[1];
        self.leds[idx].b = c[2];
    }

    fn flush(&mut self) -> Result<(), <ws2812_spi::hosted::Ws2812<Spi> as SmartLedsWrite>::Error> {
        if self.pending {
            self.stream.write(self.leds.iter().cloned())?;
            self.pending = false;
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
    };

    let sock = UdpSocket::bind("0.0.0.0:21324").await?;
    println!("Listening on {:?}", sock.local_addr()?);
    let mut buf = [0; 490 * 3 + 2];

    let mut write_rate = time::interval(time::Duration::from_secs_f64(1.0 / 120.0));
    write_rate.set_missed_tick_behavior(time::MissedTickBehavior::Delay);

    let mut fade_rate = time::interval(time::Duration::from_secs_f64(1.0 / 30.0));
    fade_rate.set_missed_tick_behavior(time::MissedTickBehavior::Delay);

    let current_timeout = time::sleep(time::Duration::from_secs(86400));
    tokio::pin!(current_timeout);

    loop {
        tokio::select! {
            biased;
            _ = write_rate.tick(), if strip.pending => strip.flush()?,
            Ok((len, _)) = sock.recv_from(&mut buf) => match buf {
                [1, timeout, ..] => {
                    update_timeout(current_timeout.as_mut(), timeout);
                    update_warls(&mut strip, &buf[2..len]);
                },
                [2, timeout, ..] => {
                    update_timeout(current_timeout.as_mut(), timeout);
                    update_drgb(&mut strip, &buf[2..len]);
                },
                _ => println!("Unhandled data type {:?}", buf[0]),
            },
            _ = &mut current_timeout => {
                tokio::select! {
                    _ = fade_rate.tick() => {
                        strip.leds.iter_mut()
                        .filter(|led| { led.r > 0 || led.g > 0 || led.b > 0 })
                        .for_each(|led| {
                            if led.r > 0 { led.r -= u8::max(1, (led.r as f32 * 0.10) as u8) }
                            if led.g > 0 { led.g -= u8::max(1, (led.g as f32 * 0.10) as u8) }
                            if led.b > 0 { led.b -= u8::max(1, (led.b as f32 * 0.10) as u8) }
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
