pub use freedom_canvas as canvas;
pub use freedom_font as font;
pub use freedom_graphics_core::{flatten, geometry, raster};
pub use freedom_image as image;
pub use freedom_svg as svg;

// Resource limits are enforced per page by the browser, not by Canvas2D itself.
pub(crate) const MAX_PAGE_CANVAS_PIXELS: usize = 16 * 1024 * 1024;
pub(crate) const MAX_PAGE_CANVASES: usize = 1024;
