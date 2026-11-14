// Shared data structures. Kept intentionally identical in meaning to the
// C# TTJiang.Capture.Layout types so the OCR port can be diffed line by line.

#[derive(Clone, Debug)]
pub struct OcrWord {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl OcrWord {
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn cy(&self) -> f32 {
        self.y + self.h * 0.5
    }
}

#[derive(Clone, Debug, Default)]
pub struct OcrResult {
    pub words: Vec<OcrWord>,
    pub text: String,
    pub language: String,
    pub engine: String,
    pub max_image_dimension: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Rect { x, y, w, h }
    }
    pub fn right(&self) -> i32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }
    pub fn is_empty(&self) -> bool {
        self.w <= 0 || self.h <= 0
    }
    pub fn from_points(x0: i32, y0: i32, x1: i32, y1: i32) -> Self {
        let x = x0.min(x1);
        let y = y0.min(y1);
        Rect::new(x, y, (x1 - x0).abs(), (y1 - y0).abs())
    }
    pub fn intersect(&self, other: Rect) -> Rect {
        let x0 = self.x.max(other.x);
        let y0 = self.y.max(other.y);
        let x1 = self.right().min(other.right());
        let y1 = self.bottom().min(other.bottom());
        if x1 <= x0 || y1 <= y0 {
            Rect::new(0, 0, 0, 0)
        } else {
            Rect::new(x0, y0, x1 - x0, y1 - y0)
        }
    }
    pub fn inflate(&self, dx: i32, dy: i32) -> Rect {
        Rect::new(self.x - dx, self.y - dy, self.w + 2 * dx, self.h + 2 * dy)
    }
    pub fn union(&self, other: Rect) -> Rect {
        if self.is_empty() {
            return other;
        }
        if other.is_empty() {
            return *self;
        }
        let x0 = self.x.min(other.x);
        let y0 = self.y.min(other.y);
        let x1 = self.right().max(other.right());
        let y1 = self.bottom().max(other.bottom());
        Rect::new(x0, y0, x1 - x0, y1 - y0)
    }
    pub fn clamp(&self, w: i32, h: i32) -> Rect {
        let x0 = self.x.max(0).min(w);
        let y0 = self.y.max(0).min(h);
        let x1 = self.right().max(0).min(w);
        let y1 = self.bottom().max(0).min(h);
        Rect::new(x0, y0, (x1 - x0).max(0), (y1 - y0).max(0))
    }
}
