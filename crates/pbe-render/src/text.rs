//! Text rasterization: shaped glyphs → pixels, composed from two sealed
//! primitives.
//!
//! - **Shaping** is the kit's `cap_text_shape::CosmicShaper` (cosmic-text):
//!   text + font + size → positioned glyph ids. It deliberately does not
//!   rasterize, but exposes `font_bytes(font_id)` so the renderer can.
//! - **Rasterization** is `ab_glyph`: a sealed, pure-Rust glyph-outline →
//!   coverage primitive. We feed it the same font bytes and the shaped glyph
//!   ids, and blit the coverage onto our framebuffer.
//!
//! This is the composition doctrine for text: drive two sealed math engines
//! from outside; the engine itself contains no font-format or shaping logic.

use std::collections::HashMap;

use ab_glyph::{Font, FontVec, Glyph, GlyphId as AbGlyphId, Point as AbPoint, ScaleFont};
use cap_geometry::Pixels;
use cap_text_shape::{CosmicShaper, FontDescriptor, FontId, FontStyle, FontWeight, ShapeRequest};

use crate::Raster;
use pbe_protocol::TextDraw;

/// Holds the shaper plus a cache of `ab_glyph` fonts keyed by the kit's
/// `FontId`, so we load + parse each face only once per render.
pub struct TextRasterizer {
    shaper: CosmicShaper,
    fonts: HashMap<usize, FontVec>,
}

impl TextRasterizer {
    pub fn new() -> Self {
        Self {
            shaper: CosmicShaper::new(),
            fonts: HashMap::new(),
        }
    }

    /// Shape and rasterize one text run onto `raster`. Positions glyphs from the
    /// run's pen origin; the baseline is `top_y + ascent`.
    pub fn draw(&mut self, raster: &mut Raster, run: &TextDraw) {
        let weight = if run.bold {
            FontWeight::Bold
        } else {
            FontWeight::Normal
        };
        let style = if run.italic {
            FontStyle::Italic
        } else {
            FontStyle::Normal
        };
        let size = Pixels(run.font_size.max(1.0));

        let shaped = self.shaper.shape(ShapeRequest {
            text: &run.text,
            font: FontDescriptor {
                family: &run.family,
                weight,
                style,
            },
            size,
        });

        // Baseline: place text within its box using the first run's font ascent.
        let ascent = shaped
            .runs
            .first()
            .and_then(|r| self.shaper.font_metrics(r.font_id, size))
            .map(|m| m.ascent.0)
            .unwrap_or(run.font_size * 0.8);
        let baseline_y = run.top_y + ascent;

        let color = [run.r, run.g, run.b, run.a];
        let mut pen_x = run.x;

        for srun in &shaped.runs {
            // Ensure the ab_glyph font for this face is loaded.
            if !self.fonts.contains_key(&srun.font_id.0) {
                if let Some(bytes) = self.font_bytes(srun.font_id) {
                    if let Ok(font) = FontVec::try_from_vec(bytes) {
                        self.fonts.insert(srun.font_id.0, font);
                    }
                }
            }
            let Some(font) = self.fonts.get(&srun.font_id.0) else {
                // Could not load this face; still advance the pen so layout of
                // following runs stays correct.
                for g in &srun.glyphs {
                    pen_x += g.x_advance.0;
                }
                continue;
            };
            let scaled = font.as_scaled(run.font_size.max(1.0));

            for g in &srun.glyphs {
                let gx = pen_x + g.x_offset.0;
                let gy = baseline_y + g.y_offset.0;
                let glyph = Glyph {
                    id: AbGlyphId(g.glyph_id.0 as u16),
                    scale: scaled.scale(),
                    position: AbPoint { x: gx, y: gy },
                };
                if let Some(outlined) = font.outline_glyph(glyph) {
                    let bb = outlined.px_bounds();
                    outlined.draw(|ox, oy, coverage| {
                        let px = bb.min.x as i32 + ox as i32;
                        let py = bb.min.y as i32 + oy as i32;
                        raster.blend_px(px, py, color, coverage);
                    });
                }
                pen_x += g.x_advance.0;
            }
        }
    }

    /// Fetch a face's raw bytes from the shaper (cosmic-text fontdb).
    fn font_bytes(&mut self, font_id: FontId) -> Option<Vec<u8>> {
        self.shaper.font_bytes(font_id)
    }
}

impl Default for TextRasterizer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cap_primitives::Rgba;

    #[test]
    fn draws_dark_text_onto_a_white_raster() {
        // White background; draw black text and confirm some pixel got darker.
        let white = Rgba {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 1.0,
        };
        let mut raster = Raster::new(400, 60, white);
        let mut tr = TextRasterizer::new();
        tr.draw(
            &mut raster,
            &TextDraw {
                text: "Ag".to_string(),
                x: 4.0,
                top_y: 4.0,
                font_size: 40.0,
                family: "sans-serif".to_string(),
                bold: false,
                italic: false,
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            },
        );

        // Count non-white pixels. With any system font present this is > 0; in a
        // truly font-less environment cosmic-text yields no glyphs — tolerate
        // that rather than failing on CI with no fonts.
        let darkened = raster
            .pixels
            .chunks_exact(4)
            .filter(|p| p[0] < 250 || p[1] < 250 || p[2] < 250)
            .count();

        if cfg!(windows) {
            // Windows always ships fonts; require real glyph coverage here.
            assert!(darkened > 0, "expected text to mark pixels, got {darkened}");
        }
    }
}
