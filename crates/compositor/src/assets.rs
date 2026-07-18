//! Embedded shell assets. Pico + our overrides are concatenated into one
//! cacheable stylesheet; the JS drives the theme toggle and scroll-spy. All
//! vendored into the binary (no runtime CDN).

use std::sync::LazyLock;

pub const PICO_CSS: &str = include_str!("../assets/pico.min.css");
pub const OVERRIDES_CSS: &str = include_str!("../assets/compositor.css");
pub const COMPOSITOR_JS: &str = include_str!("../assets/compositor.js");
/// Inline-editing UI, embedded like the shell assets above but served only
/// when `ServedSite::edit_enabled` — see `serve::handle`. Stub content for
/// now; Tasks 7-9 fill in the toggle/autosave logic.
pub const EDITOR_CSS: &str = include_str!("../assets/editor.css");
pub const EDITOR_JS: &str = include_str!("../assets/editor.js");

/// Site-root-relative url (no leading slash) the stylesheet is emitted at.
pub const CSS_URL: &str = "assets/compositor.css";
/// Site-root-relative url (no leading slash) the script is emitted at.
pub const JS_URL: &str = "assets/compositor.js";
/// Site-root-relative url (no leading slash) the editor stylesheet is
/// emitted at — matches the `href` `inject_editor` writes into each page.
pub const EDITOR_CSS_URL: &str = "assets/editor.css";
/// Site-root-relative url (no leading slash) the editor script is emitted
/// at — matches the `src` `inject_editor` writes into each page.
pub const EDITOR_JS_URL: &str = "assets/editor.js";

static STYLESHEET: LazyLock<String> = LazyLock::new(|| format!("{PICO_CSS}\n{OVERRIDES_CSS}"));

/// The full stylesheet served to every page: Pico first, our overrides last so
/// they win the cascade. Concatenated once and cached.
pub fn stylesheet() -> &'static str {
    &STYLESHEET
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overrides_style_unresolved_wikilinks() {
        assert!(
            OVERRIDES_CSS.contains(r#"a[data-wikilink="true"]"#),
            "compositor.css must style unresolved wikilinks"
        );
    }
}
