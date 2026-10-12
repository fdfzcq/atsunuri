use crate::Rect;

/// A single-layer RGBA8 canvas, stored row-major with straight (non-premultiplied)
/// sRGB-encoded color. The canvas is always fully opaque.
pub struct Canvas {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
    dirty: Option<Rect>,
}

impl Canvas {
    pub const BACKGROUND: [u8; 4] = [255, 255, 255, 255];

    /// Creates a white canvas. The whole canvas starts out dirty so the first
    /// upload covers it.
    pub fn new(width: u32, height: u32) -> Self {
        assert!(width > 0 && height > 0, "canvas must not be empty");
        let mut canvas = Canvas {
            width,
            height,
            pixels: Vec::new(),
            dirty: None,
        };
        canvas.clear();
        canvas
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// Raw RGBA8 pixel data, `width * height * 4` bytes.
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = self.index(x, y);
        self.pixels[i..i + 4].try_into().unwrap()
    }

    /// Resets the canvas to white.
    pub fn clear(&mut self) {
        self.pixels = Self::BACKGROUND.repeat((self.width * self.height) as usize);
        self.dirty = Some(self.bounds());
    }

    pub fn bounds(&self) -> Rect {
        Rect {
            x0: 0,
            y0: 0,
            x1: self.width,
            y1: self.height,
        }
    }

    /// Returns the region changed since the last call, if any.
    pub fn take_dirty(&mut self) -> Option<Rect> {
        self.dirty.take()
    }

    pub(crate) fn mark_dirty(&mut self, rect: Rect) {
        if rect.is_empty() {
            return;
        }
        self.dirty = Some(match self.dirty {
            Some(d) => d.union(&rect),
            None => rect,
        });
    }

    /// Composites `color` over the pixel at `(x, y)` with coverage `alpha` in `[0, 1]`.
    pub(crate) fn blend(&mut self, x: u32, y: u32, color: [u8; 3], alpha: f32) {
        let i = self.index(x, y);
        for (dst, src) in self.pixels[i..i + 3].iter_mut().zip(color) {
            let (d, s) = (*dst as f32, src as f32);
            *dst = (d + (s - d) * alpha).round() as u8;
        }
    }

    pub(crate) fn set_rgb(&mut self, x: u32, y: u32, color: [u8; 3]) {
        let i = self.index(x, y);
        self.pixels[i..i + 3].copy_from_slice(&color);
    }

    fn index(&self, x: u32, y: u32) -> usize {
        debug_assert!(x < self.width && y < self.height);
        ((y * self.width + x) * 4) as usize
    }
}
