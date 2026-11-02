use resvg::{tiny_skia::Transform, usvg};
use super::{MAX_RENDER_DEPTH, MAX_RENDER_PIXELS, MAX_SVG_NODES};
use super::validate::{MAX_ORDER, MAX_RADIUS, MAX_SIGMA};

const MAX_WORK: u64 = 256 * 1024 * 1024;
const FILTER_COPIES: u64 = 16;
const FILTER_BASE: u64 = 2;
const MERGE_COPIES: u64 = 2;
const MASK_COPIES: u64 = 2;
const MAX_SIDE: f32 = 16384.0;

#[derive(Clone, Copy)]
struct Surface { width: u32, height: u32 }

impl Surface {
    fn size(width: f32, height: f32) -> Option<Self> {
        if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 || width > MAX_SIDE || height > MAX_SIDE { return None; }
        Some(Self { width: width.ceil() as u32, height: height.ceil() as u32 })
    }
    fn pixels(self) -> u64 { u64::from(self.width) * u64::from(self.height) }
}

pub(super) fn check(tree: &usvg::Tree, transform: Transform, dimensions: (u32, u32)) -> Option<()> {
    let canvas = Surface { width: dimensions.0, height: dimensions.1 };
    let mut budget = Budget { pixels: 0, work: 0, visited: 0 };
    budget.charge(canvas, 1)?;
    budget.nodes(tree.root(), transform, canvas, 0)
}

struct Budget { pixels: u64, work: u64, visited: u32 }

impl Budget {
    fn enter(&mut self, depth: usize) -> Option<()> {
        self.visited = self.visited.checked_add(1)?;
        (depth <= MAX_RENDER_DEPTH && self.visited <= MAX_SVG_NODES).then_some(())
    }
    fn charge(&mut self, canvas: Surface, copies: u64) -> Option<()> {
        self.pixels = self.pixels.checked_add(canvas.pixels().checked_mul(copies)?)?;
        (self.pixels <= MAX_RENDER_PIXELS).then_some(())
    }
    fn work(&mut self, canvas: Surface, passes: u64) -> Option<()> {
        self.work = self.work.checked_add(canvas.pixels().checked_mul(passes)?)?;
        (self.work <= MAX_WORK).then_some(())
    }
    fn nodes(&mut self, group: &usvg::Group, transform: Transform, canvas: Surface, depth: usize) -> Option<()> {
        self.enter(depth)?;
        for node in group.children() {
            match node {
                usvg::Node::Group(group) => self.group(group, transform, canvas, depth + 1)?,
                usvg::Node::Text(text) => self.group(text.flattened(), transform, canvas, depth + 1)?,
                usvg::Node::Path(path) => {
                    if let Some(fill) = path.fill() { self.paint(fill.paint(), transform, canvas, depth + 1)?; }
                    if let Some(stroke) = path.stroke() { self.paint(stroke.paint(), transform, canvas, depth + 1)?; }
                }
                usvg::Node::Image(image) => match image.kind() {
                    usvg::ImageKind::SVG(tree) => {
                        self.charge(canvas, 1)?;
                        self.nodes(tree.root(), transform, canvas, depth + 1)?;
                    }
                    usvg::ImageKind::JPEG(bytes) | usvg::ImageKind::PNG(bytes) | usvg::ImageKind::GIF(bytes) | usvg::ImageKind::WEBP(bytes) => {
                        let size = imagesize::blob_size(bytes).ok()?;
                        self.charge(Surface::size(size.width as f32, size.height as f32)?, 2)?;
                    }
                }
            }
        }
        Some(())
    }
    fn group(&mut self, group: &usvg::Group, transform: Transform, canvas: Surface, depth: usize) -> Option<()> {
        let transform = transform.pre_concat(group.transform());
        if !group.should_isolate() { return self.nodes(group, transform, canvas, depth); }
        // Filter layers remove rotation/skew before allocating. Unclamped bounds overestimate resvg's crop.
        let transform = if group.filters().is_empty() { transform } else {
            let sx = transform.sx.hypot(transform.ky);
            let sy = transform.kx.hypot(transform.sy);
            Transform::from_row(transform.sx / sx, transform.ky / sx, transform.kx / sy, transform.sy / sy, 0.0, 0.0).invert()?.pre_concat(transform)
        };
        let bounds = group.layer_bounding_box().transform(transform)?;
        let canvas = Surface::size(bounds.width().ceil() + 4.0, bounds.height().ceil() + 4.0)?;
        self.charge(canvas, 1)?;
        self.nodes(group, transform, canvas, depth)?;
        for filter in group.filters() { self.filter(filter, transform, canvas, depth + 1)?; }
        if let Some(clip) = group.clip_path() { self.clip(clip, transform, canvas, depth + 1)?; }
        if let Some(mask) = group.mask() { self.mask(mask, transform, canvas, depth + 1)?; }
        Some(())
    }
    fn paint(&mut self, paint: &usvg::Paint, transform: Transform, _: Surface, depth: usize) -> Option<()> {
        let usvg::Paint::Pattern(pattern) = paint else { return Some(()) };
        let (sx, sy) = transform.pre_concat(pattern.transform()).get_scale();
        let canvas = Surface::size((pattern.rect().width() * sx).round(), (pattern.rect().height() * sy).round())?;
        self.charge(canvas, 1)?;
        self.nodes(pattern.root(), Transform::from_scale(sx, sy), canvas, depth)
    }
    fn clip(&mut self, clip: &usvg::ClipPath, transform: Transform, canvas: Surface, depth: usize) -> Option<()> {
        self.enter(depth)?;
        self.charge(canvas, MASK_COPIES)?;
        self.clip_nodes(clip.root(), transform.pre_concat(clip.transform()), canvas, depth + 1)?;
        if let Some(parent) = clip.clip_path() { self.clip(parent, transform, canvas, depth + 1)?; }
        Some(())
    }
    fn clip_nodes(&mut self, group: &usvg::Group, transform: Transform, canvas: Surface, depth: usize) -> Option<()> {
        self.enter(depth)?;
        for node in group.children() {
            match node {
                usvg::Node::Path(path) => if let Some(fill) = path.fill() { self.paint(fill.paint(), transform, canvas, depth + 1)?; },
                usvg::Node::Text(text) => self.clip_nodes(text.flattened(), transform, canvas, depth + 1)?,
                usvg::Node::Group(group) => {
                    let transform = transform.pre_concat(group.transform());
                    if let Some(clip) = group.clip_path() {
                        self.charge(canvas, 1)?;
                        self.clip(clip, transform, canvas, depth + 1)?;
                    }
                    self.clip_nodes(group, transform, canvas, depth + 1)?;
                }
                usvg::Node::Image(_) => {}
            }
        }
        Some(())
    }
    fn mask(&mut self, mask: &usvg::Mask, transform: Transform, canvas: Surface, depth: usize) -> Option<()> {
        self.enter(depth)?;
        if mask.root().children().is_empty() { return Some(()); }
        self.charge(canvas, MASK_COPIES)?;
        self.nodes(mask.root(), transform, canvas, depth + 1)?;
        if let Some(parent) = mask.mask() { self.mask(parent, transform, canvas, depth + 1)?; }
        Some(())
    }
    fn filter(&mut self, filter: &usvg::filter::Filter, transform: Transform, canvas: Surface, depth: usize) -> Option<()> {
        self.enter(depth)?;
        self.charge(canvas, FILTER_BASE)?;
        let (sx, sy) = transform.get_scale();
        for primitive in filter.primitives() {
            self.charge(canvas, FILTER_COPIES)?;
            let passes = match primitive.kind() {
                usvg::filter::Kind::Merge(merge) => {
                    self.charge(canvas, MERGE_COPIES.checked_mul(merge.inputs().len() as u64)?)?;
                    merge.inputs().len() as u64
                }
                usvg::filter::Kind::ConvolveMatrix(matrix) => {
                    if matrix.matrix().columns() > MAX_ORDER || matrix.matrix().rows() > MAX_ORDER { return None; }
                    let passes = u64::from(matrix.matrix().columns()) * u64::from(matrix.matrix().rows());
                    let wrap = if matrix.edge_mode() == usvg::filter::EdgeMode::Wrap { u64::from(matrix.matrix().columns().max(matrix.matrix().rows())) } else { 1 };
                    passes.checked_mul(wrap)?
                }
                usvg::filter::Kind::Morphology(morphology) => {
                    let x = morphology.radius_x().get() * sx;
                    let y = morphology.radius_y().get() * sy;
                    if !bounded(x, MAX_RADIUS) || !bounded(y, MAX_RADIUS) { return None; }
                    (2 * x.ceil() as u64 + 1).checked_mul(2 * y.ceil() as u64 + 1)?
                }
                usvg::filter::Kind::Turbulence(turbulence) => u64::from(turbulence.num_octaves().max(1)).checked_mul(4)?,
                usvg::filter::Kind::GaussianBlur(blur) => {
                    if !bounded(blur.std_dev_x().get() * sx, MAX_SIGMA) || !bounded(blur.std_dev_y().get() * sy, MAX_SIGMA) { return None; }
                    8
                }
                usvg::filter::Kind::DropShadow(shadow) => {
                    if !bounded(shadow.std_dev_x().get() * sx, MAX_SIGMA) || !bounded(shadow.std_dev_y().get() * sy, MAX_SIGMA) { return None; }
                    10
                }
                usvg::filter::Kind::Image(image) => {
                    self.nodes(image.root(), Transform::from_scale(sx, sy), canvas, depth + 1)?;
                    1
                }
                _ => 4,
            };
            self.work(canvas, passes)?;
        }
        Some(())
    }
}

fn bounded(value: f32, max: f32) -> bool { value.is_finite() && value >= 0.0 && value <= max }
