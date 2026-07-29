//! Vector PDF export for the planar mandala.
//!
//! The exporter writes the complete drawing rather than a screenshot of the
//! current viewport. Geometry, resize clearance, nesting order, fitted caption
//! layout, palette, and operator routes come from the same data and helpers as
//! the live canvas; SVG is only the vector interchange used to produce a
//! compact one-page PDF.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::Path;

use eframe::egui::{Color32, Pos2, Vec2};
use kaos_core::visual::{Form, Mandala, Node, NodeId, Shape, Stroke};

use crate::theme::Ink;
use crate::{
    circuit_trace_geometry, edge_colour, fit_caption, node_outline_width, node_resize,
    resize_weight, visible_edges,
};

const A4_SHORT: f32 = 595.28;
const A4_LONG: f32 = 841.89;
const PAGE_MARGIN: f32 = 34.0;

#[derive(Clone, Copy, Debug)]
struct Bounds {
    min_x: f32,
    min_y: f32,
    max_x: f32,
    max_y: f32,
}

impl Bounds {
    fn empty() -> Self {
        Self {
            min_x: f32::INFINITY,
            min_y: f32::INFINITY,
            max_x: f32::NEG_INFINITY,
            max_y: f32::NEG_INFINITY,
        }
    }

    fn include(&mut self, x: f32, y: f32) {
        self.min_x = self.min_x.min(x);
        self.min_y = self.min_y.min(y);
        self.max_x = self.max_x.max(x);
        self.max_y = self.max_y.max(y);
    }

    fn include_rect(&mut self, centre: Pos2, half: Vec2) {
        self.include(centre.x - half.x, centre.y - half.y);
        self.include(centre.x + half.x, centre.y + half.y);
    }

    fn width(self) -> f32 {
        self.max_x - self.min_x
    }

    fn height(self) -> f32 {
        self.max_y - self.min_y
    }

    fn padded(mut self, amount: f32) -> Self {
        self.min_x -= amount;
        self.min_y -= amount;
        self.max_x += amount;
        self.max_y += amount;
        self
    }

    fn with_aspect(mut self, aspect: f32) -> Self {
        let current = self.width() / self.height().max(f32::EPSILON);
        if current < aspect {
            let extra = (self.height() * aspect - self.width()) * 0.5;
            self.min_x -= extra;
            self.max_x += extra;
        } else {
            let extra = (self.width() / aspect - self.height()) * 0.5;
            self.min_y -= extra;
            self.max_y += extra;
        }
        self
    }
}

pub(crate) fn save(
    mandala: &Mandala,
    angled: &HashSet<NodeId>,
    ink: Ink,
    path: &Path,
) -> Result<(), String> {
    let bytes = render(mandala, angled, ink)?;
    std::fs::write(path, bytes)
        .map_err(|error| format!("could not write {}: {error}", path.display()))
}

fn render(mandala: &Mandala, angled: &HashSet<NodeId>, ink: Ink) -> Result<Vec<u8>, String> {
    let svg = render_svg(mandala, angled, ink)?;
    let mut options = svg2pdf::usvg::Options::default();
    options.fontdb_mut().load_system_fonts();
    let tree = svg2pdf::usvg::Tree::from_str(&svg, &options)
        .map_err(|error| format!("could not build PDF drawing: {error}"))?;
    svg2pdf::to_pdf(
        &tree,
        svg2pdf::ConversionOptions::default(),
        svg2pdf::PageOptions::default(),
    )
    .map_err(|error| format!("could not encode PDF: {error}"))
}

fn render_svg(mandala: &Mandala, angled: &HashSet<NodeId>, ink: Ink) -> Result<String, String> {
    if mandala.is_empty() {
        return Err("nothing to export — the mandala is empty".to_string());
    }

    let bounds = drawing_bounds(mandala).padded(PAGE_MARGIN);
    let landscape = bounds.width() >= bounds.height();
    let (page_width, page_height) = if landscape {
        (A4_LONG, A4_SHORT)
    } else {
        (A4_SHORT, A4_LONG)
    };
    let bounds = bounds.with_aspect(page_width / page_height);
    let mut svg = String::new();
    write!(
        svg,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{page_width:.2}" height="{page_height:.2}" viewBox="{:.3} {:.3} {:.3} {:.3}" preserveAspectRatio="xMidYMid meet">"#,
        bounds.min_x,
        bounds.min_y,
        bounds.width(),
        bounds.height()
    )
    .expect("writing to a String cannot fail");
    write!(
        svg,
        r#"<rect x="{:.3}" y="{:.3}" width="{:.3}" height="{:.3}" fill="{}"/>"#,
        bounds.min_x,
        bounds.min_y,
        bounds.width(),
        bounds.height(),
        colour(ink.ground)
    )
    .expect("writing to a String cannot fail");
    svg.push_str(
        r#"<g stroke-linecap="round" stroke-linejoin="round" font-family="'DejaVu Sans Mono','Liberation Mono',monospace">"#,
    );

    // Match the 2D painter: every boundary is a receiving surface beneath the
    // operator traces, and text-bearing/sigil forms sit above both.
    append_nodes(&mut svg, mandala, ink, false, true);
    append_nodes(&mut svg, mandala, ink, true, true);
    append_edges(&mut svg, mandala, angled, ink, false);
    append_edges(&mut svg, mandala, angled, ink, true);
    append_nodes(&mut svg, mandala, ink, false, false);
    append_nodes(&mut svg, mandala, ink, true, false);
    svg.push_str("</g></svg>");
    Ok(svg)
}

fn drawing_bounds(mandala: &Mandala) -> Bounds {
    let mut bounds = Bounds::empty();
    for node in mandala.nodes() {
        let centre = Pos2::new(node.x as f32, node.y as f32);
        let extent = mandala.extent(node.id);
        let half = Vec2::new(extent.0 as f32, extent.1 as f32);
        bounds.include_rect(centre, half);
        let resize = node_resize(mandala, node);
        let symbol_scale = resize_weight(resize);

        if node.form.uses_text()
            && !matches!(
                node.shape(),
                Shape::Circle
                    | Shape::Triangle
                    | Shape::Square
                    | Shape::Diamond
                    | Shape::Parallelogram
                    | Shape::Amp
                    | Shape::Hexagon
            )
        {
            let font = 11.0 * symbol_scale;
            let width = node.caption().chars().count() as f32 * font * 0.62;
            let label = Pos2::new(centre.x, centre.y + half.y + 12.0 * symbol_scale);
            bounds.include_rect(label, Vec2::new(width * 0.5, font * 0.65));
        }
        if let Some(model) = &node.model {
            let font = 9.0 * symbol_scale;
            let width = (model.chars().count() + 1) as f32 * font * 0.62;
            let label = Pos2::new(centre.x, centre.y - half.y - 13.0 * symbol_scale);
            bounds.include_rect(label, Vec2::new(width * 0.5, font));
        }
    }
    bounds
}

fn append_nodes(svg: &mut String, mandala: &Mandala, ink: Ink, inlined: bool, boundaries: bool) {
    for id in mandala.paint_order(inlined) {
        let Some(node) = mandala.node(id) else {
            continue;
        };
        if node.form.opens_indentation() != boundaries {
            continue;
        }
        append_node(svg, mandala, node, ink);
    }
}

fn append_node(svg: &mut String, mandala: &Mandala, node: &Node, ink: Ink) {
    let centre = Pos2::new(node.x as f32, node.y as f32);
    let extent = mandala.extent(node.id);
    let half = Vec2::new(extent.0 as f32, extent.1 as f32);
    let resize = node_resize(mandala, node);
    let symbol_scale = resize_weight(resize);
    let shape = node.shape();
    let accented = false;
    let outline = ink.faint;
    let outline_width = node_outline_width(shape, accented) * symbol_scale;
    let fill = colour(ink.fill);
    let stroke = colour(outline);

    match shape {
        Shape::Circle => {
            write!(
                svg,
                r#"<circle cx="{:.3}" cy="{:.3}" r="{:.3}" fill="{fill}" stroke="{stroke}" stroke-width="{outline_width:.3}"/>"#,
                centre.x, centre.y, half.x
            )
            .expect("writing to a String cannot fail");
        }
        Shape::Square => {
            write!(
                svg,
                r#"<rect x="{:.3}" y="{:.3}" width="{:.3}" height="{:.3}" rx="4" fill="{fill}" stroke="{stroke}" stroke-width="{outline_width:.3}"/>"#,
                centre.x - half.x,
                centre.y - half.y,
                half.x * 2.0,
                half.y * 2.0
            )
            .expect("writing to a String cannot fail");
        }
        Shape::Triangle => append_polygon(
            svg,
            centre,
            &Shape::triangle_points(),
            resize,
            fill.as_str(),
            stroke.as_str(),
            outline_width,
        ),
        Shape::Diamond => append_polygon(
            svg,
            centre,
            &Shape::diamond_points(),
            resize,
            fill.as_str(),
            stroke.as_str(),
            outline_width,
        ),
        Shape::Parallelogram => append_polygon(
            svg,
            centre,
            &Shape::parallelogram_points(),
            resize,
            fill.as_str(),
            stroke.as_str(),
            outline_width,
        ),
        Shape::Hexagon => append_polygon(
            svg,
            centre,
            &Shape::hexagon_points(),
            resize,
            fill.as_str(),
            stroke.as_str(),
            outline_width,
        ),
        Shape::Amp => append_polygon(
            svg,
            centre,
            &Shape::inlet_points(),
            resize,
            fill.as_str(),
            stroke.as_str(),
            outline_width,
        ),
        Shape::Arrow => {
            let mut points = Shape::arrow_points();
            if node.form == Form::Backflow {
                for point in &mut points {
                    point.0 = -point.0;
                }
            }
            append_polygon(
                svg,
                centre,
                &points,
                resize,
                fill.as_str(),
                stroke.as_str(),
                outline_width,
            );
        }
        _ => {
            let pen = 5.0 * symbol_scale;
            for shape_stroke in shape.strokes() {
                match shape_stroke {
                    Stroke::Poly(points) => {
                        let points = points
                            .iter()
                            .map(|(x, y)| {
                                format!(
                                    "{:.3},{:.3}",
                                    centre.x + x * resize.x,
                                    centre.y + y * resize.y
                                )
                            })
                            .collect::<Vec<_>>()
                            .join(" ");
                        write!(
                            svg,
                            r#"<polyline points="{points}" fill="none" stroke="{}" stroke-width="{pen:.3}"/>"#,
                            colour(ink.ink)
                        )
                        .expect("writing to a String cannot fail");
                    }
                    Stroke::Cubic(points) => {
                        let at = |point: (f32, f32)| {
                            (centre.x + point.0 * resize.x, centre.y + point.1 * resize.y)
                        };
                        let [a, b, c, d] = points.map(at);
                        write!(
                            svg,
                            r#"<path d="M {:.3} {:.3} C {:.3} {:.3}, {:.3} {:.3}, {:.3} {:.3}" fill="none" stroke="{}" stroke-width="{pen:.3}"/>"#,
                            a.0,
                            a.1,
                            b.0,
                            b.1,
                            c.0,
                            c.1,
                            d.0,
                            d.1,
                            colour(ink.ink)
                        )
                        .expect("writing to a String cannot fail");
                    }
                }
            }
        }
    }

    if let Some(model) = &node.model {
        append_text(
            svg,
            centre.x,
            centre.y - half.y - 13.0 * symbol_scale,
            9.0 * symbol_scale,
            &format!("/{model}"),
            ink.secondary,
        );
    }

    // An indentation wears the notation that opened it on its own ring, exactly
    // as the canvas draws it, so an export reads as the same drawing.
    let mark = node.mark();
    if !mark.is_empty() {
        append_text(
            svg,
            centre.x,
            centre.y - half.y,
            // The same damped growth the canvas uses, so an export reads as
            // the same drawing.
            crate::MARK_PX * crate::mark_weight(resize),
            &mark,
            ink.ink,
        );
    }

    // A program triangle is deliberately anonymous in static output. In the
    // editor its "program" label appears only while the node is selected.
    let caption = node.caption();
    if caption.is_empty() || node.form == Form::Program {
        return;
    }
    if node.form.uses_text()
        && matches!(
            shape,
            Shape::Circle
                | Shape::Triangle
                | Shape::Square
                | Shape::Diamond
                | Shape::Parallelogram
                | Shape::Amp
                | Shape::Hexagon
        )
    {
        let layout = fit_caption(&caption, shape, half, f32::INFINITY);
        let centre = centre + layout.offset;
        let first_y =
            centre.y - layout.line_height * (layout.lines.len().saturating_sub(1) as f32) * 0.5;
        for (index, line) in layout.lines.iter().enumerate() {
            append_text(
                svg,
                centre.x,
                first_y + index as f32 * layout.line_height,
                layout.font_size,
                line,
                ink.ink,
            );
        }
    } else {
        let font = 11.0 * symbol_scale;
        let inside = matches!(
            shape,
            Shape::Circle
                | Shape::Triangle
                | Shape::Square
                | Shape::Diamond
                | Shape::Parallelogram
                | Shape::Amp
                | Shape::Hexagon
        );
        append_text(
            svg,
            centre.x,
            if inside {
                centre.y
            } else {
                centre.y + half.y + 12.0 * symbol_scale
            },
            font,
            &caption,
            if inside { ink.ink } else { ink.faint },
        );
    }
}

fn append_edges(
    svg: &mut String,
    mandala: &Mandala,
    angled: &HashSet<NodeId>,
    ink: Ink,
    want_interior: bool,
) {
    for edge in visible_edges(mandala) {
        let interior = mandala.is_inlined(edge.from) && mandala.is_inlined(edge.to);
        if interior != want_interior {
            continue;
        }
        let (Some(from), Some(to)) = (mandala.node(edge.from), mandala.node(edge.to)) else {
            continue;
        };
        let from_extent = mandala.extent(from.id);
        let to_extent = mandala.extent(to.id);
        let segments = circuit_trace_geometry(
            Pos2::new(from.x as f32, from.y as f32),
            Pos2::new(to.x as f32, to.y as f32),
            [
                Vec2::new(from_extent.0 as f32, from_extent.1 as f32),
                Vec2::new(to_extent.0 as f32, to_extent.1 as f32),
            ],
            11.0,
            angled.contains(&edge.owner),
        );
        let mut path = String::new();
        for [start, end] in segments {
            write!(
                path,
                "M {:.3} {:.3} L {:.3} {:.3} ",
                start.x, start.y, end.x, end.y
            )
            .expect("writing to a String cannot fail");
        }
        write!(
            svg,
            r#"<path d="{path}" fill="none" stroke="{}" stroke-width="1.8"/>"#,
            colour(edge_colour(ink))
        )
        .expect("writing to a String cannot fail");

        if let Some(model) = mandala
            .node(edge.owner)
            .and_then(|node| node.model.as_deref())
        {
            append_text(
                svg,
                (from.x + to.x) as f32 * 0.5,
                (from.y + to.y) as f32 * 0.5 - 8.0,
                9.0,
                &format!("/{model}"),
                ink.secondary,
            );
        }
    }
}

fn append_polygon(
    svg: &mut String,
    centre: Pos2,
    points: &[(f32, f32)],
    resize: Vec2,
    fill: &str,
    stroke: &str,
    stroke_width: f32,
) {
    let points = points
        .iter()
        .map(|(x, y)| {
            format!(
                "{:.3},{:.3}",
                centre.x + x * resize.x,
                centre.y + y * resize.y
            )
        })
        .collect::<Vec<_>>()
        .join(" ");
    write!(
        svg,
        r#"<polygon points="{points}" fill="{fill}" stroke="{stroke}" stroke-width="{stroke_width:.3}"/>"#
    )
    .expect("writing to a String cannot fail");
}

fn append_text(svg: &mut String, x: f32, centre_y: f32, size: f32, text: &str, color: Color32) {
    // SVG text positions use the baseline. 0.34 em centres the visual body of
    // the monospace face closely enough to match egui's CENTER_CENTER anchor.
    let baseline = centre_y + size * 0.34;
    write!(
        svg,
        r#"<text x="{x:.3}" y="{baseline:.3}" text-anchor="middle" xml:space="preserve" font-size="{size:.3}" fill="{}">{}</text>"#,
        colour(color),
        escape_xml(text)
    )
    .expect("writing to a String cannot fail");
}

fn colour(color: Color32) -> String {
    format!("#{:02x}{:02x}{:02x}", color.r(), color.g(), color.b())
}

fn escape_xml(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn svg_export_keeps_complete_prompt_text_and_hides_program_label() {
        let mut mandala = Mandala::new();
        mandala.add(
            Form::Prompt,
            "complete <prompt> & every character",
            0.0,
            0.0,
        );
        mandala.add(Form::Program, "", 120.0, 0.0);

        let svg = render_svg(&mandala, &HashSet::new(), Ink::load()).unwrap();

        assert!(svg.contains("complete "));
        assert!(svg.contains("&lt;prompt&gt;"));
        assert!(svg.contains("&amp; every"));
        assert!(!svg.contains(">program</text>"));
    }

    #[test]
    fn mandala_export_is_a_real_pdf() {
        let mut mandala = Mandala::new();
        mandala.add(Form::Prompt, "full prompt", 0.0, 0.0);

        let bytes = render(&mandala, &HashSet::new(), Ink::load()).unwrap();

        assert!(bytes.starts_with(b"%PDF-"));
        assert!(bytes.ends_with(b"%%EOF"));
    }
}
