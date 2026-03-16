use eframe::egui::{self, Color32, Pos2, Stroke, Vec2};
use std::f32::consts::PI;

const GREEN: Color32 = Color32::from_rgb(0, 220, 100);
const YELLOW: Color32 = Color32::from_rgb(255, 200, 0);
const RED: Color32 = Color32::from_rgb(230, 60, 60);
const DIM: Color32 = Color32::from_rgb(45, 48, 55);

fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    Color32::from_rgb(
        (a.r() as f32 + (b.r() as f32 - a.r() as f32) * t) as u8,
        (a.g() as f32 + (b.g() as f32 - a.g() as f32) * t) as u8,
        (a.b() as f32 + (b.b() as f32 - a.b() as f32) * t) as u8,
    )
}

/// Smooth gradient color based on how many cents off.
pub fn cents_color(abs_cents: f32) -> Color32 {
    if abs_cents < 3.0 {
        GREEN
    } else if abs_cents < 8.0 {
        let t = (abs_cents - 3.0) / 5.0;
        lerp_color(GREEN, YELLOW, t)
    } else if abs_cents < 20.0 {
        let t = (abs_cents - 8.0) / 12.0;
        lerp_color(YELLOW, RED, t)
    } else {
        RED
    }
}

pub fn draw_gauge(ui: &mut egui::Ui, cents: f64, clarity: f64, needle_angle: f32, time: f64) {
    let desired_size = Vec2::new(ui.available_width().min(380.0), 200.0);
    let (response, painter) = ui.allocate_painter(desired_size, egui::Sense::hover());
    let rect = response.rect;

    let center = Pos2::new(rect.center().x, rect.bottom() - 24.0);
    let radius = (rect.width() * 0.40).min(145.0);

    let arc_start = PI;
    let arc_end = 0.0;
    let num_segments = 120;

    // Outer arc — thin border
    let outer_r = radius + 16.0;
    for i in 0..num_segments {
        let t0 = i as f32 / num_segments as f32;
        let t1 = (i + 1) as f32 / num_segments as f32;
        let a0 = arc_start + (arc_end - arc_start) * t0;
        let a1 = arc_start + (arc_end - arc_start) * t1;
        let p0 = Pos2::new(center.x + outer_r * a0.cos(), center.y - outer_r * a0.sin());
        let p1 = Pos2::new(center.x + outer_r * a1.cos(), center.y - outer_r * a1.sin());
        painter.line_segment([p0, p1], Stroke::new(1.5, Color32::from_gray(50)));
    }

    // Main colored arc
    for i in 0..num_segments {
        let t0 = i as f32 / num_segments as f32;
        let t1 = (i + 1) as f32 / num_segments as f32;
        let a0 = arc_start + (arc_end - arc_start) * t0;
        let a1 = arc_start + (arc_end - arc_start) * t1;

        let seg_cents = ((t0 + t1) / 2.0 * 100.0 - 50.0).abs();
        let color = cents_color(seg_cents);

        let p0 = Pos2::new(center.x + radius * a0.cos(), center.y - radius * a0.sin());
        let p1 = Pos2::new(center.x + radius * a1.cos(), center.y - radius * a1.sin());

        // Brighter near needle when active, dim background otherwise
        let brightness = if clarity > 0.0 {
            let needle_t = (needle_angle - arc_start) / (arc_end - arc_start);
            let seg_t = (t0 + t1) / 2.0;
            let dist = (needle_t - seg_t).abs();
            // Glow near needle
            0.25 + 0.6 * (-dist * dist * 80.0).exp()
        } else {
            0.15
        };

        painter.line_segment([p0, p1], Stroke::new(10.0, color.gamma_multiply(brightness)));
    }

    // Inner arc — thin border
    let inner_r = radius - 16.0;
    for i in 0..num_segments {
        let t0 = i as f32 / num_segments as f32;
        let t1 = (i + 1) as f32 / num_segments as f32;
        let a0 = arc_start + (arc_end - arc_start) * t0;
        let a1 = arc_start + (arc_end - arc_start) * t1;
        let p0 = Pos2::new(center.x + inner_r * a0.cos(), center.y - inner_r * a0.sin());
        let p1 = Pos2::new(center.x + inner_r * a1.cos(), center.y - inner_r * a1.sin());
        painter.line_segment([p0, p1], Stroke::new(1.5, Color32::from_gray(50)));
    }

    // Tick marks
    for &tick_cents in &[-50.0_f32, -25.0, -10.0, -5.0, 0.0, 5.0, 10.0, 25.0, 50.0] {
        let t = (tick_cents + 50.0) / 100.0;
        let angle = arc_start + (arc_end - arc_start) * t;

        let is_major = tick_cents.abs() == 0.0 || tick_cents.abs() == 25.0 || tick_cents.abs() == 50.0;
        let tick_inner = if is_major { radius - 20.0 } else { radius - 14.0 };
        let tick_outer = if is_major { radius + 20.0 } else { radius + 14.0 };
        let tick_width = if tick_cents == 0.0 { 2.5 } else if is_major { 1.5 } else { 1.0 };
        let tick_color = if tick_cents == 0.0 {
            GREEN.gamma_multiply(0.8)
        } else {
            Color32::from_gray(if is_major { 100 } else { 65 })
        };

        let p_inner = Pos2::new(center.x + tick_inner * angle.cos(), center.y - tick_inner * angle.sin());
        let p_outer = Pos2::new(center.x + tick_outer * angle.cos(), center.y - tick_outer * angle.sin());
        painter.line_segment([p_inner, p_outer], Stroke::new(tick_width, tick_color));

        // Labels for major ticks
        if is_major {
            let label_r = tick_outer + 14.0;
            let label_pos = Pos2::new(
                center.x + label_r * angle.cos(),
                center.y - label_r * angle.sin(),
            );
            let label = if tick_cents == 0.0 {
                "0".to_string()
            } else {
                format!("{:+.0}", tick_cents)
            };
            painter.text(
                label_pos,
                egui::Align2::CENTER_CENTER,
                label,
                egui::FontId::proportional(10.0),
                Color32::from_gray(110),
            );
        }
    }

    // Needle
    if clarity > 0.0 {
        let needle_len = radius - 18.0;
        let tip = Pos2::new(
            center.x + needle_len * needle_angle.cos(),
            center.y - needle_len * needle_angle.sin(),
        );

        let clamped_cents = (cents as f32).clamp(-50.0, 50.0).abs();
        let needle_color = cents_color(clamped_cents);
        let alpha = (clarity as f32 * 1.2).clamp(0.5, 1.0);

        // Needle shadow
        let shadow_offset = Vec2::new(1.5, 1.5);
        let shadow_tip = Pos2::new(tip.x + shadow_offset.x, tip.y + shadow_offset.y);
        let shadow_center = Pos2::new(center.x + shadow_offset.x, center.y + shadow_offset.y);
        painter.line_segment(
            [shadow_center, shadow_tip],
            Stroke::new(3.5, Color32::from_black_alpha(60)),
        );

        // Needle body
        painter.line_segment(
            [center, tip],
            Stroke::new(2.5, needle_color.gamma_multiply(alpha)),
        );

        // Hub
        painter.circle_filled(center, 7.0, DIM);
        painter.circle_filled(center, 5.0, needle_color.gamma_multiply(alpha));
        painter.circle_stroke(center, 7.0, Stroke::new(1.0, Color32::from_gray(70)));

        // Pulsing in-tune glow
        if clamped_cents < 3.0 {
            let intensity = (3.0 - clamped_cents) / 3.0;
            let pulse = ((time * 3.0).sin() as f32 * 0.3 + 0.7).clamp(0.5, 1.0);
            let glow_alpha = (intensity * pulse * 50.0) as u8;
            let top_angle = PI / 2.0;
            let glow_pos = Pos2::new(
                center.x + (radius + 2.0) * top_angle.cos(),
                center.y - (radius + 2.0) * top_angle.sin(),
            );
            painter.circle_filled(glow_pos, 18.0, Color32::from_rgba_unmultiplied(0, 220, 100, glow_alpha / 2));
            painter.circle_filled(glow_pos, 10.0, Color32::from_rgba_unmultiplied(0, 255, 110, glow_alpha));
            painter.circle_filled(glow_pos, 5.0, Color32::from_rgba_unmultiplied(200, 255, 220, glow_alpha));
        }
    } else {
        // No signal — dim hub
        painter.circle_filled(center, 7.0, DIM);
        painter.circle_filled(center, 5.0, Color32::from_gray(55));
        painter.circle_stroke(center, 7.0, Stroke::new(1.0, Color32::from_gray(45)));
    }

    // "CENTS" label
    painter.text(
        Pos2::new(center.x, center.y - 16.0),
        egui::Align2::CENTER_CENTER,
        "CENTS",
        egui::FontId::proportional(10.0),
        Color32::from_gray(70),
    );
}
