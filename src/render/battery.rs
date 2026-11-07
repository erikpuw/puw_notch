use super::*;

const GREEN_RING: (f32, f32, f32) = (0.2, 0.84, 0.29);

fn ring_colour(b: &Battery) -> (f32, f32, f32) {
    if b.charging {
        GREEN_RING
    } else if b.saver {
        (1.0, 0.8, 0.0)
    } else if b.percent <= 20 {
        (1.0, 0.27, 0.23)
    } else {
        GREEN_RING
    }
}

impl Renderer {
    pub(super) fn draw_ring(&self, centre: (f32, f32), d: f32, width: f32, fraction: f32, accent: (f32, f32, f32), a: f32) -> Result<()> {
        self.draw_ring_on(centre, d, width, fraction, accent, grey(0.3, a), a)
    }

    pub(super) fn draw_ring_on(&self, (x, y): (f32, f32), d: f32, width: f32, fraction: f32, accent: (f32, f32, f32), track: D2D1_COLOR_F, a: f32) -> Result<()> {
        let rt = &self.rt;
        let radius = (d - width) / 2.0;
        let ring = D2D1_ELLIPSE { point: Vector2 { X: x, Y: y }, radiusX: radius, radiusY: radius };
        let arc_colour = color(accent.0, accent.1, accent.2, a);
        unsafe {
            rt.DrawEllipse(&ring, &self.brush(track)?, width, None);
            let fraction = fraction.clamp(0.0, 1.0).max(0.03);
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
        }
        Ok(())
    }

    pub(super) fn draw_battery_ring(&self, (x, y): (f32, f32), d: f32, b: &Battery, a: f32) -> Result<()> {
        let rt = &self.rt;
        self.draw_ring((x, y), d, d * 0.16, b.percent as f32 / 100.0, ring_colour(b), a)?;
        let k = d / 22.0;
        let at = |px: f32, py: f32| Vector2 { X: x + px * k, Y: y + py * k };
        unsafe {
            let white = self.brush(grey(0.95, a))?;
            if b.charging {
                let geo = self.factory.CreatePathGeometry()?;
                let sink = geo.Open()?;
                sink.BeginFigure(at(1.3, -5.5), D2D1_FIGURE_BEGIN_FILLED);
                sink.AddLines(&[at(-3.2, 1.0), at(-0.3, 1.0), at(-1.3, 5.5), at(3.2, -1.0), at(0.3, -1.0)]);
                sink.EndFigure(D2D1_FIGURE_END_CLOSED);
                sink.Close()?;
                rt.FillGeometry(&geo, &white, None);
            }
        }
        Ok(())
    }
}
