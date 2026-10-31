//! Font database: bundled OFL fonts, user fonts, the installed system fonts (cataloged the first
//! time a lookup needs them, native only), outline cache.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};

use kurbo::BezPath;
use skrifa::instance::{Location, LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::raw::FileRef;
use skrifa::string::StringId;
use skrifa::{GlyphId, MetadataProvider};

use crate::group::{FamilyInfo, FontGroup};

impl FontSource {
    /// The font file, if the face was loaded from one.
    pub fn path(&self) -> Option<&std::path::Path> {
        match self {
            FontSource::Installed(p) | FontSource::Document(p) => Some(p),
            FontSource::Bundled | FontSource::Memory => None,
        }
    }
}

impl FaceRef {
    /// The handle for a loaded face (memoised per face).
    pub fn of(face: &Arc<FontFace>) -> FaceRef {
        static LEAKED: RwLock<Vec<Option<&FontFace>>> = RwLock::new(Vec::new());
        let id = face.id as usize;
        if let Some(Some(f)) = LEAKED.read().unwrap_or_else(PoisonError::into_inner).get(id) {
            return FaceRef(f);
        }
        let mut w = LEAKED.write().unwrap_or_else(PoisonError::into_inner);
        if w.len() <= id {
            w.resize(id + 1, None);
        }
        let f: &'static FontFace = w[id].unwrap_or_else(|| {
            let keep: &'static Arc<FontFace> = Box::leak(Box::new(face.clone()));
            keep
        });
        w[id] = Some(f);
        FaceRef(f)
    }
    pub fn get(self) -> &'static FontFace {
        self.0
    }
}

impl std::ops::Deref for FaceRef {
    type Target = FontFace;
    fn deref(&self) -> &FontFace {
        self.0
    }
}

impl std::fmt::Debug for FaceRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl PartialEq for FaceRef {
    fn eq(&self, o: &FaceRef) -> bool {
        self.0.id == o.0.id
    }
}

impl std::fmt::Debug for FontFace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FontFace({} {})", self.family, self.style)
    }
}

/// Metadata for a face whose bytes a host supplies on demand through [`FontDb::add_font`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServedFace {
    pub family: String,
    pub style: String,
    pub weight: u16,
    pub italic: bool,
}

/// Process-wide font database: the fonts every document shares (bundled, installed, added), and
/// each open document's own fonts under its scope (see [`FontDb::scoped`]).
pub struct FontDb {
    served: RwLock<Vec<ServedFace>>,
    requests: Mutex<Vec<(String, String)>>,
    faces: RwLock<Vec<Arc<FontFace>>>,
    /// Each open document's fonts (its `Document Fonts` folder), by scope.
    scopes: RwLock<HashMap<u32, Arc<[Arc<FontFace>]>>>,
    /// The document font files read, by path: (length, modification time, faces). A document
    /// opened again finds its unchanged files here instead of reading them again.
    #[cfg(not(target_arch = "wasm32"))]
    document_files: Mutex<HashMap<std::path::PathBuf, (u64, Option<std::time::SystemTime>, Vec<Arc<FontFace>>)>>,
    /// Variable font instances (`Style {wght:650}`), made on first use, by base face and style.
    instances: RwLock<HashMap<(u32, String), Arc<FontFace>>>,
    outlines: Mutex<HashMap<(u32, u32), Arc<BezPath>>>,
    #[cfg(not(target_arch = "wasm32"))]
    catalog: RwLock<Vec<CatalogEntry>>,
    /// Each installed font file read into the shared fonts, with its (length, modification time)
    /// when it was read: [`FontDb::reload_fonts`] reads again the files that changed since.
    #[cfg(not(target_arch = "wasm32"))]
    installed_files: Mutex<HashMap<std::path::PathBuf, FileStamp>>,
    /// The folders the system font scan reads (the platform's font folders for [`FontDb::global`]).
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    font_dirs: Vec<std::path::PathBuf>,
    /// Lists the font files outside `font_dirs` that the system font scan reads too, asked again by
    /// each scan ([`platform_font_files`] for [`FontDb::global`]).
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    font_files: fn() -> Vec<std::path::PathBuf>,
    /// Set once the font folders have been scanned. Lookups by family name wait for the first
    /// scan, so what they find doesn't depend on what ran before them.
    #[cfg(not(target_arch = "wasm32"))]
    cataloged: std::sync::OnceLock<()>,
    /// System fallback state: enabled, and characters no system font covers.
    #[cfg(not(target_arch = "wasm32"))]
    sys: Mutex<SysFallback>,
    /// Pause one fallback after copying catalog paths, with no database locks held.
    #[cfg(all(test, target_os = "linux"))]
    fallback_snapshot_hook: Mutex<Option<FallbackSnapshotHook>>,
}

/// The font files outside the font folders that the platform has available, which
/// [`FontDb::global`]'s scan reads too: on macOS, those CoreText's font manager lists, such as the
/// fonts font services and font managers register from their own folders. None elsewhere.
#[cfg(target_os = "macos")]
use crate::mac_fonts::available_font_files as platform_font_files;

mod db;
mod face;
mod fallback;
mod names;
mod outline;
mod scoped;
mod served;
mod system;

#[cfg(test)]
pub(crate) use face::last_resort_face;
pub use face::{
    DOCUMENT_FONTS_FOLDER, DocumentFonts, FALLBACK_FAMILY, FaceRef, FontFace, FontSource, MAX_DOCUMENT_FONT_BYTES, MAX_DOCUMENT_FONT_FILES,
    MAX_DOCUMENT_FONTS_TOTAL, bundled,
};
#[cfg(test)]
use face::{FontBytes, make_face};
pub(crate) use face::{measured_top, plausible};
#[cfg(all(test, target_os = "linux"))]
use fallback::FallbackSnapshotHook;
#[cfg(not(target_arch = "wasm32"))]
use fallback::SysFallback;
pub use names::{base_style, same_style};
use outline::FlipPen;
pub use scoped::ScopedFonts;
#[cfg(all(not(target_arch = "wasm32"), test))]
use system::file_face_names;
#[cfg(all(test, not(any(target_os = "macos", windows))))]
use system::unix_font_dirs;
#[cfg(not(target_arch = "wasm32"))]
use system::{CatalogEntry, FileStamp};
pub use system::{ReloadedFonts, system_font_dirs};

impl OutlinePen for FlipPen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.0.move_to((x as f64, -y as f64));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.0.line_to((x as f64, -y as f64));
    }
    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        self.0.quad_to((cx0 as f64, -cy0 as f64), (x as f64, -y as f64));
    }
    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.0.curve_to((cx0 as f64, -cy0 as f64), (cx1 as f64, -cy1 as f64), (x as f64, -y as f64));
    }
    fn close(&mut self) {
        self.0.close_path();
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod served_fonts_tests {
    use super::face::enumerate_faces;
    use super::*;

    fn listed(style: &str, weight: u16) -> ServedFace {
        ServedFace { family: "Served Test".into(), style: style.into(), weight, italic: false }
    }

    #[test]
    fn metadata_lists_without_loading_and_real_resolution_requests_once() {
        let db = FontDb::with_font_dirs(vec![]);
        db.register_served_faces(vec![listed("Regular", 400), listed("Heavy", 900)]);
        let fonts = db.scoped(0);
        assert!(fonts.has_family("Served Test"));
        assert!(fonts.family_infos().iter().any(|f| f.family == "Served Test"));
        assert_eq!(fonts.styles("Served Test"), ["Regular", "Heavy"]);
        assert_eq!(fonts.traits_of("Served Test", "Heavy"), (900.0, false));
        assert!(fonts.has_style("Served Test", "Heavy"));
        assert!(!db.is_loaded("Served Test", "Regular"));
        assert!(db.take_requests().is_empty());
        fonts.face("Served Test", "Regular");
        fonts.face("Served Test", "Regular");
        assert_eq!(db.take_requests(), [("Served Test".into(), "Regular".into())]);
        // Draining a failed request must not permanently prevent a later retry.
        fonts.face("Served Test", "Regular");
        assert_eq!(db.take_requests().len(), 1);
        assert_eq!(db.add_font(crate::testing::font_with("Served Test", &['A']).unwrap()), 1);
        assert!(db.is_loaded("Served Test", "Regular"));
        assert_eq!(fonts.face("Served Test", "Regular").family, "Served Test");
        assert!(db.take_requests().is_empty());
        // A loaded regular face must not hide another cataloged style of its family.
        fonts.face("Served Test", "Heavy");
        assert_eq!(db.take_requests(), [("Served Test".into(), "Heavy".into())]);
    }

    #[test]
    fn document_faces_take_precedence_over_served_requests() {
        let db = FontDb::with_font_dirs(vec![]);
        db.register_served_faces(vec![listed("Regular", 400), listed("Heavy", 900)]);
        let data = crate::testing::font_with("Served Test", &['A']).unwrap();
        let own = enumerate_faces(&data)
            .into_iter()
            .filter_map(|(i, family, style, coords)| {
                make_face(FontBytes::Owned(Arc::new(data.clone())), FontSource::Memory, i, family, style, coords)
            })
            .map(Arc::new)
            .collect::<Vec<_>>();
        db.scopes.write().unwrap().insert(1, own.into());
        let fonts = db.scoped(1);
        assert!(fonts.is_loaded("Served Test", "Regular"));
        assert_eq!(fonts.face("Served Test", "Regular").family, "Served Test");
        assert!(!fonts.has_style("Served Test", "Heavy"));
        assert!(!fonts.is_loaded("Served Test", "Heavy"));
        assert_eq!(fonts.face("Served Test", "Heavy").style, "Regular");
        assert!(db.take_requests().is_empty());
    }
}
