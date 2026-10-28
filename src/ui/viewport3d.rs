//! The 3D viewport: moleucle_3dview_rs's renderer and camera, assembled here rather
//! than through its `InteractiveMoleculeViewport`.
//!
//! That widget reports atom clicks only (the bond a pick hits is dropped), binds the wheel to
//! the camera and keeps the camera to itself. Editing needs bond clicks (to choose a bond to
//! turn), a right click (to fix an atom), the wheel (to turn the bond) and the camera (to put
//! labels next to atoms), so the render half of its `show` is reproduced here from the crate's
//! public parts — `MoleculeViewer`, `OrbitalCamera`, `OffscreenRenderer`, `RenderFrameState` —
//! as chem_3d_builder_rs's viewport does, and the input half is this app's own. Picking is
//! its own too ([`pick`]): the library's tests every atom as a ball-and-stick ball, whatever
//! radius the style draws it at.

use eframe::egui_wgpu;
use egui::{PointerButton, Pos2, Rect, Sense};
use lin_alg::f32::{Vec2, Vec3};
use moleucle_3dview_rs::{
    AdditionalRender, Atom, AtomGroup, AtomGroupRender, AtomGroupState, AtomPairRender,
    AtomPairState, Bond, Element, GpuPipeline, LodSettings, Molecule, MoleculeViewer,
    OffscreenRenderer, PeriodicImages, RenderFrameState, RenderStyle, SharedRenderStates,
    SimulationCellRender, SimulationCellState,
    camera::{Camera, OrbitalCamera},
    new_shared_states, set_state_by_type, vdw_radius,
};
use serde::{Deserialize, Serialize};

use crate::three_d::{Atom3d, Bond3d, Model3d};

mod pick;

/// Å → nm, the viewer's unit.
const NM_PER_ANGSTROM: f32 = 0.1;

/// How the model is drawn. The app's own enum (the library's is not serialisable) so the choice
/// is remembered between runs.
///
/// Names saved by earlier versions load as the style that draws them now: `Light` (which was
/// ball-and-stick with small atoms) and `BallStick` as [`Style3d::BallAndStick`],
/// `SpaceFilling` as [`Style3d::Spheres`]; any other unknown name as the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Style3d {
    /// Each atom a sphere at its van der Waals radius, no bonds.
    #[serde(alias = "SpaceFilling")]
    Spheres,
    /// Bonds as lines only.
    Wireframe,
    /// Small spheres joined by thin sticks. Last, as serde requires of the catch-all.
    #[default]
    #[serde(alias = "Light", alias = "BallStick", other)]
    BallAndStick,
}

impl Style3d {
    pub const ALL: [Style3d; 3] = [Style3d::BallAndStick, Style3d::Spheres, Style3d::Wireframe];

    pub fn label(self) -> &'static str {
        match self {
            Style3d::BallAndStick => t!("three_d.view.style_ball_and_stick"),
            Style3d::Spheres => t!("three_d.view.style_spheres"),
            Style3d::Wireframe => t!("three_d.view.style_wireframe"),
        }
    }

    fn render_style(self) -> RenderStyle {
        match self {
            // Ball-and-stick geometry with the atoms shrunk (`BALL_AND_STICK_ATOM_SCALE`) at a
            // coarse sphere mesh.
            Style3d::BallAndStick => RenderStyle::BallStick,
            // Ray-traced sphere impostors: a few bytes of instance data an atom instead of a
            // mesh, so the cost does not grow with the sphere resolution and large assemblies
            // stay as light as in ball-and-stick (the mesh `BallOnly` would not).
            Style3d::Spheres => RenderStyle::Circles,
            Style3d::Wireframe => RenderStyle::Wireframe,
        }
    }

    /// Atom radius as a fraction of the van der Waals radius. It sizes the drawn spheres and the
    /// spheres a click is tested against (wireframe draws none, so it keeps ball-and-stick's).
    fn atom_scale(self) -> f32 {
        match self {
            Style3d::Spheres => 1.0,
            Style3d::BallAndStick | Style3d::Wireframe => BALL_AND_STICK_ATOM_SCALE,
        }
    }
}

/// Atom radius in ball-and-stick, as a fraction of the van der Waals radius: carbon 0.43 Å,
/// hydrogen 0.30 Å, against the library ball-and-stick's 0.3 (0.51 Å for carbon). The sticks
/// are the library's fixed 0.18 Å, so this keeps the atoms clearly wider than the bonds.
const BALL_AND_STICK_ATOM_SCALE: f32 = 0.25;

/// The radius (nm) of the sphere a highlighted atom is wrapped in. It has to clear the atom:
/// 0.85 Å is about twice a ball-and-stick carbon; in spheres it clears the largest van der
/// Waals radius the library knows (iodine, 1.98 Å).
fn highlight_radius(style: RenderStyle) -> f32 {
    match style {
        RenderStyle::Circles => 0.21,
        _ => 0.085,
    }
}

/// The highlight spheres (selection, fixed atom, the side that moves, clashes), sized for the
/// style drawn: the library's overlay takes one radius, and a ball-and-stick highlight would
/// vanish inside a van der Waals sphere. The style is part of the overlay's revision, so a
/// change of style redraws it.
struct Highlights {
    ball_and_stick: AtomGroupRender,
    spheres: AtomGroupRender,
}

impl Highlights {
    fn new() -> Self {
        let sized = |style| {
            let mut groups = AtomGroupRender::new();
            groups.set_radius(highlight_radius(style));
            groups
        };
        Self {
            ball_and_stick: sized(RenderStyle::BallStick),
            spheres: sized(RenderStyle::Circles),
        }
    }
}

impl AdditionalRender for Highlights {
    fn update_scene(&self, scene: &mut moleucle_3dview_rs::Scene, frame: &RenderFrameState<'_>) {
        match frame.render_style {
            RenderStyle::Circles => self.spheres.update_scene(scene, frame),
            _ => self.ball_and_stick.update_scene(scene, frame),
        }
    }

    fn gpu_pipeline(&self) -> GpuPipeline {
        self.ball_and_stick.gpu_pipeline()
    }

    fn revision(&self, frame: &RenderFrameState<'_>) -> Option<u64> {
        frame.overlay_revision::<AtomGroupState>()
    }
}

/// Sphere mesh resolution (latitude bands; twice as many longitudes). The library's default
/// and minimum, 3, turns atoms this small into hexagons; 6 is still coarse (72 triangles a
/// sphere), and the library halves it on its own when a large model would overflow.
const SPHERE_RESOLUTION: usize = 6;

/// From this many atoms the sphere mesh follows the camera distance (level of detail), so a
/// large assembly stays smooth to turn.
const LOD_FROM_ATOMS: usize = 5_000;

/// The sphere meshes used with level of detail: near, middle and far.
const LOD_SETTINGS: LodSettings = LodSettings {
    enabled: true,
    distance_check_fps: 12.0,
    high_detail_max_distance: 4.0,
    medium_detail_max_distance: 12.0,
    high_detail_mesh_resolution: SPHERE_RESOLUTION,
    medium_detail_mesh_resolution: 4,
    low_detail_mesh_resolution: 3,
};

/// Generic display data, for anything other than the 3D model: atoms and bonds with a
/// source and revision that say when they changed. The assembly viewer shows its build
/// through this (molecule membership and colouring are the caller's).
#[derive(Debug, Clone, Copy)]
pub struct Scene<'a> {
    /// What is shown, as the caller tells its contents apart. Another source is framed anew.
    pub source: u64,
    /// Changes whenever the atoms or bonds do; the viewer reloads only then.
    pub revision: u64,
    pub atoms: &'a [Atom3d],
    pub bonds: &'a [Bond3d],
}

/// What a ray through the pointer hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    Atom(usize),
    /// A bond, by index into the model's bonds.
    Bond(usize),
    Nothing,
}

/// What the user did to the view this frame.
#[derive(Debug, Clone, Copy)]
pub struct Frame3d {
    pub rect: Rect,
    pub primary: Option<Hit>,
    /// A right click (not the end of a right drag, which pans).
    pub secondary: Option<Hit>,
    pub hovered: Option<Hit>,
    /// Wheel travel in notches, when the caller claimed the wheel (the camera did not zoom).
    pub wheel_notches: f32,
    /// This frame's primary drag in points, when the caller claimed the drag (see
    /// [`Viewport3d::claim_drag`]; the camera did not move).
    pub drag: Option<egui::Vec2>,
}

/// Everything a hover pick depends on, so a still pointer over a still scene reuses the last
/// answer instead of ray-testing every atom and bond each frame.
#[derive(Clone, Copy, PartialEq, Eq)]
struct HoverKey {
    pointer: [u32; 2],
    view_proj: [u32; 16],
    revision: u64,
}

/// The structure on screen, to tell an edit (move the atoms) from a new structure (rebuild and
/// reframe).
#[derive(Debug, Clone, PartialEq, Default)]
struct Shown {
    elements: Vec<String>,
    bonds: Vec<(usize, usize, u8)>,
    coords: Vec<[f64; 3]>,
}

pub struct Viewport3d {
    viewer: MoleculeViewer,
    camera: OrbitalCamera,
    offscreen: OffscreenRenderer,
    shared: SharedRenderStates,
    last_hover: Option<(HoverKey, Hit)>,
    /// The atoms bucketed for picking at the radii they are drawn at, rebuilt when the
    /// viewer's revision (geometry or radii) moves on.
    pick_grid: Option<pick::AtomGrid>,
    shown: Option<Shown>,
    /// Frame the model on the next draw, once the view's aspect ratio is known.
    reframe: bool,
    style: Style3d,
    clear_color: [f32; 4],
    /// The [`Scene`] shown (source, revision) and the key of its colours.
    scene: Option<(u64, u64)>,
    scene_colors: Option<u64>,
    /// The unit cell last handed to the renderer (see [`Viewport3d::set_cell`]).
    cell: Option<SimulationCellState>,
    /// The caller takes this frame's primary drag (moving a molecule) instead of the camera.
    drag_claimed: bool,
}

/// Periodic images: the repeated cell's vectors in Å and the copies per axis.
pub type CellImages = ([[f64; 3]; 3], [u32; 3]);

/// The unit cell's outline: a warm grey that reads on the dark background without competing
/// with the atoms.
const CELL_COLOR: (f32, f32, f32) = (0.86, 0.78, 0.52);

impl Default for Viewport3d {
    fn default() -> Self {
        Self::new()
    }
}

impl Viewport3d {
    pub fn new() -> Self {
        let mut offscreen = OffscreenRenderer::new();
        // Highlights (selection, fixed atom, the side that moves, clashes) and the bond axis.
        offscreen.add_additional_render(Box::new(Highlights::new()));
        let mut pairs = AtomPairRender::new();
        pairs.set_color((0.95, 0.55, 0.20));
        pairs.set_radius(0.028);
        offscreen.add_additional_render(Box::new(pairs));
        // A crystal's unit cell (draws nothing until `set_cell` gives it one).
        let mut cell = SimulationCellRender::new();
        cell.set_color(CELL_COLOR);
        cell.set_edge_radius(0.006);
        offscreen.add_additional_render(Box::new(cell));
        offscreen.set_render_style(Style3d::default().render_style());
        offscreen.set_mesh_resolution(SPHERE_RESOLUTION);
        let mut camera = OrbitalCamera::default();
        camera.set_aspect(1.0);
        Self {
            viewer: MoleculeViewer::new(),
            camera,
            offscreen,
            shared: new_shared_states(),
            last_hover: None,
            pick_grid: None,
            shown: None,
            reframe: false,
            style: Style3d::default(),
            clear_color: [0.08, 0.10, 0.14, 1.0],
            scene: None,
            scene_colors: None,
            cell: None,
            drag_claimed: false,
        }
    }

    /// Whether the next [`Viewport3d::show`] hands a primary drag to the caller
    /// ([`Frame3d::drag`]) instead of orbiting or panning the camera.
    pub fn claim_drag(&mut self, claim: bool) {
        self.drag_claimed = claim;
    }

    /// The point the camera turns about, in ångström.
    pub fn view_center(&self) -> [f64; 3] {
        let c = self.camera.center;
        [c.x, c.y, c.z].map(|v| f64::from(v / NM_PER_ANGSTROM))
    }

    /// The camera's directions in the scene and the ångström a screen point spans at the depth
    /// of `at` (Å), for moving things with the pointer in `rect`.
    pub fn screen_basis(&self, rect: Rect, at: [f64; 3]) -> crate::assembly::place::ScreenBasis {
        let rotation = self.camera.camera_rotation();
        let axis = |v: Vec3| {
            let w = rotation.rotate_vec(v);
            [f64::from(w.x), f64::from(w.y), f64::from(w.z)]
        };
        // Camera space: +x right, +y up, +z into the screen (see `project`).
        let forward = axis(Vec3::new(0.0, 0.0, 1.0));
        let depth_nm = (to_nm(at) - self.camera.position())
            .dot(rotation.rotate_vec(Vec3::new(0.0, 0.0, 1.0)))
            .max(1e-3);
        let tan_half = (self.camera.fov_y() * 0.5).tan();
        let angstrom_per_point = f64::from(2.0 * depth_nm * tan_half / NM_PER_ANGSTROM)
            / f64::from(rect.height().max(1.0));
        crate::assembly::place::ScreenBasis {
            right: axis(Vec3::new(1.0, 0.0, 0.0)),
            up: axis(Vec3::new(0.0, 1.0, 0.0)),
            toward_viewer: forward.map(|v| -v),
            angstrom_per_point,
        }
    }

    pub fn set_style(&mut self, style: Style3d) {
        if style != self.style {
            self.style = style;
            self.offscreen.set_render_style(style.render_style());
            self.size_atoms();
        }
    }

    /// Give the shown atoms the radii of the current style.
    fn size_atoms(&mut self) {
        let scale = self.style.atom_scale();
        let radii = self.viewer.molecule().map(|molecule| {
            molecule
                .atoms
                .iter()
                .map(|a| vdw_radius(&a.element) * scale)
                .collect()
        });
        if radii.is_some() {
            self.viewer.set_atom_radii(radii);
            self.last_hover = None;
        }
    }

    /// The colour behind the model, as egui paints it (the renderer wants linear values).
    pub fn set_background(&mut self, color: egui::Color32) {
        // The colour target is not sRGB, so the gamma-encoded values go in as they are.
        let [r, g, b, _] = color.to_array();
        self.clear_color = [
            f32::from(r) / 255.0,
            f32::from(g) / 255.0,
            f32::from(b) / 255.0,
            1.0,
        ];
    }

    /// Show `model`: a new structure is loaded and framed; the same structure with atoms moved
    /// (an edit, an optimisation) keeps the camera where the user put it.
    pub fn sync(&mut self, model: Option<&Model3d>) {
        self.scene = None;
        let Some(model) = model else {
            if self.shown.take().is_some() {
                self.viewer.take_molecule();
                self.last_hover = None;
            }
            return;
        };
        let coords: Vec<[f64; 3]> = model.atoms.iter().map(|a| a.pos).collect();
        if let Some(shown) = &mut self.shown
            && shown.elements.len() == model.atoms.len()
            && shown
                .elements
                .iter()
                .zip(&model.atoms)
                .all(|(e, a)| *e == a.element)
            && shown.bonds.len() == model.bonds.len()
            && shown
                .bonds
                .iter()
                .zip(&model.bonds)
                .all(|(s, b)| *s == (b.begin, b.end, b.order))
        {
            if shown.coords != coords {
                let angstrom: Vec<[f32; 3]> = coords
                    .iter()
                    .map(|p| [p[0] as f32, p[1] as f32, p[2] as f32])
                    .collect();
                if self.viewer.update_positions_angstrom(&angstrom).is_ok() {
                    shown.coords = coords;
                    self.last_hover = None;
                }
            }
            return;
        }

        let atoms = model
            .atoms
            .iter()
            .map(|a| Atom::new(to_nm(a.pos), Element::new(&a.element)))
            .collect();
        let bonds = model
            .bonds
            .iter()
            .map(|b| Bond::new(b.begin, b.end, b.order.min(3)))
            .collect();
        self.viewer
            .set_molecule(Molecule::from_atoms_bonds(atoms, bonds));
        self.viewer.set_atom_colors(Some(
            model
                .atoms
                .iter()
                .map(|a| element_color(&a.element))
                .collect(),
        ));
        self.size_atoms();
        self.shown = Some(Shown {
            elements: model.atoms.iter().map(|a| a.element.clone()).collect(),
            bonds: model
                .bonds
                .iter()
                .map(|b| (b.begin, b.end, b.order))
                .collect(),
            coords,
        });
        self.last_hover = None;
        self.reframe = true;
    }

    /// Show `scene`, reloading only when its source or revision changed: comparing every atom
    /// each frame would cost more than drawing a large assembly. `colors` is called for new
    /// atoms or a new `colors_key` (the colouring changed).
    pub fn sync_scene(
        &mut self,
        scene: Option<Scene<'_>>,
        colors_key: u64,
        colors: impl FnOnce() -> Vec<[f32; 4]>,
    ) {
        self.shown = None;
        let Some(scene) = scene else {
            if self.scene.take().is_some() {
                self.viewer.take_molecule();
                self.last_hover = None;
            }
            return;
        };
        let key = (scene.source, scene.revision);
        if self.scene == Some(key) {
            if self.scene_colors != Some(colors_key) {
                self.viewer.set_atom_colors(Some(colors()));
                self.scene_colors = Some(colors_key);
            }
            return;
        }
        let new_source = self.scene.is_none_or(|(source, _)| source != scene.source);
        // A different number of atoms (a step added, a count changed) is framed anew, so the
        // whole assembly stays in view; moved atoms (a radius changed) keep the camera.
        let resized = self
            .viewer
            .molecule()
            .is_none_or(|m| m.atoms.len() != scene.atoms.len());
        let atoms = scene
            .atoms
            .iter()
            .map(|a| Atom::new(to_nm(a.pos), Element::new(&a.element)))
            .collect();
        let bonds = scene
            .bonds
            .iter()
            .map(|b| Bond::new(b.begin, b.end, b.order.min(3)))
            .collect();
        self.viewer
            .set_molecule(Molecule::from_atoms_bonds(atoms, bonds));
        self.viewer.set_atom_colors(Some(colors()));
        self.size_atoms();
        let lod = if scene.atoms.len() >= LOD_FROM_ATOMS {
            LOD_SETTINGS
        } else {
            LodSettings::default()
        };
        if self.offscreen.lod_settings() != lod {
            self.offscreen.set_lod_settings(lod);
            if !lod.enabled {
                self.offscreen.set_mesh_resolution(SPHERE_RESOLUTION);
            }
        }
        self.scene = Some(key);
        self.scene_colors = Some(colors_key);
        self.last_hover = None;
        if new_source || resized {
            self.reframe = true;
        }
    }

    /// Frame the whole model.
    pub fn reset_view(&mut self) {
        let Some(molecule) = self.viewer.molecule() else {
            return;
        };
        if molecule.atoms.is_empty() {
            return;
        }
        self.camera.center = molecule.center();
        // Far enough for the whole model (its radius includes the atoms' van der Waals spheres)
        // to fit the narrower side of the view, with a little margin.
        let tan_half = (self.camera.fov_y() * 0.5).tan() * self.camera.aspect.min(1.0);
        self.camera.radius = molecule.radius().max(0.2) / tan_half.max(0.05) * 1.1;
    }

    /// A crystal's display, given as cell vectors in Å (rows **a**, **b**, **c**): `outline`
    /// draws that cell's edges from the origin, and `images` repeats the whole scene along
    /// its cell's vectors, the given number of times per axis, the scene itself included (the
    /// copies sit on both sides, as moleucle_3dview_rs's `PeriodicImages` centres them).
    /// Clicking a copy reports the atom it copies. Kept apart from [`Scene`] because these
    /// are display options that change without the atoms changing.
    pub fn set_cell(&mut self, outline: Option<[[f64; 3]; 3]>, images: Option<CellImages>) {
        let state = outline.map_or_else(SimulationCellState::default, |v| {
            SimulationCellState::triclinic(v.map(to_nm))
        });
        // Stored only on a change: storing bumps the overlay's revision, which rebuilds it.
        if self.cell != Some(state) {
            set_state_by_type(&self.shared, state);
            self.cell = Some(state);
        }
        let images = images
            .map(|(v, n)| PeriodicImages::new(SimulationCellState::triclinic(v.map(to_nm)), n))
            .filter(|i| !i.is_trivial());
        if self.viewer.periodic_images() != images.as_ref() {
            self.viewer.set_periodic_images(images);
            self.last_hover = None;
        }
    }

    /// Highlight groups of atoms (each atom in at most one group) and draw `pairs` as rods.
    pub fn set_highlights(&mut self, groups: Vec<AtomGroup>, pairs: Vec<(usize, usize)>) {
        set_state_by_type(
            &self.shared,
            AtomGroupState {
                visible: !groups.is_empty(),
                groups,
                opacity: 0.45,
            },
        );
        set_state_by_type(&self.shared, AtomPairState::new(pairs));
    }

    /// Draw the view into the space left in `ui` and read the pointer. With `claim_wheel` the
    /// wheel is reported instead of zooming.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        render_state: &egui_wgpu::RenderState,
        claim_wheel: bool,
    ) -> Result<Frame3d, String> {
        let available = ui.available_size_before_wrap();
        let ppp = ui.ctx().pixels_per_point();
        // Rendered at physical resolution so the model is sharp on a scaled display.
        let width = (available.x * ppp).max(1.0) as u32;
        let height = (available.y * ppp).max(1.0) as u32;
        self.camera.set_aspect(width as f32 / height as f32);
        if std::mem::take(&mut self.reframe) {
            self.reset_view();
        }

        if self.offscreen.lod_enabled()
            && let Some(molecule) = self.viewer.molecule()
        {
            let distance = (self.camera.position() - molecule.center()).magnitude();
            self.offscreen.submit_lod_distance(distance);
        }
        self.offscreen
            .ensure_resources(render_state, width, height)?;

        let view_proj = self.camera.view_projection().data;
        let rotation = self.camera.camera_rotation();
        let frame = RenderFrameState::new(
            self.viewer.molecule(),
            view_proj,
            Some(self.camera.position()),
            self.camera.fov_y(),
            rotation.rotate_vec(Vec3::new(1.0, 0.0, 0.0)),
            rotation.rotate_vec(Vec3::new(0.0, 1.0, 0.0)),
            rotation.rotate_vec(Vec3::new(0.0, 0.0, 1.0)),
            self.viewer.color_fn,
            Some(&self.shared),
            self.offscreen.render_style(),
            self.offscreen.mesh_resolution(),
            self.offscreen.is_low_mode(),
            self.viewer.molecule_opacity,
        )
        .with_geometry_revision(self.viewer.revision())
        .with_periodic_images(self.viewer.periodic_images())
        .with_visible_atoms(self.viewer.visible_atoms())
        .with_atom_attrs(
            self.viewer.atom_radii.as_deref(),
            self.viewer.atom_colors.as_deref(),
        )
        .with_clear_color(self.clear_color);
        self.offscreen
            .render_frame_with_state(render_state, &frame)?;

        let texture = self
            .offscreen
            .texture_id()
            .ok_or_else(|| t!("three_d.error.texture").to_owned())?;
        let response = ui.add(
            egui::Image::from_texture(egui::load::SizedTexture::new(texture, available))
                .sense(Sense::click_and_drag()),
        );
        Ok(self.interact(ui.ctx(), &response, claim_wheel))
    }

    fn interact(
        &mut self,
        ctx: &egui::Context,
        response: &egui::Response,
        claim_wheel: bool,
    ) -> Frame3d {
        let rect = response.rect;
        let mut out = Frame3d {
            rect,
            primary: None,
            secondary: None,
            hovered: None,
            wheel_notches: 0.0,
            drag: None,
        };

        if response.hovered()
            && !response.dragged()
            && let Some(pointer) = response.hover_pos()
        {
            let key = self.hover_key(pointer);
            let hit = match self.last_hover {
                Some((cached, hit)) if cached == key => hit,
                _ => {
                    let hit = self.pick(pointer, rect);
                    self.last_hover = Some((key, hit));
                    hit
                }
            };
            out.hovered = Some(hit);
        }

        if response.hovered() {
            if claim_wheel {
                // Raw notches, not the smoothed delta: a stepped rotation would otherwise lose
                // the tail of each notch's easing once the app stops repainting.
                out.wheel_notches = wheel_notches(ctx);
            } else {
                let scroll = ctx.input(|i| i.smooth_scroll_delta.y);
                if scroll.abs() > f32::EPSILON {
                    // Proportional, so a notch zooms as much on a small molecule as a large one.
                    self.camera.radius = (self.camera.radius * (-scroll * 0.002).exp()).max(0.05);
                }
            }
        }

        let delta = response.drag_delta();
        if response.dragged_by(PointerButton::Primary) && self.drag_claimed {
            out.drag = Some(delta);
        } else if response.dragged_by(PointerButton::Primary) {
            if ctx.input(|i| i.modifiers.shift) {
                self.camera.pan(Vec2::new(delta.x * 0.01, delta.y * 0.01));
            } else {
                self.camera.orbit(delta.x * 0.006, delta.y * 0.006);
            }
        }
        if response.dragged_by(PointerButton::Secondary)
            || response.dragged_by(PointerButton::Middle)
        {
            self.camera.pan(Vec2::new(delta.x * 0.01, delta.y * 0.01));
        }

        if response.double_clicked_by(PointerButton::Primary)
            && let Some(pointer) = response.interact_pointer_pos()
            && self.pick(pointer, rect) == Hit::Nothing
        {
            self.reset_view();
        } else if response.clicked_by(PointerButton::Primary)
            && let Some(pointer) = response.interact_pointer_pos()
        {
            out.primary = Some(self.pick(pointer, rect));
        }
        if response.clicked_by(PointerButton::Secondary)
            && let Some(pointer) = response.interact_pointer_pos()
        {
            out.secondary = Some(self.pick(pointer, rect));
        }
        out
    }

    /// What is under `pointer`, tested against the atoms at the radii they are drawn at (see
    /// [`pick`]).
    fn pick(&mut self, pointer: Pos2, rect: Rect) -> Hit {
        let local = pointer - rect.min;
        let (origin, dir) = self.camera.ray_from_screen(
            local.x,
            local.y,
            rect.width().max(1.0),
            rect.height().max(1.0),
        );
        let revision = self.viewer.revision();
        let grid = match self.pick_grid.take() {
            Some(grid) if grid.revision == revision => grid,
            _ => pick::AtomGrid::build(&self.viewer),
        };
        let hit = match pick::pick(&self.viewer, &grid, origin, dir) {
            Some(pick::Picked::Atom(i)) => Hit::Atom(i),
            Some(pick::Picked::Bond(i)) => Hit::Bond(i),
            None => Hit::Nothing,
        };
        self.pick_grid = Some(grid);
        hit
    }

    fn hover_key(&self, pointer: Pos2) -> HoverKey {
        let mut view_proj = [0u32; 16];
        for (bits, value) in view_proj.iter_mut().zip(self.camera.view_projection().data) {
            *bits = value.to_bits();
        }
        HoverKey {
            pointer: [pointer.x.to_bits(), pointer.y.to_bits()],
            view_proj,
            revision: self.viewer.revision(),
        }
    }

    /// Where the point `pos` (Å) appears in `rect`, or `None` behind the camera. The exact
    /// inverse of the camera's `ray_from_screen`, which picking uses.
    pub fn project(&self, rect: Rect, pos: [f64; 3]) -> Option<Pos2> {
        project(&self.camera, rect, pos)
    }

    pub fn free(&mut self, render_state: &egui_wgpu::RenderState) {
        self.offscreen.free_egui_texture(render_state);
    }
}

fn project(camera: &OrbitalCamera, rect: Rect, pos: [f64; 3]) -> Option<Pos2> {
    let world = to_nm(pos) - camera.position();
    let local = camera.camera_rotation().inverse().rotate_vec(world);
    if local.z <= 1e-6 {
        return None;
    }
    let (width, height) = (rect.width().max(1.0), rect.height().max(1.0));
    let tan_half = (camera.fov_y() * 0.5).tan();
    let ndc_x = local.x / (local.z * tan_half * (width / height));
    let ndc_y = local.y / (local.z * tan_half);
    Some(Pos2::new(
        rect.min.x + (ndc_x + 1.0) * 0.5 * width,
        rect.min.y + (1.0 - ndc_y) * 0.5 * height,
    ))
}

fn to_nm(p: [f64; 3]) -> Vec3 {
    Vec3::new(
        p[0] as f32 * NM_PER_ANGSTROM,
        p[1] as f32 * NM_PER_ANGSTROM,
        p[2] as f32 * NM_PER_ANGSTROM,
    )
}

/// This frame's wheel travel in notches (lines), from the raw events.
fn wheel_notches(ctx: &egui::Context) -> f32 {
    let points_per_line = ctx
        .options(|o| o.input_options.line_scroll_speed)
        .max(f32::EPSILON);
    ctx.input(|i| {
        i.raw
            .events
            .iter()
            .filter_map(|event| match event {
                egui::Event::MouseWheel { unit, delta, .. } => Some(match unit {
                    egui::MouseWheelUnit::Line => delta.y,
                    egui::MouseWheelUnit::Point => delta.y / points_per_line,
                    egui::MouseWheelUnit::Page => delta.y * 20.0,
                }),
                _ => None,
            })
            .sum()
    })
}

/// Atom colours for a dark background (the library's carbon is near black and vanishes).
pub fn element_color(element: &str) -> [f32; 4] {
    let (r, g, b) = match element {
        "H" => (0.92, 0.92, 0.92),
        "C" => (0.56, 0.56, 0.58),
        "N" => (0.25, 0.42, 0.98),
        "O" => (0.95, 0.20, 0.17),
        "F" | "Cl" => (0.35, 0.88, 0.35),
        "Br" => (0.70, 0.25, 0.18),
        "I" => (0.60, 0.25, 0.75),
        "S" => (0.95, 0.85, 0.20),
        "P" => (1.00, 0.55, 0.10),
        "B" => (1.00, 0.70, 0.70),
        "Si" => (0.85, 0.75, 0.60),
        _ => (0.85, 0.45, 0.85),
    };
    [r, g, b, 1.0]
}

/// Group `roles` (each a set of atoms and a colour, most important first) so every atom is in
/// one group only: overlapping groups would stack translucent spheres.
pub fn exclusive_groups(roles: &[Role<'_>]) -> Vec<AtomGroup> {
    let mut used = std::collections::HashSet::new();
    roles
        .iter()
        .filter_map(|(atoms, color)| {
            let fresh: Vec<usize> = atoms.iter().copied().filter(|a| used.insert(*a)).collect();
            (!fresh.is_empty()).then_some(AtomGroup {
                atom_indices: fresh,
                color: *color,
            })
        })
        .collect()
}

/// Atoms with one highlight role, and the role's colour.
pub type Role<'a> = (&'a [usize], (f32, f32, f32));

#[cfg(test)]
mod tests {
    use super::*;

    /// Projecting a point and casting the picking ray back through that pixel must land on
    /// the point: labels and clicks agree on where an atom is.
    #[test]
    fn projection_is_the_inverse_of_the_picking_ray() {
        let mut camera = OrbitalCamera {
            center: Vec3::new(0.1, -0.2, 0.05),
            radius: 3.0,
            ..Default::default()
        };
        camera.orbit(0.7, -0.4);
        let rect = Rect::from_min_size(Pos2::new(40.0, 20.0), egui::vec2(400.0, 300.0));
        camera.set_aspect(400.0 / 300.0);

        for pos in [[1.0, 2.0, -0.5], [-3.0, 0.5, 2.0], [0.0, 0.0, 0.0]] {
            let screen = project(&camera, rect, pos).expect("in front");
            let local = screen - rect.min;
            let (origin, dir) = camera.ray_from_screen(local.x, local.y, 400.0, 300.0);
            let target = to_nm(pos) - origin;
            // The ray passes through the point: the target is parallel to the direction.
            let along = target.dot(dir);
            let off = (target - dir * along).magnitude();
            assert!(off < 1e-4, "{pos:?}: {off}");
        }
        // The centre of rotation is in the middle of the view.
        let centre = project(
            &camera,
            rect,
            [1.0, -2.0, 0.5], // camera.center in Å
        )
        .unwrap();
        assert!((centre - rect.center()).length() < 1e-2, "{centre:?}");
    }

    /// The screen basis agrees with the projection: a point moved by a Shift+drag's translation
    /// lands under the pointer, and the toward-the-viewer direction comes nearer the camera.
    #[test]
    fn the_screen_basis_follows_the_pointer() {
        use crate::assembly::place::drag_translation;
        let mut view = Viewport3d::new();
        view.camera.center = Vec3::new(0.2, 0.1, -0.3);
        view.camera.radius = 4.0;
        view.camera.orbit(0.9, -0.5);
        let rect = Rect::from_min_size(Pos2::new(10.0, 30.0), egui::vec2(500.0, 400.0));
        view.camera.set_aspect(500.0 / 400.0);
        let at = [3.0, -1.0, 2.0];
        let basis = view.screen_basis(rect, at);
        let before = view.project(rect, at).unwrap();
        let moved = crate::assembly::math::add(at, drag_translation(&basis, [25.0, -15.0]));
        let after = view.project(rect, moved).unwrap();
        let shift = after - before;
        assert!(
            (shift.x - 25.0).abs() < 0.5 && (shift.y + 15.0).abs() < 0.5,
            "{shift:?}"
        );
        let camera = view.camera.position();
        let distance = |p: [f64; 3]| (to_nm(p) - camera).magnitude();
        let nearer = crate::assembly::math::add(at, basis.toward_viewer);
        assert!(distance(nearer) < distance(at));
        let c = view.view_center();
        assert!((c[0] - 2.0).abs() < 1e-5 && (c[2] + 3.0).abs() < 1e-5);
    }

    /// Spheres and ball-and-stick are different drawings, not one style under two names.
    #[test]
    fn spheres_and_ball_and_stick_draw_differently() {
        assert_eq!(Style3d::BallAndStick.render_style(), RenderStyle::BallStick);
        assert_eq!(Style3d::Spheres.render_style(), RenderStyle::Circles);
        assert_eq!(Style3d::Wireframe.render_style(), RenderStyle::Wireframe);
        for (i, a) in Style3d::ALL.iter().enumerate() {
            for b in &Style3d::ALL[i + 1..] {
                assert_ne!(a.render_style(), b.render_style(), "{a:?} {b:?}");
            }
        }
    }

    /// Spheres draws each atom at its van der Waals radius and ball-and-stick at the small
    /// one, whichever is chosen first and whether a model or a scene is shown.
    #[test]
    fn the_atom_radii_follow_the_style() {
        let atoms = [
            Atom3d {
                element: "C".into(),
                pos: [0.0, 0.0, 0.0],
            },
            Atom3d {
                element: "H".into(),
                pos: [1.1, 0.0, 0.0],
            },
        ];
        let radii = |view: &Viewport3d| view.viewer.atom_radii.clone().unwrap();
        let expected = |scale: f32| vec![vdw_radius("C") * scale, vdw_radius("H") * scale];

        let mut view = Viewport3d::new();
        view.sync_scene(
            Some(Scene {
                source: 1,
                revision: 1,
                atoms: &atoms,
                bonds: &[],
            }),
            0,
            || vec![[1.0; 4]; 2],
        );
        assert_eq!(radii(&view), expected(BALL_AND_STICK_ATOM_SCALE));
        view.set_style(Style3d::Spheres);
        assert_eq!(radii(&view), expected(1.0));
        view.set_style(Style3d::BallAndStick);
        assert_eq!(radii(&view), expected(BALL_AND_STICK_ATOM_SCALE));

        // Chosen before the model arrives.
        let mut view = Viewport3d::new();
        view.set_style(Style3d::Spheres);
        view.sync(Some(&Model3d {
            atoms: atoms.to_vec(),
            bonds: Vec::new(),
            force_field: crate::three_d::ForceField::Uff,
            energy: 0.0,
            energy_unit: String::new(),
            formula: String::new(),
            molfile: String::new(),
        }));
        assert_eq!(radii(&view), expected(1.0));
    }

    /// A click lands on the atom as drawn: in spheres the whole van der Waals sphere, in
    /// ball-and-stick only the small ball, and that still holds after the style changes once
    /// picking has run.
    #[test]
    fn clicks_hit_the_atoms_as_drawn() {
        let mut view = Viewport3d::new();
        view.sync(Some(&Model3d {
            atoms: vec![Atom3d {
                element: "C".into(),
                pos: [0.0, 0.0, 0.0],
            }],
            bonds: Vec::new(),
            force_field: crate::three_d::ForceField::Uff,
            energy: 0.0,
            energy_unit: String::new(),
            formula: String::new(),
            molfile: String::new(),
        }));
        let rect = Rect::from_min_size(Pos2::new(0.0, 0.0), egui::vec2(400.0, 300.0));
        view.camera.set_aspect(400.0 / 300.0);
        view.reset_view();
        // 1.0 Å beside the centre, across the line of sight: inside the carbon's 1.7 Å van der
        // Waals sphere, outside its ball-and-stick ball (0.43 Å).
        let basis = view.screen_basis(rect, [0.0; 3]);
        let beside = view.project(rect, basis.right).expect("in front");
        let centre = view.project(rect, [0.0; 3]).expect("in front");

        assert_eq!(view.pick(centre, rect), Hit::Atom(0));
        assert_eq!(view.pick(beside, rect), Hit::Nothing);
        view.set_style(Style3d::Spheres);
        assert_eq!(view.pick(beside, rect), Hit::Atom(0));
        view.set_style(Style3d::Wireframe);
        assert_eq!(view.pick(centre, rect), Hit::Atom(0));
        assert_eq!(view.pick(beside, rect), Hit::Nothing);
        view.set_style(Style3d::BallAndStick);
        assert_eq!(view.pick(beside, rect), Hit::Nothing);
    }

    /// A highlight shows outside the atom it marks: in spheres, around the largest van der
    /// Waals sphere; otherwise around the ball-and-stick atoms.
    #[test]
    fn highlights_are_wider_than_the_atoms() {
        let largest = ["H", "C", "N", "O", "F", "P", "S", "Cl", "Br", "I", "Xx"]
            .map(vdw_radius)
            .into_iter()
            .fold(0.0, f32::max);
        assert!(highlight_radius(RenderStyle::Circles) > largest);
        assert!(highlight_radius(RenderStyle::BallStick) > largest * BALL_AND_STICK_ATOM_SCALE);
    }

    #[test]
    fn each_atom_is_highlighted_once() {
        let groups = exclusive_groups(&[
            (&[1, 2], (1.0, 0.0, 0.0)),
            (&[2, 3], (0.0, 1.0, 0.0)),
            (&[1], (0.0, 0.0, 1.0)),
        ]);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].atom_indices, vec![1, 2]);
        assert_eq!(groups[1].atom_indices, vec![3]);
    }

    /// A scene reloads only when its revision changes, recolours on a new colour key, and is
    /// reframed only when another source is shown or the atom count changes.
    #[test]
    fn a_scene_reloads_on_a_new_revision_and_reframes_on_a_new_source() {
        let mut atoms = vec![
            Atom3d {
                element: "C".into(),
                pos: [0.0, 0.0, 0.0],
            },
            Atom3d {
                element: "N".into(),
                pos: [10.0, 0.0, 0.0],
            },
        ];
        fn scene(source: u64, revision: u64, atoms: &[Atom3d]) -> Scene<'_> {
            Scene {
                source,
                revision,
                atoms,
                bonds: &[],
            }
        }
        let mut view = Viewport3d::new();
        let mut calls = 0;
        view.sync_scene(Some(scene(1, 1, &atoms)), 0, || {
            calls += 1;
            vec![[1.0; 4]; 2]
        });
        assert!(std::mem::take(&mut view.reframe));
        view.camera.radius = 42.0;
        // Same revision: nothing is reloaded, even though the atoms moved.
        atoms[1].pos = [20.0, 0.0, 0.0];
        view.sync_scene(Some(scene(1, 1, &atoms)), 0, || unreachable!());
        let x = view.viewer.molecule().unwrap().atoms[1].position.x;
        assert!((x - 1.0).abs() < 1e-6);
        // A new colour key recolours only.
        view.sync_scene(Some(scene(1, 1, &atoms)), 5, || {
            calls += 1;
            vec![[0.5; 4]; 2]
        });
        assert_eq!(view.viewer.atom_colors.as_deref().unwrap()[0], [0.5; 4]);
        // A new revision reloads, keeping the camera.
        view.sync_scene(Some(scene(1, 2, &atoms)), 5, || {
            calls += 1;
            vec![[0.5; 4]; 2]
        });
        let x = view.viewer.molecule().unwrap().atoms[1].position.x;
        assert!((x - 2.0).abs() < 1e-6);
        assert!(!view.reframe && view.camera.radius == 42.0);
        assert_eq!(calls, 3);
        // Another source is framed, and so is another atom count.
        view.sync_scene(Some(scene(2, 2, &atoms)), 5, || vec![[0.5; 4]; 2]);
        assert!(std::mem::take(&mut view.reframe));
        view.sync_scene(Some(scene(2, 3, &atoms[..1])), 5, || vec![[0.5; 4]; 1]);
        assert!(view.reframe);
        // Large scenes get level of detail.
        let many: Vec<Atom3d> = (0..LOD_FROM_ATOMS)
            .map(|i| Atom3d {
                element: "C".into(),
                pos: [i as f64, 0.0, 0.0],
            })
            .collect();
        view.sync_scene(Some(scene(2, 4, &many)), 5, || {
            vec![[0.5; 4]; LOD_FROM_ATOMS]
        });
        assert!(view.offscreen.lod_settings().enabled);
        view.sync_scene(None, 0, Vec::new);
        assert!(view.viewer.molecule().is_none());
    }

    #[test]
    fn a_cell_is_outlined_and_repeated_only_when_asked() {
        let mut view = Viewport3d::new();
        let cell = [[10.0, 0.0, 0.0], [0.0, 12.0, 0.0], [1.0, 0.0, 8.0]];
        view.set_cell(Some(cell), Some((cell, [3, 1, 1])));
        let stored =
            moleucle_3dview_rs::with_state_by_type::<SimulationCellState, _>(&view.shared, |s| *s)
                .unwrap();
        assert!((stored.vectors[2].x - 0.1).abs() < 1e-6, "Å → nm");
        let images = view.viewer.periodic_images().unwrap();
        assert_eq!(images.len(), 3);
        // One copy per axis is no images; no outline is an empty cell.
        view.set_cell(None, Some((cell, [1, 1, 1])));
        assert!(view.viewer.periodic_images().is_none());
        let stored =
            moleucle_3dview_rs::with_state_by_type::<SimulationCellState, _>(&view.shared, |s| *s)
                .unwrap();
        assert!(stored.is_empty());
    }

    #[test]
    fn a_model_is_framed_once_and_then_only_moved() {
        use crate::three_d::{Atom3d, Bond3d, ForceField};
        let mut model = Model3d {
            atoms: vec![
                Atom3d {
                    element: "C".into(),
                    pos: [0.0, 0.0, 0.0],
                },
                Atom3d {
                    element: "O".into(),
                    pos: [1.4, 0.0, 0.0],
                },
            ],
            bonds: vec![Bond3d {
                begin: 0,
                end: 1,
                order: 1,
            }],
            force_field: ForceField::Uff,
            energy: 0.0,
            energy_unit: String::new(),
            formula: String::new(),
            molfile: String::new(),
        };
        let mut view = Viewport3d::new();
        view.sync(Some(&model));
        let framed = (view.camera.center, view.camera.radius);
        view.camera.radius *= 3.0;
        model.atoms[1].pos = [1.6, 0.0, 0.0];
        view.sync(Some(&model));
        assert_eq!(
            view.camera.radius,
            framed.1 * 3.0,
            "an edit keeps the camera"
        );
        let atoms = &view.viewer.molecule().unwrap().atoms;
        assert!((atoms[1].position.x - 0.16).abs() < 1e-6);
        view.sync(None);
        assert!(view.viewer.molecule().is_none());
    }
}
