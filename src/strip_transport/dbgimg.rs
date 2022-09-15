use image::{ImageError, ImageResult, Rgb};
use smart_leds::SmartLedsWrite;

pub struct DebugImage {
    img: image::RgbImage,
    y: u32,
    n: u32,
}

impl DebugImage {
    pub(crate) fn new(width: u32, height: u32) -> Self {
        DebugImage {
            img: image::RgbImage::new(width, height),
            n: 0,
            y: 0,
        }
    }
}

impl SmartLedsWrite for DebugImage {
    type Error = ImageError;
    type Color = Rgb<u8>;

    fn write<T, I>(&mut self, iterator: T) -> ImageResult<()>
    where
        T: Iterator<Item = I>,
        I: Into<Self::Color>,
    {
        iterator
            .enumerate()
            .for_each(|(i, l)| self.img.put_pixel(self.y, i as u32, l.into()));

        self.y = (self.y + 1) % self.img.width();
        if self.y == 0 {
            let file = &format!("dbg/dbg-{:03}.png", self.n);
            self.img.save(file)?;
            println!("Saved {}", file);
            self.n += 1;
        };
        Ok(())
    }
}
