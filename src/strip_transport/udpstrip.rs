use std::net::SocketAddr;

use rgb::RGB8;
use tokio::net::UdpSocket;

use super::AsyncSmartLedsWrite;
use crate::{Error, Result};
use async_trait::async_trait;

pub struct UdpStrip {
    pub(crate) dest: SocketAddr,
    sock: UdpSocket,
    buf: [u8; 2 + 3 * 490],
}

impl UdpStrip {
    pub(crate) async fn new(dest: std::net::SocketAddr) -> Result<Self> {
        let sock = tokio::net::UdpSocket::bind("0.0.0.0:0").await?;
        Ok(UdpStrip {
            sock,
            dest,
            buf: [u8::default(); 1472],
        })
    }
}

#[async_trait]
impl AsyncSmartLedsWrite for UdpStrip {
    type Error = Error;
    type Color = RGB8;

    async fn write<T, I>(&mut self, iterator: T) -> Result<()>
    where
        T: Iterator<Item = I> + Send,
        I: Into<Self::Color>,
    {
        let mut len = 2;
        self.buf[0] = 2;
        self.buf[1] = 5;
        self.buf[2..]
            .iter_mut()
            .zip(iterator.flat_map(|item| {
                len += 3;
                let i = item.into();
                [i.r, i.g, i.b]
            }))
            .for_each(|(dst, itm)| *dst = itm);
        self.sock.send_to(&self.buf[..len], self.dest).await?;
        Ok(())
    }
}
