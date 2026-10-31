//! The battery as a ring (like a phone's lock-screen gauge): a track, an arc of the charge from
//! the top clockwise, a bolt in the middle while charging. Its colour is `battery_tint`'s:
//! green charging, yellow in battery saver, red at 20 % or less, white otherwise.

use super::*;

impl Renderer {
    /// A ring of diameter `d` (px) centred on `(x, y)`, filled to `b.percent`, at opacity `a`.
    pub(super) fn draw_battery_ring(&self, (x, y): (f32, f32), d: f32, b: &Battery, a: f32) -> Result<()> {
        let rt = &self.rt;
        let (accent, _) = battery_tint(b);
        let width = d * 0.13;
        let radius = (d - width) / 2.0;
        let ring = D2D1_ELLIPSE { point: Vector2 { X: x, Y: y }, radiusX: radius, radiusY: radius };
        let arc_colour = color(accent.0, accent.1, accent.2, a);
        unsafe {
            rt.DrawEllipse(&ring, &self.brush(grey(0.3, a))?, width, None);
            let fraction = (b.percent.min(100) as f32 / 100.0).max(0.03);
            if fraction >= 1.0 {
                rt.DrawEllipse(&ring, &self.brush(arc_colour)?, width, None);
            } else {
                let angle = fraction * std::f32::consts::TAU;
                let end = Vector2 { X: x + radius * angle.sin(), Y: y - radius * angle.cos() };
                let geo = self.factory.CreatePathGeometry()?;
                let sink = geo.Open()?;
                sink.BeginFigure(Vector2 { X: x, Y: y - radius }, D2D1_FIGURE_BEGIN_HOLLOW);
                sink.AddArc(&D2D1_ARC_SEGMENT {
                    point: end,
                    size: D2D_SIZE_F { width: radius, height: radius },
                    rotationAngle: 0.0,
                    sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
                    arcSize: if fraction > 0.5 { D2D1_ARC_SIZE_LARGE } else { D2D1_ARC_SIZE_SMALL },
                });
                sink.EndFigure(D2D1_FIGURE_END_OPEN);
                sink.Close()?;
                rt.DrawGeometry(&geo, &self.brush(arc_colour)?, width, &self.round);
            }
            if b.charging {
                // A bolt, 10 px tall at a ring 18 px across.
                let k = d / 18.0;
                let at = |px: f32, py: f32| Vector2 { X: x + px * k, Y: y + py * k };
                let geo = self.factory.CreatePathGeometry()?;
                let sink = geo.Open()?;
                sink.BeginFigure(at(1.2, -5.0), D2D1_FIGURE_BEGIN_FILLED);
                sink.AddLines(&[at(-3.0, 1.0), at(-0.2, 1.0), at(-1.2, 5.0), at(3.0, -1.0), at(0.2, -1.0)]);
                sink.EndFigure(D2D1_FIGURE_END_CLOSED);
                sink.Close()?;
                rt.FillGeometry(&geo, &self.brush(arc_colour)?, None);
            }
        }
        Ok(())
    }
}
