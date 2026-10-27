//! The window's panels. Each draws itself and reports what the user asked for as
//! [`Command`]s, which [`crate::app::OpenDrawApp`] carries out after the frame's UI is built —
//! so a panel never needs the whole app mutably while it is being drawn.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::io::Export;
use crate::three_d::{Export3d, ForceField};

pub mod icons;
pub mod menu;
pub mod palette;
pub mod properties;
pub mod status;
pub mod tabs;
pub mod view3d;

/// Which views share the window: the 2D drawing, the 3D model, or both side by side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ViewMode {
    TwoD,
    #[default]
    Split,
    ThreeD,
}

impl ViewMode {
    pub const ALL: [ViewMode; 3] = [ViewMode::TwoD, ViewMode::Split, ViewMode::ThreeD];

    pub fn label(self) -> &'static str {
        match self {
            ViewMode::TwoD => "2D",
            ViewMode::Split => "2D ＋ 3D",
            ViewMode::ThreeD => "3D",
        }
    }

    pub fn shows_2d(self) -> bool {
        self != ViewMode::ThreeD
    }

    pub fn shows_3d(self) -> bool {
        self != ViewMode::TwoD
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
    /// Generate the active document's 3D model from its drawing.
    Generate3d,
    /// Minimise the active document's 3D model further.
    Optimize3d,
    Export3d(Export3d),
    /// The force field the active document's next generation or optimisation uses.
    SetForceField(ForceField),
    About,
}
