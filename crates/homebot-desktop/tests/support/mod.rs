#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]

use std::collections::HashMap;

use egui::{
    Context, FullOutput, TextureId, TexturesDelta,
    epaint::{ColorImage, ImageData, Mesh, Primitive, Vertex},
};
use egui_kittest::TestRenderer;
use image::{Rgba, RgbaImage};

/// Deterministic CPU renderer for visual goldens.
///
/// It intentionally uses nearest-neighbour texture sampling and a fixed
/// premultiplied-alpha triangle rasterizer. This removes GPU, driver and window
/// system differences from macOS and Linux snapshot generation.
#[derive(Default)]
pub struct CpuRenderer {
    textures: HashMap<TextureId, ColorImage>,
}

impl TestRenderer for CpuRenderer {
    fn handle_delta(&mut self, delta: &TexturesDelta) {
        for (id, change) in &delta.set {
            let ImageData::Color(update) = &change.image;
            if let Some([left, top]) = change.pos {
                let Some(target) = self.textures.get_mut(id) else {
                    continue;
                };
                for row in 0..update.size[1] {
                    let source = row * update.size[0];
                    let destination = (top + row) * target.size[0] + left;
                    target.pixels[destination..destination + update.size[0]]
                        .copy_from_slice(&update.pixels[source..source + update.size[0]]);
                }
            } else {
                self.textures.insert(*id, update.as_ref().clone());
            }
        }
        for id in &delta.free {
            self.textures.remove(id);
        }
    }

    fn render(&mut self, context: &Context, output: &FullOutput) -> Result<RgbaImage, String> {
        let pixels_per_point = context.pixels_per_point();
        let logical_size = context.screen_rect().size();
        let width = (logical_size.x * pixels_per_point).round() as u32;
        let height = (logical_size.y * pixels_per_point).round() as u32;
        let mut image = RgbaImage::from_pixel(width, height, Rgba([0, 0, 0, 0]));

        for clipped in context.tessellate(output.shapes.clone(), pixels_per_point) {
            match clipped.primitive {
                Primitive::Mesh(mesh) => {
                    self.paint_mesh(&mut image, &mesh, clipped.clip_rect, pixels_per_point)?;
                }
                Primitive::Callback(_) => {
                    return Err("paint callbacks are forbidden in deterministic goldens".to_owned());
                }
            }
        }
        Ok(image)
    }
}

impl CpuRenderer {
    fn paint_mesh(
        &self,
        target: &mut RgbaImage,
        mesh: &Mesh,
        clip: egui::Rect,
        pixels_per_point: f32,
    ) -> Result<(), String> {
        let texture = self
            .textures
            .get(&mesh.texture_id)
            .ok_or_else(|| format!("missing texture {:?}", mesh.texture_id))?;
        for triangle in mesh.indices.chunks_exact(3) {
            let vertices = [
                mesh.vertices[triangle[0] as usize],
                mesh.vertices[triangle[1] as usize],
                mesh.vertices[triangle[2] as usize],
            ];
            paint_triangle(target, texture, vertices, clip, pixels_per_point);
        }
        Ok(())
    }
}

fn paint_triangle(
    target: &mut RgbaImage,
    texture: &ColorImage,
    mut vertices: [Vertex; 3],
    clip: egui::Rect,
    pixels_per_point: f32,
) {
    for vertex in &mut vertices {
        vertex.pos *= pixels_per_point;
    }
    let min_x = vertices
        .iter()
        .map(|vertex| vertex.pos.x)
        .fold(f32::INFINITY, f32::min)
        .max(clip.min.x * pixels_per_point)
        .floor()
        .max(0.0) as u32;
    let min_y = vertices
        .iter()
        .map(|vertex| vertex.pos.y)
        .fold(f32::INFINITY, f32::min)
        .max(clip.min.y * pixels_per_point)
        .floor()
        .max(0.0) as u32;
    let max_x = vertices
        .iter()
        .map(|vertex| vertex.pos.x)
        .fold(f32::NEG_INFINITY, f32::max)
        .min(clip.max.x * pixels_per_point)
        .ceil()
        .min(target.width() as f32) as u32;
    let max_y = vertices
        .iter()
        .map(|vertex| vertex.pos.y)
        .fold(f32::NEG_INFINITY, f32::max)
        .min(clip.max.y * pixels_per_point)
        .ceil()
        .min(target.height() as f32) as u32;

    let area = edge(vertices[0].pos, vertices[1].pos, vertices[2].pos);
    if area.abs() <= f32::EPSILON {
        return;
    }
    for y in min_y..max_y {
        for x in min_x..max_x {
            let point = egui::pos2(x as f32 + 0.5, y as f32 + 0.5);
            let weights = [
                edge(vertices[1].pos, vertices[2].pos, point) / area,
                edge(vertices[2].pos, vertices[0].pos, point) / area,
                edge(vertices[0].pos, vertices[1].pos, point) / area,
            ];
            if weights.iter().any(|weight| *weight < 0.0) {
                continue;
            }
            let uv = vertices[0].uv.to_vec2() * weights[0]
                + vertices[1].uv.to_vec2() * weights[1]
                + vertices[2].uv.to_vec2() * weights[2];
            let vertex_color = interpolate_color(vertices, weights);
            let texture_color = sample(texture, uv.x, uv.y);
            let source = multiply(vertex_color, texture_color);
            blend(target.get_pixel_mut(x, y), source);
        }
    }
}

fn edge(a: egui::Pos2, b: egui::Pos2, point: egui::Pos2) -> f32 {
    (point.x - a.x) * (b.y - a.y) - (point.y - a.y) * (b.x - a.x)
}

fn interpolate_color(vertices: [Vertex; 3], weights: [f32; 3]) -> [u8; 4] {
    let colors = vertices.map(|vertex| vertex.color.to_array());
    std::array::from_fn(|channel| {
        (f32::from(colors[0][channel]) * weights[0]
            + f32::from(colors[1][channel]) * weights[1]
            + f32::from(colors[2][channel]) * weights[2])
            .round()
            .clamp(0.0, 255.0) as u8
    })
}

fn sample(texture: &ColorImage, u: f32, v: f32) -> [u8; 4] {
    let x = (u.clamp(0.0, 1.0) * (texture.size[0].saturating_sub(1)) as f32).round() as usize;
    let y = (v.clamp(0.0, 1.0) * (texture.size[1].saturating_sub(1)) as f32).round() as usize;
    texture.pixels[y * texture.size[0] + x].to_array()
}

fn multiply(left: [u8; 4], right: [u8; 4]) -> [u8; 4] {
    std::array::from_fn(|channel| {
        ((u16::from(left[channel]) * u16::from(right[channel]) + 127) / 255) as u8
    })
}

fn blend(destination: &mut Rgba<u8>, source: [u8; 4]) {
    let inverse_alpha = 255_u16 - u16::from(source[3]);
    for channel in 0..3 {
        destination[channel] = (u16::from(source[channel])
            + (u16::from(destination[channel]) * inverse_alpha + 127) / 255)
            .min(255) as u8;
    }
    destination[3] = (u16::from(source[3])
        + (u16::from(destination[3]) * inverse_alpha + 127) / 255)
        .min(255) as u8;
}
