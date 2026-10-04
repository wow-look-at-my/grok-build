#![allow(clippy::cast_lossless)] // Hits predate the gate
#![allow(clippy::cast_possible_truncation)] // Hits predate the gate
#![allow(clippy::cast_possible_wrap)]
#![allow(clippy::cast_precision_loss)] // Hits predate the gate
#![allow(clippy::cast_sign_loss)] // Hits predate the gate
#![allow(clippy::expect_used)]
//! Render [Mermaid](https://mermaid.js.org/) diagram source to a rasterized PNG, behind a swappable [`MermaidEngine`] trait.

#![warn(missing_docs)]
#![deny(clippy::indexing_slicing)]

mod engine;
mod mmdc;
mod pure;
mod raster;
mod subprocess;

pub use engine::{MermaidEngine, MermaidError, RenderLimits, render_checked};
pub use mmdc::{MmdcEngine, detect_mmdc};
pub use pure::PureRustEngine;
pub use raster::{MAX_OUTPUT_MEGAPIXELS, rasterize};
pub use subprocess::{SubprocessError, run_with_timeout};

use std::sync::Arc;

/// Which color scheme a diagram should be rendered for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MermaidTheme {
    /// Light surfaces with dark text (e.g. `GrokDay`).
    #[default]
    Light,
    /// Dark surfaces with light text (e.g. `GrokNight`, `TokyoNight`).
    Dark,
}

/// Default opaque surface colors.
pub(crate) const LIGHT_SURFACE: Rgba = Rgba::new(0xFA, 0xFA, 0xFA, 0xFF);
pub(crate) const DARK_SURFACE: Rgba = Rgba::new(0x18, 0x18, 0x1B, 0xFF);

impl MermaidTheme {
    /// The default opaque surface color a diagram blends into for this theme.
    pub fn surface_background(self) -> Rgba {
        match self {
            MermaidTheme::Light => LIGHT_SURFACE,
            MermaidTheme::Dark => DARK_SURFACE,
        }
    }
}

/// A straight 8-bit-per-channel, non-premultiplied RGBA color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    /// Construct an [`Rgba`] from its channels.
    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// Format as an opaque `#RRGGBB` hex string (alpha is ignored).
    pub fn to_hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }
}

/// Parameters controlling a single diagram render.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderParams {
    /// Color scheme to render for.
    pub theme: MermaidTheme,
    /// Target output width in pixels.
    pub target_width_px: u32,
    /// Hard ceiling on output height in pixels; the render is scaled down to fit.
    pub max_height_px: u32,
    /// Oversample factor applied **only** when `target_width_px == 0`.
    pub scale: f32,
    /// Minimum output width in pixels; `0` disables.
    pub min_width_px: u32,
    /// Opaque background fill. `None` renders on a transparent background so the terminal cell color shows through.
    pub background: Option<Rgba>,
}

impl Default for RenderParams {
    fn default() -> Self {
        Self {
            theme: MermaidTheme::Light,
            target_width_px: 1024,
            max_height_px: 4096,
            scale: 1.0,
            min_width_px: 0,
            background: None,
        }
    }
}

impl RenderParams {
    /// Sizing tuned for opening a PNG in an OS image viewer. Prefers 2x the
    /// SVG's intrinsic size, raises small diagrams to at least
    /// `min_width_px`, and allows a taller canvas than the in-terminal
    /// sizing.
    pub fn for_os_viewer(theme: MermaidTheme, min_width_px: u32, max_height_px: u32) -> Self {
        Self {
            theme,
            // Drive from `scale` and `min_width_px` so large SVGs keep aspect at 2x and small SVGs are upscaled to a readable minimum width
            target_width_px: 0,
            max_height_px,
            scale: 2.0,
            min_width_px,
            background: Some(theme.surface_background()),
        }
    }
}

/// A rendered diagram: PNG bytes plus the exact raster dimensions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedDiagram {
    /// The encoded PNG image.
    pub png: Vec<u8>,
    /// Output width in pixels.
    pub width_px: u32,
    /// Output height in pixels.
    pub height_px: u32,
}

/// Construct the default engine: the offline, pure-Rust [`PureRustEngine`].
pub fn default_engine() -> Arc<dyn MermaidEngine> {
    Arc::new(PureRustEngine::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_surface_background_differs_light_vs_dark() {
        let light = MermaidTheme::Light.surface_background();
        let dark = MermaidTheme::Dark.surface_background();
        assert_ne!(light, dark, "light and dark must map to different surfaces");
        // Light surface is brighter than dark on every channel; both opaque.
        assert!(light.r > dark.r && light.g > dark.g && light.b > dark.b);
        assert_eq!(light.a, 0xFF);
        assert_eq!(dark.a, 0xFF);
    }

    #[test]
    fn rgba_to_hex_is_opaque_rrggbb() {
        assert_eq!(Rgba::new(0x12, 0xAB, 0xCD, 0xFF).to_hex(), "#12ABCD");
        // Alpha is ignored.
        assert_eq!(Rgba::new(0, 0, 0, 0).to_hex(), "#000000");
        // The shared dark surface const renders to the hex the dark theme uses.
        assert_eq!(DARK_SURFACE.to_hex(), "#18181B");
    }

    #[test]
    fn default_params_are_target_width_driven() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="50" viewBox="0 0 100 50"><rect width="10" height="10" fill="#0000ff"/></svg>"##;
        let out = rasterize(svg, &RenderParams::default()).expect("render");
        assert_eq!(
            out.width_px, 1024,
            "default target_width_px should drive width"
        );
    }

    #[test]
    fn default_engine_is_constructible_and_send_sync() {
        fn assert_send_sync<T: Send + Sync>(_: &T) {}
        let engine = default_engine();
        assert_send_sync(&engine);
    }
}
