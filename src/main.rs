use rppal::spi::{Bus, Mode, SlaveSelect, Spi};
use smart_leds::{SmartLedsWrite, RGB8};
use tokio::net::UdpSocket;
use ws2812_spi::hosted::Ws2812;

const NUM_LEDS: usize = 105;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let spi = Spi::new(Bus::Spi0, SlaveSelect::Ss0, 3_000_000, Mode::Mode0)?;
    let mut ws = Ws2812::new(spi);
    let mut leds = [RGB8::default(); NUM_LEDS];

    let sock = UdpSocket::bind("0.0.0.0:21324").await?;
    println!("Listening on {:?}", sock.local_addr()?);
    let mut buf = [0; 490 * 3 + 2];

    loop {
        let (len, _) = sock.recv_from(&mut buf).await?;

        match buf {
            [1, _, ..] => update_warls(&mut leds, &buf[2..len]),
            [2, _, ..] => update_drgb(&mut leds, &buf[2..len]),
            _ => println!("Unhandled data type {:?}", buf[0]),
        }

        ws.write(leds.iter().cloned())?;
    }
}

fn update_warls(leds: &mut [RGB8], buf: &[u8]) {
    buf.chunks(4).for_each(|c| match c[0] as usize {
        i if i < leds.len() => {
            leds[i].r = c[1];
            leds[i].g = c[2];
            leds[i].b = c[3];
        }
        _ => (),
    });
}

fn update_drgb(leds: &mut [RGB8], buf: &[u8]) {
    buf.chunks(3)
        .take(leds.len())
        .enumerate()
        .for_each(|(i, c)| {
            leds[i].r = c[0];
            leds[i].g = c[1];
            leds[i].b = c[2];
        });
}
