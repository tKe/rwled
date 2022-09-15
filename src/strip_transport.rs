use super::Result;

use async_trait::async_trait;
use rppal::spi::{Bus, Mode, SlaveSelect, Spi};
use smart_leds::{SmartLedsWrite, RGB8};
use std::ops::Range;
use std::str::FromStr;
use ws2812_spi::hosted::Ws2812;

mod dbgimg;
mod huee;
mod udpstrip;

pub(super) enum StripTransport {
    Ws2812(Ws2812<Spi>),
    Hue(huee::Hue),
    Udp(udpstrip::UdpStrip),
    DebugImage(dbgimg::DebugImage),
    Composite(Vec<StripTransport>),
    Sampled(SampledStripTransport),
}

impl std::fmt::Debug for StripTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StripTransport::Ws2812(_) => f.write_str("ws2812"),
            StripTransport::Hue(h) => f.write_str(format!("hue:{}", h.desc).as_str()),
            StripTransport::Udp(u) => f.write_str(format!("udp:{:?}", u.dest).as_str()),
            StripTransport::DebugImage(_) => f.write_str("dbg"),
            StripTransport::Composite(c) => f.write_str(format!("({:?})", c).as_str()),
            StripTransport::Sampled(s) => {
                f.write_str(format!("{:?}[{:?}:{:?}]", s.base, s.range, s.count).as_str())
            }
        }
    }
}

pub(crate) struct SampledStripTransport {
    base: Box<StripTransport>,
    range: Range<usize>,
    count: usize,
}

impl SampledStripTransport {
    async fn write<T, I>(&mut self, iterator: T) -> Result<()>
    where
        T: Iterator<Item = I>,
        I: Into<RGB8>,
    {
        let r = self.range.start..self.range.end;
        let scaled = scale(iterator, r, self.count);
        self.base
            .write_base(scaled.iter().map(|ch| ch.map(|c| c as u8)))
            .await
    }
}

#[allow(dead_code)]
impl StripTransport {
    pub(crate) fn ws2812() -> Result<Self> {
        let spi = Spi::new(Bus::Spi0, SlaveSelect::Ss0, 3_000_000, Mode::Mode0)?;
        Ok(Self::Ws2812(Ws2812::new(spi)))
    }

    pub(crate) async fn hue(
        hub_ip: &str,
        username: &str,
        clientkey: &str,
        group: u16,
    ) -> Result<Self> {
        Ok(Self::Hue(
            huee::Hue::new(hub_ip, username, clientkey, group).await?,
        ))
    }

    pub(crate) async fn udp(dest: std::net::SocketAddr) -> Result<Self> {
        Ok(Self::Udp(udpstrip::UdpStrip::new(dest).await?))
    }

    pub(crate) async fn udp_str(dest: &str) -> Result<Self> {
        Self::udp(std::net::SocketAddr::V4(std::net::SocketAddrV4::from_str(
            dest,
        )?))
        .await
    }

    pub(crate) fn debug_image(width: u32, height: u32) -> Self {
        Self::DebugImage(dbgimg::DebugImage::new(width, height))
    }

    pub async fn write<T, I>(&mut self, iterator: T) -> Result<()>
    where
        T: Iterator<Item = I> + Send + Clone,
        I: Into<RGB8>,
    {
        match self {
            StripTransport::Composite(s) => {
                futures::future::try_join_all(
                    s.iter_mut().map(|t| t.write_single(iterator.clone())),
                )
                .await?;
            }
            _ => self.write_single(iterator).await?,
        }
        Ok(())
    }

    async fn write_base<T, I>(&mut self, iterator: T) -> Result<()>
    where
        T: Iterator<Item = I> + Send + Clone,
        I: Into<RGB8>,
    {
        match self {
            StripTransport::Ws2812(s) => s.write(iterator)?,
            StripTransport::Hue(s) => s.write(iterator).await?,
            StripTransport::Udp(s) => s.write(iterator).await?,
            StripTransport::DebugImage(i) => i.write(iterator.map(|f| {
                let i = f.into();
                [i.r, i.g, i.b]
            }))?,
            _ => panic!("nope {:?}", self),
        }
        Ok(())
    }

    async fn write_single<T, I>(&mut self, iterator: T) -> Result<()>
    where
        T: Iterator<Item = I> + Send + Clone,
        I: Into<RGB8>,
    {
        match self {
            StripTransport::Sampled(s) => s.write(iterator.map(|x| x.into())).await?,
            _ => self.write_base(iterator).await?,
        }
        Ok(())
    }

    pub(crate) fn composite(transports: Vec<StripTransport>) -> Self {
        StripTransport::Composite(transports)
    }

    pub(crate) fn sample(self, range: Range<usize>, count: usize) -> Self {
        match self {
            StripTransport::Composite(_) => panic!("not permitted"),
            StripTransport::Sampled(_) => panic!("not permitted"),
            _ => Self::Sampled(SampledStripTransport {
                base: Box::new(self),
                range,
                count,
            }),
        }
    }
}

fn scale<T, I>(iterator: T, range: Range<usize>, leds: usize) -> Vec<[u8; 3]>
where
    T: Iterator<Item = I>,
    I: Into<RGB8>,
{
    let src = iterator
        .skip(range.start)
        .map(|x| {
            let i = x.into();
            [i.r as f32, i.g as f32, i.b as f32]
        })
        .collect::<Vec<_>>();
    let scaled = src
        .chunks(range.len() / leds)
        .take(leds)
        .map(|chunk| {
            chunk
                .iter()
                .fold([0_f32; 3], |mut out, v| {
                    out[0] += v[0];
                    out[1] += v[1];
                    out[2] += v[2];
                    out
                })
                .map(|c| (c / chunk.len() as f32) as u8)
        })
        .collect::<Vec<_>>();
    scaled
}

#[async_trait]
pub(crate) trait AsyncSmartLedsWrite {
    type Error;
    type Color;
    async fn write<T, I>(&mut self, iterator: T) -> std::result::Result<(), Self::Error>
    where
        T: Iterator<Item = I>,
        T: Send,
        I: Into<Self::Color>;
}
