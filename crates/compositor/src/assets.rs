//! Embedded shell assets. Pico + our overrides are concatenated into one
//! cacheable stylesheet; the JS drives the theme toggle, scroll-spy, and
//! Pagefind search box. All vendored into the binary (no runtime CDN).

pub const PICO_CSS: &str = include_str!("../assets/pico.min.css");
pub const OVERRIDES_CSS: &str = include_str!("../assets/compositor.css");
pub const COMPOSITOR_JS: &str = include_str!("../assets/compositor.js");

/// Site-root-relative url (no leading slash) the stylesheet is emitted at.
pub const CSS_URL: &str = "assets/compositor.css";
/// Site-root-relative url (no leading slash) the script is emitted at.
pub const JS_URL: &str = "assets/compositor.js";

/// The full stylesheet served to every page: Pico first, our overrides last so
/// they win the cascade.
pub fn stylesheet() -> String {
    format!("{PICO_CSS}\n{OVERRIDES_CSS}")
}
