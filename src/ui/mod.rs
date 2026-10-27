//! The window's panels. Each draws itself and reports what the user asked for as
//! [`Command`]s, which [`crate::app::OpenDrawApp`] carries out after the frame's UI is built —
//! so a panel never needs the whole app mutably while it is being drawn.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::io::Export;
use crate::three_d::{Export3d, ForceField};

pub mod fragments;
pub mod icons;
pub mod menu;
pub mod palette;
pub mod periodic_table;
pub mod properties;
pub mod status;
pub mod tabs;
pub mod view3d;
pub mod viewport3d;

/// What the window's central area shows: the 2D drawing or the 3D model, one at a time. The
/// right panel follows it (解析 for the drawing, 3D 編集 for the model).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(from = "SavedViewMode")]
pub enum ViewMode {
    #[default]
    TwoD,
    ThreeD,
}

/// A view mode as saved by any version: the 2D＋3D split view that once existed (`Split`), and
/// anything else unknown, opens the drawing.
#[derive(Deserialize)]
enum SavedViewMode {
    ThreeD,
    #[serde(other)]
    Other,
}

impl From<SavedViewMode> for ViewMode {
    fn from(saved: SavedViewMode) -> Self {
        match saved {
            SavedViewMode::ThreeD => ViewMode::ThreeD,
            SavedViewMode::Other => ViewMode::TwoD,
        }
    }
}

impl ViewMode {
    pub const ALL: [ViewMode; 2] = [ViewMode::TwoD, ViewMode::ThreeD];

    pub fn label(self) -> &'static str {
        match self {
            ViewMode::TwoD => "2D",
            ViewMode::ThreeD => "3D",
        }
    }

    pub fn is_3d(self) -> bool {
        self == ViewMode::ThreeD
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    New,
    Open,
    OpenPath(PathBuf),
    Save,
    SaveAs,
    Export(Export),
    /// Close the tab at this index (asks first when it has unsaved changes).
    CloseTab(usize),
    CloseActive,
    SelectTab(usize),
    Quit,
    Undo,
    Redo,
    Copy,
    Paste,
    FitView,
    ToggleCleanup,
    ToggleProperties,
    SetView(ViewMode),
    /// Generate the active document's 3D model from its drawing (again: the same molecule
    /// as the current model when that is part of the drawing).
    Generate3d,
    /// 3D化: generate the 3D model of the molecules holding these atoms (the whole drawing
    /// for `None`) and show it in the 3D view.
    Make3d(Option<Vec<u32>>),
    /// Minimise the active document's 3D model further.
    Optimize3d,
    Export3d(Export3d),
    /// The force field the active document's next generation or optimisation uses.
    SetForceField(ForceField),
    About,
    /// Open the 断片の管理 window.
    ManageFragments,
    /// Switch the active document's editor to the arrow tool of this kind.
    ArrowTool(chembuider_rs::molecule::ArrowKind),
    /// Switch the active document's editor to the text tool (reaction conditions).
    TextTool,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The 2D＋3D split view is gone: a saved `Split` (or any mode this version does not know)
    /// loads as the drawing, and the modes that remain round-trip.
    #[test]
    fn a_saved_split_view_or_unknown_mode_loads_as_2d() {
        for (saved, expected) in [
            ("Split", ViewMode::TwoD),
            ("SomethingNewer", ViewMode::TwoD),
            ("TwoD", ViewMode::TwoD),
            ("ThreeD", ViewMode::ThreeD),
        ] {
            let mode: ViewMode = serde_json::from_str(&format!("\"{saved}\""))
                .unwrap_or_else(|e| panic!("{saved} did not load: {e}"));
            assert_eq!(mode, expected, "{saved}");
        }
        for mode in ViewMode::ALL {
            let text = serde_json::to_string(&mode).unwrap();
            assert_eq!(serde_json::from_str::<ViewMode>(&text).unwrap(), mode);
        }
    }
}
