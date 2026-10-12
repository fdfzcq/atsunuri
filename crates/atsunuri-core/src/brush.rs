use crate::{Canvas, Rect};

/// One input sample in canvas pixel coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InputPoint {
    pub x: f32,
    pub y: f32,
    /// Normalized pen pressure in `[0, 1]`. Mice report `1.0`.
    pub pressure: f32,
}

/// A round brush with pressure-controlled size.
#[derive(Debug, Clone, PartialEq)]
pub struct Brush {
    /// Diameter in canvas pixels at full pressure.
    pub size: f32,
    /// Fraction of `size` used at zero pressure.
    pub min_size: f32,
    /// How much of the radius is fully opaque before the edge falls off, in `[0, 1]`.
    pub hardness: f32,
    /// Distance between dabs as a fraction of the current diameter.
    pub spacing: f32,
    /// Maximum opacity of a stroke, in `[0, 1]`. Overlapping dabs within one
    /// stroke never build up past this.
    pub opacity: f32,
    pub color: [u8; 3],
}

impl Default for Brush {
    fn default() -> Self {
        Brush {
            size: 8.0,
            min_size: 0.1,
            hardness: 0.8,
            spacing: 0.1,
            opacity: 1.0,
            color: [0, 0, 0],
        }
    }
}

impl Brush {
    fn radius(&self, pressure: f32) -> f32 {
        let p = pressure.clamp(0.0, 1.0);
        let fraction = self.min_size + (1.0 - self.min_size) * p;
        self.size * fraction * 0.5
    }

    fn step(&self, pressure: f32) -> f32 {
        (self.radius(pressure) * 2.0 * self.spacing).max(0.5)
    }
}

/// A stroke in progress.
///
/// Dabs are combined into a per-stroke coverage buffer by taking the maximum,
/// then the stroke is composited over the canvas as it was when the stroke
/// began. This keeps edges anti-aliased and stops a stroke from darkening
/// where it overlaps itself.
pub struct Stroke {
    brush: Brush,
    last: InputPoint,
    /// Distance travelled since the last dab.
    since_last_dab: f32,
    /// Canvas pixels from before the stroke started.
    original: Vec<u8>,
    /// Per-pixel stroke coverage in `[0, 1]`.
    coverage: Vec<f32>,
}

impl Stroke {
    /// Starts a stroke and paints its first dab.
    pub fn begin(canvas: &mut Canvas, brush: Brush, point: InputPoint) -> Self {
        let mut stroke = Stroke {
            brush,
            last: point,
            since_last_dab: 0.0,
            original: canvas.pixels().to_vec(),
            coverage: vec![0.0; (canvas.width() * canvas.height()) as usize],
        };
        stroke.dab(canvas, point);
        stroke
    }

    /// Extends the stroke to `point`, placing dabs along the way.
    pub fn add(&mut self, canvas: &mut Canvas, point: InputPoint) {
        let a = self.last;
        let (dx, dy) = (point.x - a.x, point.y - a.y);
        let length = (dx * dx + dy * dy).sqrt();
        self.last = point;
        if length <= f32::EPSILON {
            return;
        }

        let at = |d: f32| {
            let t = d / length;
            InputPoint {
                x: a.x + dx * t,
                y: a.y + dy * t,
                pressure: a.pressure + (point.pressure - a.pressure) * t,
            }
        };

        let mut travelled = 0.0;
        loop {
            let needed = self.brush.step(at(travelled).pressure) - self.since_last_dab;
            if travelled + needed > length {
                self.since_last_dab += length - travelled;
                break;
            }
            travelled += needed;
            self.since_last_dab = 0.0;
            self.dab(canvas, at(travelled));
        }
    }

    fn dab(&mut self, canvas: &mut Canvas, p: InputPoint) {
        let brush = &self.brush;
        let radius = brush.radius(p.pressure);
        // Dabs smaller than a pixel are drawn at half a pixel radius and faded
        // by their area instead, so light pressure gives thin, light lines.
        let r = radius.max(0.5);
        let strength = brush.opacity * (radius / r).powi(2);
        let inner = r * brush.hardness.clamp(0.0, 1.0);

        let x0 = (p.x - r - 1.0).floor().max(0.0) as u32;
        let y0 = (p.y - r - 1.0).floor().max(0.0) as u32;
        let x1 = ((p.x + r + 1.0).ceil().max(0.0) as u32).min(canvas.width());
        let y1 = ((p.y + r + 1.0).ceil().max(0.0) as u32).min(canvas.height());
        let area = Rect { x0, y0, x1, y1 };
        if area.is_empty() {
            return;
        }

        let width = canvas.width();
        for y in y0..y1 {
            for x in x0..x1 {
                let (fx, fy) = (x as f32 + 0.5 - p.x, y as f32 + 0.5 - p.y);
                let d = (fx * fx + fy * fy).sqrt();
                let edge = (r - d + 0.5).clamp(0.0, 1.0);
                let falloff = if d <= inner {
                    1.0
                } else {
                    let t = (1.0 - (d - inner) / (r - inner)).clamp(0.0, 1.0);
                    t * t * (3.0 - 2.0 * t)
                };
                let alpha = edge * falloff * strength;

                let i = (y * width + x) as usize;
                if alpha > self.coverage[i] {
                    self.coverage[i] = alpha;
                    let o = i * 4;
                    let original = [self.original[o], self.original[o + 1], self.original[o + 2]];
                    canvas.set_rgb(x, y, original);
                    canvas.blend(x, y, brush.color, alpha);
                }
            }
        }
        canvas.mark_dirty(area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(x: f32, y: f32, pressure: f32) -> InputPoint {
        InputPoint { x, y, pressure }
    }

    fn is_white(c: [u8; 4]) -> bool {
        c == Canvas::BACKGROUND
    }

    #[test]
    fn dab_paints_center_and_leaves_far_pixels_white() {
        let mut canvas = Canvas::new(64, 64);
        canvas.take_dirty();
        Stroke::begin(&mut canvas, Brush::default(), pt(32.0, 32.0, 1.0));
        assert_eq!(canvas.pixel(32, 32), [0, 0, 0, 255]);
        assert!(is_white(canvas.pixel(0, 0)));
        assert!(is_white(canvas.pixel(40, 32)));

        let dirty = canvas.take_dirty().expect("dab should mark dirty region");
        assert!(dirty.x0 <= 28 && dirty.x1 >= 36 && dirty.y0 <= 28 && dirty.y1 >= 36);
    }

    #[test]
    fn stroke_has_no_gaps() {
        let mut canvas = Canvas::new(200, 20);
        let mut stroke = Stroke::begin(&mut canvas, Brush::default(), pt(10.0, 10.0, 1.0));
        // Sparse samples, as from a fast pen movement.
        stroke.add(&mut canvas, pt(100.0, 10.0, 1.0));
        stroke.add(&mut canvas, pt(190.0, 10.0, 1.0));
        for x in 10..190 {
            assert_eq!(canvas.pixel(x, 10), [0, 0, 0, 255], "gap at x = {x}");
        }
    }

    #[test]
    fn pressure_controls_width() {
        let width_at = |pressure| {
            let mut canvas = Canvas::new(100, 100);
            let brush = Brush {
                size: 20.0,
                ..Brush::default()
            };
            let mut stroke = Stroke::begin(&mut canvas, brush, pt(10.0, 50.5, pressure));
            stroke.add(&mut canvas, pt(90.0, 50.5, pressure));
            (0..100).filter(|&y| !is_white(canvas.pixel(50, y))).count()
        };
        let light = width_at(0.2);
        let full = width_at(1.0);
        assert!(light > 0, "light pressure should still draw");
        assert!(full > light * 2, "full {full} vs light {light}");
    }

    #[test]
    fn overlapping_soft_edges_do_not_build_up() {
        let brush = Brush {
            size: 20.0,
            hardness: 0.0,
            ..Brush::default()
        };
        let mut canvas = Canvas::new(100, 100);
        Stroke::begin(&mut canvas, brush.clone(), pt(50.0, 50.0, 1.0));
        let once = canvas.pixel(56, 50);

        let mut canvas = Canvas::new(100, 100);
        let mut stroke = Stroke::begin(&mut canvas, brush, pt(50.0, 50.0, 1.0));
        // Scribble back and forth over the same spot.
        for _ in 0..10 {
            stroke.add(&mut canvas, pt(50.0, 51.0, 1.0));
            stroke.add(&mut canvas, pt(50.0, 50.0, 1.0));
        }
        let scribbled = canvas.pixel(56, 50);
        assert!(once[0] < 255);
        assert!(
            scribbled[0] + 10 >= once[0],
            "once {once:?} vs scribbled {scribbled:?}"
        );
    }

    #[test]
    fn separate_strokes_do_build_up() {
        let brush = Brush {
            opacity: 0.5,
            ..Brush::default()
        };
        let mut canvas = Canvas::new(32, 32);
        Stroke::begin(&mut canvas, brush.clone(), pt(16.0, 16.0, 1.0));
        let first = canvas.pixel(16, 16)[0];
        Stroke::begin(&mut canvas, brush, pt(16.0, 16.0, 1.0));
        let second = canvas.pixel(16, 16)[0];
        assert!(second < first);
    }

    #[test]
    fn dabs_outside_the_canvas_are_clipped() {
        let mut canvas = Canvas::new(16, 16);
        canvas.take_dirty();
        let mut stroke = Stroke::begin(&mut canvas, Brush::default(), pt(-50.0, -50.0, 1.0));
        stroke.add(&mut canvas, pt(-50.0, 100.0, 1.0));
        assert!(canvas.take_dirty().is_none());
        stroke.add(&mut canvas, pt(1.0, 1.0, 1.0));
        assert!(!is_white(canvas.pixel(1, 1)));
    }
}
