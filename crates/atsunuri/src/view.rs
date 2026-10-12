/// How the canvas is placed in the window, in physical (buffer) pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    /// Window position of the canvas's top-left corner.
    pub offset: [f32; 2],
    /// Physical pixels per canvas pixel.
    pub scale: f32,
}

impl View {
    /// Centers the canvas in the window, shrinking it to fit if needed but
    /// never enlarging it past 1:1.
    pub fn fit(canvas: (u32, u32), window: (u32, u32)) -> View {
        let (cw, ch) = (canvas.0 as f32, canvas.1 as f32);
        let (ww, wh) = (window.0 as f32, window.1 as f32);
        let scale = (ww / cw).min(wh / ch).min(1.0);
        // Whole-pixel offsets keep the canvas sharp at 1:1.
        let offset = [
            ((ww - cw * scale) / 2.0).round(),
            ((wh - ch * scale) / 2.0).round(),
        ];
        View { offset, scale }
    }

    pub fn window_to_canvas(&self, x: f32, y: f32) -> (f32, f32) {
        (
            (x - self.offset[0]) / self.scale,
            (y - self.offset[1]) / self.scale,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_canvas_is_centered_at_one_to_one() {
        let view = View::fit((100, 50), (300, 250));
        assert_eq!(
            view,
            View {
                offset: [100.0, 100.0],
                scale: 1.0
            }
        );
        assert_eq!(view.window_to_canvas(100.0, 100.0), (0.0, 0.0));
        assert_eq!(view.window_to_canvas(150.5, 125.0), (50.5, 25.0));
    }

    #[test]
    fn large_canvas_is_shrunk_to_fit() {
        let view = View::fit((2000, 1000), (1000, 1000));
        assert_eq!(view.scale, 0.5);
        assert_eq!(view.offset, [0.0, 250.0]);
        assert_eq!(view.window_to_canvas(1000.0, 750.0), (2000.0, 1000.0));
    }
}
