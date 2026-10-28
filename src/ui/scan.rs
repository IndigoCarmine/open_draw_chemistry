//! The パラメータスキャン window: choose any number of parameters of the assembly (集合体 view)
//! or of the 3D model (3D view), run the scan in the background, and see the MM energy as a
//! curve (one parameter) or a heatmap (two). Of three or more, one or two are plotted and each
//! other one is sliced at a value or minimised over. Clicking a point puts its values into the
//! design or the model as one undoable step, so that structure can be looked at.

use std::time::Duration;

use egui::{Color32, RichText, vec2};
use egui_plot::{Heatmap, Line, MarkerShape, Plot, PlotPoint, PlotPoints, Points, Text};

use super::icons::{self, Icon};
use super::{Command, ViewMode};
use crate::document::Document;
use crate::scan::param::design_params;
use crate::scan::run::Relax;
use crate::scan::run::format_duration;
use crate::scan::source::{Point, Source};
use crate::scan::{self, Axis, MAX_POINTS, Param, Reduce, Run, Scan};
use crate::theme;
use crate::three_d::{ForceField, edit};

/// What a scan varies, following the view it is opened from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    Assembly,
    Model,
}

impl Target {
    fn of(view: ViewMode) -> Option<Target> {
        match view {
            ViewMode::Assembly => Some(Target::Assembly),
            ViewMode::ThreeD => Some(Target::Model),
            ViewMode::TwoD => None,
        }
    }

    fn matches(self, param: &Param) -> bool {
        param.is_assembly() == (self == Target::Assembly)
    }
}

/// One axis as being set up in the window.
#[derive(Debug, Clone, PartialEq)]
struct AxisForm {
    param: Option<Param>,
    start: f64,
    end: f64,
    points: usize,
}

impl Default for AxisForm {
    fn default() -> Self {
        Self {
            param: None,
            start: 0.0,
            end: 1.0,
            points: 11,
        }
    }
}

impl AxisForm {
    /// Choose `param`, its range set around its value now.
    fn set(&mut self, param: Param, current: Option<f64>) {
        let current = current.unwrap_or(0.0);
        let (start, end) = param.default_range(current);
        self.points = match param {
            Param::Dihedral(_) => 37,
            _ => 11,
        };
        self.start = start;
        self.end = end;
        self.param = Some(param);
    }

    fn axis(&self) -> Option<Axis> {
        Some(Axis {
            param: self.param.clone()?,
            start: self.start,
            end: self.end,
            points: self.points.clamp(1, MAX_POINTS),
        })
    }
}

/// The window's state (not saved: parameters name atoms and steps of open documents).
#[derive(Debug, Default)]
pub struct ScanWindow {
    pub open: bool,
    /// The axes, the first the outermost loop (one once the window has been shown).
    axes: Vec<AxisForm>,
    /// The force field chosen here; the document's own until changed.
    force_field: Option<ForceField>,
    /// Minimise everything but the scanned parameters at every point (the 3D model only).
    relax: bool,
    /// Show the energy relative to the lowest point (ΔE).
    relative: bool,
    colors: ColorRange,
    /// The last problem or confirmation, and whether it is a problem.
    message: Option<(String, bool)>,
    /// How a scan of three or more axes is plotted.
    view: ViewForm,
    /// The section last plotted of such a scan.
    sections: SectionCache,
}

/// Which axes of a scan of three or more are plotted, and how each other one is reduced.
#[derive(Debug, Default, Clone, PartialEq)]
struct ViewForm {
    /// The axis plotted across.
    across: usize,
    /// The axis plotted up (a heatmap), or none (a curve).
    up: Option<usize>,
    /// One per axis of the scan (those plotted ignored).
    reduce: Vec<Reduce>,
}

impl ViewForm {
    /// Fit to a scan whose axes take `shape` values each: for another number of axes, the
    /// first two plotted and the rest minimised over.
    fn fit(&mut self, shape: &[usize]) {
        let n = shape.len();
        if self.reduce.len() != n {
            *self = ViewForm {
                across: 0,
                up: (n > 1).then_some(1),
                reduce: vec![Reduce::Minimum; n],
            };
        }
        self.across = self.across.min(n.saturating_sub(1));
        if self.up.is_some_and(|u| u >= n || u == self.across) {
            self.up = None;
        }
        for (reduce, &len) in self.reduce.iter_mut().zip(shape) {
            if let Reduce::Slice(i) = reduce {
                *i = (*i).min(len.saturating_sub(1));
            }
        }
    }

    /// The axes plotted: across, then up.
    fn shown(&self) -> Vec<usize> {
        std::iter::once(self.across).chain(self.up).collect()
    }
}

/// What a plot shows: the axes plotted, the point each of their cells shows (see
/// [`Scan::section`]), and the axes minimised over.
#[derive(Debug)]
struct Section {
    axes: Vec<usize>,
    cells: Vec<usize>,
    projected: Vec<usize>,
}

impl Section {
    /// The whole of a scan of one or two axes: each cell its own point (the section of every
    /// axis).
    fn whole(scan: &Scan) -> Self {
        Section {
            axes: (0..scan.axes.len()).collect(),
            cells: (0..scan.total()).collect(),
            projected: Vec::new(),
        }
    }

    /// The scan seen as `view` says.
    fn of(scan: &Scan, view: &ViewForm) -> Self {
        let axes = view.shown();
        let projected = (0..scan.axes.len())
            .filter(|k| !axes.contains(k) && view.reduce.get(*k) == Some(&Reduce::Minimum))
            .collect();
        Section {
            cells: scan.section(&axes, &view.reduce),
            axes,
            projected,
        }
    }

    /// The cell showing point `index`, if one does.
    fn cell_of(&self, index: usize) -> Option<usize> {
        self.cells.iter().position(|&i| i == index)
    }
}

/// The section last plotted ([`Section::of`], a pass over every point) and what it was of: the
/// run, how many of its points were in, and the view. Worked out again only when one of those
/// changes, not every frame.
#[derive(Debug, Default)]
struct SectionCache {
    last: Option<((u64, usize, ViewForm), Section)>,
}

impl SectionCache {
    /// The section of `run`'s scan that `view` shows.
    fn get(&mut self, run: &Run, view: &ViewForm) -> &Section {
        let key = (run.id(), run.scan.done(), view.clone());
        let last = self.last.take().filter(|(k, _)| *k == key);
        let (_, section) = self
            .last
            .insert(last.unwrap_or_else(|| (key, Section::of(&run.scan, view))));
        section
    }
}

/// A scan's energies as one frame shows them: its lowest point, and the energy every point's
/// is shown relative to — each a pass over every point, so found once per frame and never per
/// cell.
struct Energies<'a> {
    scan: &'a Scan,
    relative: bool,
    minimum: Option<(usize, f64)>,
    baseline: f64,
}

impl<'a> Energies<'a> {
    /// Of `scan`, relative to its minimum (ΔE) when `relative`.
    fn of(scan: &'a Scan, relative: bool) -> Self {
        let minimum = scan.minimum();
        let baseline = if relative {
            minimum.map_or(0.0, |m| m.1)
        } else {
            0.0
        };
        Energies {
            scan,
            relative,
            minimum,
            baseline,
        }
    }

    /// The energy of point `index` as shown (see [`Scan::shown`]), in constant time.
    fn shown(&self, index: usize) -> Option<f64> {
        self.scan.shown_from(index, self.baseline)
    }
}

/// What a click on an axis row's buttons asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AxisAction {
    Up,
    Down,
    Remove,
}

/// Apply `action` to axis `k` of `axes` (the last one is never removed).
fn edit_axes(axes: &mut Vec<AxisForm>, k: usize, action: AxisAction) {
    match action {
        AxisAction::Up if k > 0 && k < axes.len() => axes.swap(k - 1, k),
        AxisAction::Down if k + 1 < axes.len() => axes.swap(k, k + 1),
        AxisAction::Remove if axes.len() > 1 && k < axes.len() => {
            axes.remove(k);
        }
        _ => {}
    }
}

/// What keeps a scan of `axes` over `atoms` atoms from starting, if anything: a parameter on
/// two axes, too many atoms, too many points, or (relaxing) too much work.
fn refusal(axes: &[Axis], atoms: usize, relax: bool) -> Option<String> {
    let total = scan::total_points(axes);
    let same = axes
        .iter()
        .enumerate()
        .any(|(k, a)| axes[..k].iter().any(|b| b.param == a.param));
    if same {
        Some(t!("scan.window.same_axes").into())
    } else if atoms > scan::MAX_ATOMS {
        Some(t!(
            "scan.window.too_many_atoms",
            atoms = atoms,
            max = scan::MAX_ATOMS
        ))
    } else if let Some(why) = scan::points_refusal(total) {
        Some(why)
    } else if relax {
        scan::relax_refusal(atoms, total)
    } else {
        None
    }
}

#[derive(Debug)]
struct ColorRange {
    auto: bool,
    lo: f64,
    hi: f64,
}

impl Default for ColorRange {
    fn default() -> Self {
        Self {
            auto: true,
            lo: 0.0,
            hi: 1.0,
        }
    }
}

impl ScanWindow {
    fn set_relative(&mut self, relative: bool) {
        if self.relative != relative {
            self.relative = relative;
            self.colors.auto = true;
        }
    }
}

/// Preserve the automatic data range; sanitize manual input before plotting.
fn color_range(auto_lo: f64, auto_hi: f64, manual: Option<(f64, f64)>) -> (f64, f64) {
    let automatic = || {
        if auto_lo.is_finite() {
            (
                auto_lo,
                if auto_hi > auto_lo {
                    auto_hi
                } else {
                    auto_lo + 1.0
                },
            )
        } else {
            (0.0, 1.0)
        }
    };
    let Some((lo, hi)) = manual else {
        return automatic();
    };
    if !lo.is_finite() || !hi.is_finite() {
        return automatic();
    }
    if hi > lo {
        (lo, hi)
    } else {
        // At large magnitudes adding one can round back to lo.
        let hi = (lo + 1.0).max(lo.next_up());
        if hi.is_finite() {
            (lo, hi)
        } else {
            (lo.next_down(), lo)
        }
    }
}

fn heatmap_color(value: f64, lo: f64, hi: f64) -> Color32 {
    if value == PENDING {
        Color32::TRANSPARENT
    } else if value.is_nan() {
        FAILED
    } else {
        viridis(((value - lo) / (hi - lo)).clamp(0.0, 1.0))
    }
}

/// Keep every document's scan collecting its points, also while the window is closed; keeps
/// frames coming while any runs.
pub fn poll(ctx: &egui::Context, docs: &mut [Document]) {
    let mut running = false;
    for doc in docs {
        if let Some(run) = &mut doc.scan {
            run.poll();
            running |= run.is_running();
        }
    }
    if running {
        ctx.request_repaint_after(Duration::from_millis(60));
    }
}

/// Show the window (when open). Returns a message for the status bar when a point was applied.
pub fn window(
    ctx: &egui::Context,
    win: &mut ScanWindow,
    doc: Option<&mut Document>,
    view: ViewMode,
    out: &mut Vec<Command>,
) -> Option<String> {
    if !win.open {
        return None;
    }
    let mut open = true;
    let mut status = None;
    egui::Window::new(t!("scan.window.title"))
        .open(&mut open)
        .default_size(vec2(620.0, 680.0))
        .min_width(420.0)
        .resizable(true)
        .collapsible(true)
        .show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                status = body(ui, win, doc, view, out);
            });
        });
    win.open = open;
    status
}

fn muted(ui: &egui::Ui, text: impl Into<String>) -> RichText {
    RichText::new(text.into())
        .size(11.0)
        .color(theme::muted(ui.visuals().dark_mode))
}

fn warn_color(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        theme::ORANGE_ON_DARK
    } else {
        theme::ORANGE_TEXT
    }
}

fn body(
    ui: &mut egui::Ui,
    win: &mut ScanWindow,
    doc: Option<&mut Document>,
    view: ViewMode,
    out: &mut Vec<Command>,
) -> Option<String> {
    ui.spacing_mut().item_spacing.y = 6.0;
    let Some(doc) = doc else {
        ui.label(muted(ui, t!("scan.window.no_document")));
        return None;
    };
    let target = Target::of(view);
    match target {
        None => {
            ui.label(muted(ui, t!("scan.window.no_view")));
        }
        Some(target) => setup(ui, win, doc, target),
    }

    ui.separator();
    results(ui, win, doc, out)
}

/// The parameters, the force field, and starting or cancelling the scan.
fn setup(ui: &mut egui::Ui, win: &mut ScanWindow, doc: &mut Document, target: Target) {
    if win.axes.is_empty() {
        win.axes.push(AxisForm::default());
    }
    // Forget parameters of the other kind (the window was last used from another view).
    for form in &mut win.axes {
        if form.param.as_ref().is_some_and(|p| !target.matches(p)) {
            form.param = None;
        }
    }
    let running = doc.scan.as_ref().is_some_and(Run::is_running);
    let force_field = *win.force_field.get_or_insert(doc.three_d.force_field);

    let heading = match target {
        Target::Assembly => t!("scan.window.target_assembly"),
        Target::Model => t!("scan.window.target_model"),
    };
    ui.label(RichText::new(heading).strong());

    // Every axis's points multiplied, also before their parameters are chosen.
    let total_all = win.axes.iter().fold(1usize, |t, f| {
        t.saturating_mul(f.points.clamp(1, MAX_POINTS))
    });
    let count = win.axes.len();
    let mut action = None;
    let atoms;
    let mut has_lattice = false;
    match target {
        Target::Assembly => {
            let state = &mut doc.assembly;
            let selected_manual = state.selected.and_then(|m| state.design.manual_index(m));
            let options = design_params(&state.design, selected_manual);
            atoms = state.design.atom_count();
            has_lattice = state
                .design
                .steps
                .iter()
                .any(|s| s.enabled() && matches!(s, crate::assembly::Step::Lattice(_)));
            if options.is_empty() {
                ui.label(muted(ui, t!("scan.window.no_params")));
            }
            for (k, form) in win.axes.iter_mut().enumerate() {
                // A parameter whose step was removed is dropped.
                if form
                    .param
                    .as_ref()
                    .is_some_and(|p| p.read_design(&state.design).is_none())
                {
                    form.param = None;
                }
                let row = axis_row(ui, (k, count), total_all, form, |ui, form| {
                    let selected = form.param.as_ref().map_or_else(
                        || t!("scan.window.choose").to_owned(),
                        |p| p.label(Some(&state.design), None),
                    );
                    egui::ComboBox::from_id_salt(("scan_param", k))
                        .selected_text(selected)
                        .width(240.0)
                        .show_ui(ui, |ui| {
                            for p in &options {
                                let label = p.label(Some(&state.design), None);
                                let chosen = form.param.as_ref() == Some(p);
                                if ui.selectable_label(chosen, label).clicked() && !chosen {
                                    form.set(p.clone(), p.read_design(&state.design));
                                }
                            }
                        });
                });
                action = action.or(row.map(|a| (k, a)));
            }
        }
        Target::Model => {
            let Some((model, edit3d)) = doc.three_d.model_and_edit() else {
                ui.label(muted(ui, t!("scan.window.no_model")));
                return;
            };
            atoms = model.atoms.len();
            let graph = edit::graph(model);
            let picked = Param::of_picked(&edit3d.picked, &graph);
            let coords = edit::coords(model);
            for (k, form) in win.axes.iter_mut().enumerate() {
                // Atoms that are no longer a chain of this model are dropped.
                if form.param.as_ref().is_some_and(|p| {
                    let atoms: Vec<usize> = match p {
                        Param::Bond(a) => a.to_vec(),
                        Param::Angle(a) => a.to_vec(),
                        Param::Dihedral(a) => a.to_vec(),
                        _ => Vec::new(),
                    };
                    Param::of_picked(&atoms, &graph).as_ref() != Some(p)
                }) {
                    form.param = None;
                }
                let row = axis_row(ui, (k, count), total_all, form, |ui, form| {
                    let label = form.param.as_ref().map_or_else(
                        || t!("scan.window.unset").to_owned(),
                        |p| p.label(None, Some(model)),
                    );
                    ui.label(label);
                    if ui
                        .add_enabled(
                            picked.is_some(),
                            egui::Button::new(t!("scan.window.use_picked")).small(),
                        )
                        .on_hover_text(t!("scan.window.use_picked_tip"))
                        .on_disabled_hover_text(t!("scan.window.use_picked_disabled_tip"))
                        .clicked()
                        && let Some(p) = &picked
                    {
                        form.set(p.clone(), p.read_coords(&coords));
                    }
                });
                action = action.or(row.map(|a| (k, a)));
            }
        }
    }
    if let Some((k, action)) = action {
        edit_axes(&mut win.axes, k, action);
    }

    ui.horizontal_wrapped(|ui| {
        if ui
            .button(t!("scan.window.add_axis"))
            .on_hover_text(t!("scan.window.add_axis_tip"))
            .clicked()
        {
            win.axes.push(AxisForm::default());
        }
        ui.add_space(8.0);
        ui.label(t!("scan.window.force_field"));
        egui::ComboBox::from_id_salt("scan_force_field")
            .selected_text(force_field.id())
            .show_ui(ui, |ui| {
                for ff in ForceField::ALL {
                    if ui.selectable_label(ff == force_field, ff.id()).clicked() {
                        win.force_field = Some(ff);
                    }
                }
            });
    });

    let relaxable = target == Target::Model;
    let mut relax = win.relax && relaxable;
    if ui
        .add_enabled(
            relaxable,
            egui::Checkbox::new(&mut relax, t!("scan.window.relax")),
        )
        .on_hover_text(t!("scan.window.relax_tip"))
        .on_disabled_hover_text(t!("scan.window.relax_assembly_tip"))
        .changed()
    {
        win.relax = relax;
    }

    let axes: Vec<Axis> = win.axes.iter().filter_map(AxisForm::axis).collect();
    let all_set = axes.len() == win.axes.len();
    let total = if all_set {
        scan::total_points(&axes)
    } else {
        0
    };
    let unit = scan::energy::unit(force_field);
    let summary = if relax {
        t!(
            "scan.window.summary_relaxed",
            total = total,
            atoms = atoms,
            unit = unit
        )
    } else {
        t!(
            "scan.window.summary",
            total = total,
            atoms = atoms,
            unit = unit
        )
    };
    ui.label(muted(ui, summary));
    if has_lattice {
        ui.label(muted(ui, t!("scan.window.crystal_note")));
    }
    // Of the axes set so far: too many atoms is said before any is.
    let refused = refusal(&axes, atoms, relax);
    if let Some(why) = &refused {
        ui.label(RichText::new(why).color(warn_color(ui)));
    } else if relax && all_set {
        ui.label(relax_estimate(ui, atoms, total));
    }

    ui.horizontal(|ui| {
        let ready = all_set && refused.is_none();
        if running {
            if ui.button(t!("common.cancel")).clicked()
                && let Some(run) = &mut doc.scan
            {
                run.cancel();
            }
        } else if ui
            .add_enabled(ready, egui::Button::new(t!("scan.window.start")))
            .on_disabled_hover_text(t!("scan.window.start_disabled_tip"))
            .clicked()
        {
            start(win, doc, target, &axes, force_field, relax);
        }
    });
}

/// Axis `k` of `count`'s row: the parameter (drawn by `param`), its range, and its number of
/// points next to the `total` of all axes; and buttons to move it up or down or remove it,
/// whose click is returned.
fn axis_row(
    ui: &mut egui::Ui,
    (k, count): (usize, usize),
    total: usize,
    form: &mut AxisForm,
    param: impl FnOnce(&mut egui::Ui, &mut AxisForm),
) -> Option<AxisAction> {
    let mut action = None;
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(t!("scan.window.axis", number = k + 1)).strong());
        param(ui, form);
        let buttons = [
            (
                Icon::ChevronUp,
                AxisAction::Up,
                k > 0,
                t!("scan.window.move_up_tip"),
            ),
            (
                Icon::ChevronDown,
                AxisAction::Down,
                k + 1 < count,
                t!("scan.window.move_down_tip"),
            ),
            (
                Icon::Close,
                AxisAction::Remove,
                count > 1,
                t!("scan.window.remove_axis_tip"),
            ),
        ];
        for (icon, act, enabled, tip) in buttons {
            let size = vec2(18.0, 18.0);
            let flat = icons::Frame::Flat;
            if icons::icon_button(ui, icon, size, flat, false, enabled, tip).clicked() && enabled {
                action = Some(act);
            }
        }
    });
    if let Some(p) = form.param.clone() {
        let unit = format!(" {}", p.unit());
        let speed = if p.unit() == "°" { 1.0 } else { 0.05 };
        ui.horizontal_wrapped(|ui| {
            ui.add_space(30.0);
            ui.label(t!("scan.window.range_start"));
            ui.add(
                egui::DragValue::new(&mut form.start)
                    .speed(speed)
                    .max_decimals(3)
                    .suffix(unit.clone()),
            );
            ui.label(t!("scan.window.range_end"));
            ui.add(
                egui::DragValue::new(&mut form.end)
                    .speed(speed)
                    .max_decimals(3)
                    .suffix(unit),
            );
            ui.label(t!("scan.window.points"));
            ui.add(egui::DragValue::new(&mut form.points).range(1..=MAX_POINTS))
                .on_hover_text(t!("scan.window.points_tip"));
            if count > 1 {
                let color = if total > scan::MAX_SCAN_POINTS {
                    warn_color(ui)
                } else {
                    theme::muted(ui.visuals().dark_mode)
                };
                let text = t!("scan.window.total_points", total = total);
                ui.label(RichText::new(text).color(color)).on_hover_text(t!(
                    "scan.window.total_points_tip",
                    max = scan::MAX_SCAN_POINTS
                ));
            }
        });
    }
    action
}

/// How long a relaxed scan of `atoms` atoms and `points` points should take, before it starts:
/// a warning, saying how long Cancel takes, from [`scan::LONG_RELAX`] on.
fn relax_estimate(ui: &egui::Ui, atoms: usize, points: usize) -> RichText {
    let [low, high] = scan::relax_estimate(atoms, points);
    let (low_text, high_text) = (format_duration(low), format_duration(high));
    if high >= scan::LONG_RELAX {
        let cancel = format_duration(scan::relax_cancel_latency(atoms));
        RichText::new(t!(
            "scan.window.relax_long",
            low = low_text,
            high = high_text,
            cancel = cancel
        ))
        .color(warn_color(ui))
    } else {
        muted(
            ui,
            t!(
                "scan.window.relax_estimate",
                low = low_text,
                high = high_text
            ),
        )
    }
}

fn start(
    win: &mut ScanWindow,
    doc: &mut Document,
    target: Target,
    axes: &[Axis],
    ff: ForceField,
    relax: bool,
) {
    let params: Vec<Param> = axes.iter().map(|a| a.param.clone()).collect();
    let mut restraints = None;
    let source = match target {
        Target::Assembly => Source::assembly(&doc.assembly.design, params),
        Target::Model => {
            let Some((model, edit3d)) = doc.three_d.model_and_edit() else {
                return;
            };
            restraints = Some(edit3d.active_constraints(model.atoms.len()));
            Source::model(model, edit3d.anchor, params)
        }
    };
    let started = match restraints {
        Some(restraints) if relax => Run::start_relaxed(source, axes, ff, Relax::new(restraints)),
        _ => Run::start(source, axes, ff),
    };
    match started {
        Ok(run) => {
            doc.scan = Some(run);
            win.message = None;
        }
        Err(e) => win.message = Some((e, true)),
    }
}

/// Progress, the plot, and the result's actions. Returns a status message when a point was
/// applied.
fn results(
    ui: &mut egui::Ui,
    win: &mut ScanWindow,
    doc: &mut Document,
    out: &mut Vec<Command>,
) -> Option<String> {
    if let Some((text, problem)) = &win.message {
        let color = if *problem {
            warn_color(ui)
        } else {
            theme::muted(ui.visuals().dark_mode)
        };
        ui.label(RichText::new(text).color(color));
    }
    let Some(run) = &doc.scan else {
        ui.label(muted(ui, t!("scan.window.no_results")));
        return None;
    };
    progress_row(ui, run);
    if let Some(e) = &run.error {
        ui.label(RichText::new(e).color(warn_color(ui)));
    }

    ui.horizontal_wrapped(|ui| {
        let mut relative = win.relative;
        ui.checkbox(&mut relative, t!("scan.window.relative"));
        win.set_relative(relative);
        if ui
            .add_enabled(
                run.scan.done() > 0,
                egui::Button::new(t!("scan.window.export_csv")),
            )
            .clicked()
        {
            win.message = Some(save_csv(&run.scan));
        }
    });
    let energies = Energies::of(&run.scan, win.relative);
    if let Some((index, e)) = energies.minimum {
        let values = format_values(&run.scan, index);
        ui.label(muted(
            ui,
            t!(
                "scan.window.minimum",
                values = values,
                energy = format!("{e:.3}"),
                unit = run.scan.unit
            ),
        ));
    }

    let clicked = plot(
        ui,
        run,
        &energies,
        &mut win.colors,
        &mut win.view,
        &mut win.sections,
    );
    ui.label(muted(ui, t!("scan.window.plot_help")));
    let index = clicked?;
    let message = apply_point(doc, index);
    let assembly = doc.scan.as_ref().is_some_and(|r| r.scan.assembly);
    out.push(Command::SetView(if assembly {
        ViewMode::Assembly
    } else {
        ViewMode::ThreeD
    }));
    Some(message)
}

fn progress_row(ui: &mut egui::Ui, run: &Run) {
    let p = run.progress();
    let fraction = if p.total == 0 {
        0.0
    } else {
        p.done as f32 / p.total as f32
    };
    let mut text = t!(
        "scan.progress.counts",
        done = p.done,
        total = p.total,
        elapsed = format_duration(p.elapsed)
    );
    if let Some(remaining) = p.remaining {
        let per_point =
            p.elapsed.saturating_sub(p.setup.unwrap_or_default()) / p.done.max(1) as u32;
        text.push_str(&t!(
            "scan.progress.remaining",
            remaining = format_duration(remaining),
            per_point = format_duration(per_point)
        ));
    }
    if run.was_cancelled() {
        text.push_str(t!("scan.progress.cancelled"));
    } else if !run.is_running() && p.done == p.total && p.total > 0 {
        text.push_str(t!("scan.progress.done"));
    }
    ui.add(
        egui::ProgressBar::new(fraction)
            .text(text)
            .animate(run.is_running()),
    );
    let failed = run
        .scan
        .energies
        .iter()
        .filter(|e| e.is_some_and(f64::is_nan))
        .count();
    if run.drifted() {
        ui.label(
            RichText::new(t!("scan.progress.drift"))
                .size(11.0)
                .color(warn_color(ui)),
        );
    }
    if failed > 0 {
        ui.label(muted(ui, tn!("scan.progress.failed_points", failed)));
    }
    let unconverged = (0..p.total).filter(|&i| run.scan.is_unconverged(i)).count();
    if unconverged > 0 {
        ui.label(
            RichText::new(tn!("scan.window.unconverged_points", unconverged))
                .size(11.0)
                .color(warn_color(ui)),
        );
    }
}

/// "rise = 3.50 Å, twist = 30.0°" for point `index`.
fn format_values(scan: &Scan, index: usize) -> String {
    scan.values_at(index)
        .iter()
        .zip(&scan.axes)
        .map(|(v, a)| format!("{} = {}", a.label, format_value(*v, a.unit())))
        .collect::<Vec<_>>()
        .join(t!("scan.window.values_separator"))
}

fn format_value(v: f64, unit: &str) -> String {
    if unit == "°" {
        format!("{v:.1}°")
    } else {
        format!("{v:.3} Å")
    }
}

fn save_csv(scan: &Scan) -> (String, bool) {
    let Some(path) = rfd::FileDialog::new()
        .add_filter("CSV", &["csv"])
        .set_file_name("scan.csv")
        .save_file()
    else {
        return (t!("scan.export.cancelled").into(), false);
    };
    // A BOM so that Excel reads the Japanese header as UTF-8.
    let text = format!("\u{feff}{}", scan.to_csv());
    match std::fs::write(&path, text) {
        Ok(()) => (t!("scan.export.done", path = path.display()), false),
        Err(e) => (t!("scan.export.failed", error = e), true),
    }
}

/// Put point `index`'s values into the design or the model, as one undo step.
fn apply_point(doc: &mut Document, index: usize) -> String {
    let Some(run) = &doc.scan else {
        return String::new();
    };
    let values = run.scan.values_at(index);
    let described = format_values(&run.scan, index);
    match &*run.source {
        Source::Assembly { params, .. } => {
            let params = params.clone();
            let result = doc.assembly.edit(|d| {
                params
                    .iter()
                    .zip(&values)
                    .try_for_each(|(p, &v)| p.write_design(d, v))
            });
            match result {
                Ok(()) => t!("scan.apply.done", values = described),
                Err(e) => e.to_string(),
            }
        }
        Source::Model { .. } => {
            // A relaxed point puts back its minimised structure.
            let coords = match run.structure(index) {
                Some(relaxed) => relaxed.to_vec(),
                None => match run.source.at(&values) {
                    Ok(Point::Coords(c)) => c,
                    Ok(Point::Rebuilt { .. }) => return String::new(),
                    Err(e) => return t!("scan.apply.failed", error = e),
                },
            };
            let Some((model, edit3d)) = doc.three_d.model_and_edit() else {
                return t!("scan.apply.no_model").into();
            };
            if coords.len() != model.atoms.len() {
                return t!("scan.apply.model_changed").into();
            }
            edit3d.apply(model, coords, t!("scan.apply.undo_label"));
            t!("scan.apply.done", values = described)
        }
    }
}

/// Viridis, sampled at nine points: perceptually uniform, readable on light and dark.
const VIRIDIS: [[u8; 3]; 9] = [
    [0x44, 0x01, 0x54],
    [0x47, 0x2d, 0x7b],
    [0x3b, 0x52, 0x8b],
    [0x2c, 0x72, 0x8e],
    [0x21, 0x91, 0x8c],
    [0x28, 0xae, 0x80],
    [0x5e, 0xc9, 0x62],
    [0xad, 0xdc, 0x30],
    [0xfd, 0xe7, 0x25],
];

/// The colormap at `t` in [0, 1] (clamped).
pub fn viridis(t: f64) -> Color32 {
    let t = if t.is_finite() {
        t.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let x = t * (VIRIDIS.len() - 1) as f64;
    let i = (x.floor() as usize).min(VIRIDIS.len() - 2);
    let f = x - i as f64;
    let mix = |a: u8, b: u8| (a as f64 + (b as f64 - a as f64) * f).round() as u8;
    let (a, b) = (VIRIDIS[i], VIRIDIS[i + 1]);
    Color32::from_rgb(mix(a[0], b[0]), mix(a[1], b[1]), mix(a[2], b[2]))
}

/// A failed point in the heatmap: grey, distinct from every colour of the map.
const FAILED: Color32 = Color32::from_rgb(0x80, 0x80, 0x80);
/// A point not computed yet: not drawn. (Heatmap values cannot be missing, so it is coded as
/// +∞.)
const PENDING: f64 = f64::INFINITY;

/// An axis's values as evenly spaced cells: the lowest value, the spacing, and whether the
/// values run downwards (the cells are then laid out reversed so the plot's axis increases).
fn cells(values: &[f64]) -> (f64, f64, bool) {
    let first = values.first().copied().unwrap_or(0.0);
    let last = values.last().copied().unwrap_or(0.0);
    let n = values.len();
    let step = if n > 1 {
        (last - first).abs() / (n - 1) as f64
    } else {
        0.0
    };
    let step = if step > 0.0 { step } else { 1.0 };
    (first.min(last), step, last < first)
}

/// A ring round every cell of `section` whose point's minimisation did not converge, at `at`
/// (the cell's place in the plot).
fn unconverged_rings(
    scan: &Scan,
    section: &Section,
    at: impl Fn(usize) -> Option<[f64; 2]>,
    color: Color32,
) -> Option<Points<'static>> {
    let rings: Vec<[f64; 2]> = (0..section.cells.len())
        .filter(|&c| scan.is_unconverged(section.cells[c]))
        .filter_map(at)
        .collect();
    (!rings.is_empty()).then(|| {
        Points::new("unconverged", PlotPoints::new(rings))
            .shape(MarkerShape::Circle)
            .color(color)
            .radius(6.0)
            .filled(false)
    })
}

/// The plot of `run`'s scan (its `energies`); of three or more axes, as `view` says (chosen
/// above it). Returns the point clicked, if any.
fn plot(
    ui: &mut egui::Ui,
    run: &Run,
    energies: &Energies,
    colors: &mut ColorRange,
    view: &mut ViewForm,
    sections: &mut SectionCache,
) -> Option<usize> {
    let (scan, relative) = (&run.scan, energies.relative);
    if scan.axes.is_empty() || scan.total() == 0 {
        return None;
    }
    let y_title = format!(
        "{}E [{}] ({})",
        if relative { "Δ" } else { "" },
        scan.force_field,
        scan.unit
    );
    let whole;
    let section = if scan.axes.len() <= 2 {
        whole = Section::whole(scan);
        &whole
    } else {
        view.fit(&scan.shape());
        view_controls(ui, scan, energies.minimum, view);
        sections.get(run, view)
    };
    if section.axes.len() == 2 {
        heatmap(ui, energies, section, &y_title, colors)
    } else {
        curve(ui, energies, section, &y_title)
    }
}

/// The choice of the axes plotted (across, and up or none for a curve) and, for each other
/// one, a slice at one of its values or the lowest energy along it (`minimum`: the scan's).
fn view_controls(
    ui: &mut egui::Ui,
    scan: &Scan,
    minimum: Option<(usize, f64)>,
    view: &mut ViewForm,
) {
    let label = |k: usize| scan.axes[k].label.clone();
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(t!("scan.window.view_heading")).strong());
        ui.label(t!("scan.window.view_across"));
        egui::ComboBox::from_id_salt("scan_view_across")
            .selected_text(label(view.across))
            .show_ui(ui, |ui| {
                for k in 0..scan.axes.len() {
                    if ui.selectable_label(view.across == k, label(k)).clicked() {
                        // Taking the axis plotted up swaps the two.
                        if view.up == Some(k) {
                            view.up = Some(view.across);
                        }
                        view.across = k;
                    }
                }
            });
        ui.label(t!("scan.window.view_up"));
        let up_text = view
            .up
            .map_or_else(|| t!("scan.window.view_curve").to_owned(), label);
        egui::ComboBox::from_id_salt("scan_view_up")
            .selected_text(up_text)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(view.up.is_none(), t!("scan.window.view_curve"))
                    .clicked()
                {
                    view.up = None;
                }
                for k in (0..scan.axes.len()).filter(|&k| k != view.across) {
                    if ui.selectable_label(view.up == Some(k), label(k)).clicked() {
                        view.up = Some(k);
                    }
                }
            });
    });
    let shown = view.shown();
    let at_minimum = minimum.map(|(i, _)| scan.indices_at(i));
    for (k, axis) in scan.axes.iter().enumerate() {
        if shown.contains(&k) {
            continue;
        }
        ui.horizontal_wrapped(|ui| {
            ui.add_space(12.0);
            ui.label(&axis.label);
            let sliced = matches!(view.reduce[k], Reduce::Slice(_));
            if ui
                .radio(sliced, t!("scan.window.reduce_slice"))
                .on_hover_text(t!("scan.window.reduce_slice_tip"))
                .clicked()
                && !sliced
            {
                let at = at_minimum.as_ref().map_or(0, |m| m[k]);
                view.reduce[k] = Reduce::Slice(at);
            }
            if ui
                .radio(!sliced, t!("scan.window.reduce_minimum"))
                .on_hover_text(t!("scan.window.reduce_minimum_tip"))
                .clicked()
            {
                view.reduce[k] = Reduce::Minimum;
            }
            if let Reduce::Slice(at) = &mut view.reduce[k] {
                let last = axis.values.len().saturating_sub(1);
                ui.add_enabled(last > 0, egui::Slider::new(at, 0..=last).show_value(false));
                ui.label(format_value(axis.values[*at], axis.unit()));
            }
        });
    }
    let sliced = (0..scan.axes.len())
        .any(|k| !shown.contains(&k) && matches!(view.reduce[k], Reduce::Slice(_)));
    if let Some(at) = at_minimum.filter(|_| sliced)
        && ui
            .small_button(t!("scan.window.slice_at_minimum"))
            .on_hover_text(t!("scan.window.slice_at_minimum_tip"))
            .clicked()
    {
        for (reduce, i) in view.reduce.iter_mut().zip(at) {
            if let Reduce::Slice(s) = reduce {
                *s = i;
            }
        }
    }
}

fn accent(ui: &egui::Ui) -> Color32 {
    theme::accent(ui.visuals().dark_mode)
}

/// A one-parameter scan (or a section of one axis): the energy against the parameter, a line
/// through the points with a gap at each failed one, the lowest point marked.
fn curve(
    ui: &mut egui::Ui,
    energies: &Energies,
    section: &Section,
    y_title: &str,
) -> Option<usize> {
    let scan = energies.scan;
    let axis = &scan.axes[section.axes[0]];
    let accent = accent(ui);
    let min_color = if ui.visuals().dark_mode {
        Color32::from_rgb(0x5e, 0xc9, 0x62)
    } else {
        Color32::from_rgb(0x1f, 0x7a, 0x3a)
    };
    // By cell: the point it shows is `section.cells[c]`.
    let points: Vec<(usize, [f64; 2])> = (0..section.cells.len())
        .filter_map(|c| {
            let e = energies.shown(section.cells[c])?;
            e.is_finite().then(|| (c, [axis.values[c], e]))
        })
        .collect();
    // Runs of consecutive computed points: a failed or missing point breaks the line.
    let mut segments: Vec<Vec<[f64; 2]>> = Vec::new();
    let mut last: Option<usize> = None;
    for &(i, p) in &points {
        if last != Some(i.wrapping_sub(1)) || segments.is_empty() {
            segments.push(Vec::new());
        }
        if let Some(s) = segments.last_mut() {
            s.push(p);
        }
        last = Some(i);
    }
    let min_point = energies.minimum.and_then(|(i, _)| {
        let c = section.cell_of(i)?;
        let e = energies.shown(i)?;
        Some([axis.values[c], e])
    });
    let rings = unconverged_rings(
        scan,
        section,
        |c| {
            let e = energies.shown(section.cells[c])?;
            e.is_finite().then(|| [axis.values[c], e])
        },
        warn_color(ui),
    );
    let (lo, hi) = (
        axis.values.iter().cloned().fold(f64::INFINITY, f64::min),
        axis.values
            .iter()
            .cloned()
            .fold(f64::NEG_INFINITY, f64::max),
    );

    let response = Plot::new(("scan_curve", &section.axes))
        // Room round the data, so the end points and the minimum marker are not cut off.
        .set_margin_fraction(vec2(0.04, 0.1))
        .height(320.0)
        .x_axis_label(axis.title())
        .y_axis_label(y_title.to_owned())
        .label_formatter(|_| None)
        .show_crosshair(false)
        .include_x(lo)
        .include_x(hi)
        .show(ui, |plot_ui| {
            for (k, segment) in segments.into_iter().enumerate() {
                plot_ui.line(
                    Line::new(format!("E{k}"), PlotPoints::new(segment))
                        .color(accent)
                        .width(1.8),
                );
            }
            let dots: Vec<[f64; 2]> = points.iter().map(|&(_, p)| p).collect();
            plot_ui.points(
                Points::new("points", PlotPoints::new(dots))
                    .color(accent)
                    .radius(3.0)
                    .filled(true),
            );
            if let Some(rings) = rings {
                plot_ui.points(rings);
            }
            if let Some(p) = min_point {
                plot_ui.points(
                    Points::new("minimum", PlotPoints::new(vec![p]))
                        .shape(MarkerShape::Diamond)
                        .color(min_color)
                        .radius(7.0)
                        .filled(true),
                );
                plot_ui.text(
                    Text::new(
                        "minimum_label",
                        PlotPoint::new(p[0], p[1]),
                        RichText::new(format!("  {}", t!("scan.plot.minimum")))
                            .color(min_color)
                            .strong(),
                    )
                    .anchor(egui::Align2::LEFT_BOTTOM),
                );
            }
        });

    // The point nearest the pointer along x, within reach.
    let hovered = response.response.hover_pos().and_then(|pos| {
        points
            .iter()
            .map(|&(c, p)| {
                let screen = response
                    .transform
                    .position_from_point(&PlotPoint::new(p[0], p[1]));
                (c, (screen.x - pos.x).abs())
            })
            .filter(|&(_, dx)| dx < 14.0)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(c, _)| section.cells[c])
    });
    finish_interaction(&response.response, energies, section, hovered)
}

/// A two-parameter scan (or a section of two axes): a heatmap (the first parameter across, the
/// second up), a colour bar, and the lowest cell marked.
fn heatmap(
    ui: &mut egui::Ui,
    energies: &Energies,
    section: &Section,
    z_title: &str,
    colors: &mut ColorRange,
) -> Option<usize> {
    let scan = energies.scan;
    let (ax, ay) = (&scan.axes[section.axes[0]], &scan.axes[section.axes[1]]);
    let (nx, ny) = (ax.values.len(), ay.values.len());
    let (x0, dx, x_rev) = cells(&ax.values);
    let (y0, dy, y_rev) = cells(&ay.values);
    // Grid cell (column c, row r) of the plot ↔ cell (i, j) of the section ↔ the point it shows.
    let to_point = |c: usize, r: usize| {
        let i = if x_rev { nx - 1 - c } else { c };
        let j = if y_rev { ny - 1 - r } else { r };
        section.cells[i * ny + j]
    };
    // The place in the plot of the section's cell `k`.
    let place = |k: usize| [ax.values[k / ny], ay.values[k % ny]];
    let mut values = Vec::with_capacity(nx * ny);
    for r in 0..ny {
        for c in 0..nx {
            values.push(energies.shown(to_point(c, r)).unwrap_or(PENDING));
        }
    }
    let finite = || values.iter().copied().filter(|v| v.is_finite());
    let lo = finite().fold(f64::INFINITY, f64::min);
    let hi = finite().fold(f64::NEG_INFINITY, f64::max);
    // Show the actual extrema in Auto, even for a constant-valued scan whose
    // plotting range needs the one-unit padding.
    let auto_range = if lo.is_finite() { (lo, hi) } else { (0.0, 1.0) };
    if colors.auto {
        (colors.lo, colors.hi) = auto_range;
    }
    // Above the map and its colour bar so edits affect both in this frame.
    ui.horizontal_wrapped(|ui| {
        ui.checkbox(&mut colors.auto, t!("scan.window.color_auto"));
        if colors.auto {
            (colors.lo, colors.hi) = auto_range;
        }
        ui.label(t!("scan.window.color_min"));
        let min_changed = ui
            .add(egui::DragValue::new(&mut colors.lo).speed(0.1))
            .changed();
        ui.label(t!("scan.window.color_max"));
        let max_changed = ui
            .add(egui::DragValue::new(&mut colors.hi).speed(0.1))
            .changed();
        if min_changed || max_changed {
            colors.auto = false;
        }
    });
    let (lo, hi) = color_range(lo, hi, (!colors.auto).then_some((colors.lo, colors.hi)));
    if !colors.auto {
        (colors.lo, colors.hi) = (lo, hi);
    }
    let min_cell = energies
        .minimum
        .and_then(|(index, _)| section.cell_of(index))
        .map(place);
    let marker = if ui.visuals().dark_mode {
        Color32::WHITE
    } else {
        Color32::from_rgb(0xF4, 0xF1, 0xEE)
    };

    let mut clicked = None;
    ui.horizontal(|ui| {
        let bar_width = 74.0;
        let width = (ui.available_width() - bar_width - 8.0).max(200.0);
        let map = Heatmap::new(values.clone(), nx)
            .at(PlotPoint::new(x0 - dx / 2.0, y0 - dy / 2.0))
            .tile_size(dx as f32, dy as f32)
            .show_labels(false)
            .range(lo, hi)
            .custom_mapping(Box::new(move |v| heatmap_color(v, lo, hi)));
        let response = Plot::new(("scan_heatmap", &section.axes))
            .set_margin_fraction(vec2(0.02, 0.02))
            .width(width)
            .height(340.0)
            .x_axis_label(ax.title())
            .y_axis_label(ay.title())
            .label_formatter(|_| None)
            .show_crosshair(false)
            .show_grid(false)
            .show(ui, |plot_ui| {
                plot_ui.add(map);
                let at = |k| Some(place(k));
                if let Some(rings) = unconverged_rings(scan, section, at, marker) {
                    plot_ui.points(rings);
                }
                if let Some(p) = min_cell {
                    plot_ui.points(
                        Points::new("minimum", PlotPoints::new(vec![p]))
                            .shape(MarkerShape::Diamond)
                            .color(marker)
                            .radius(6.0)
                            .filled(false),
                    );
                    plot_ui.text(
                        Text::new(
                            "minimum_label",
                            PlotPoint::new(p[0], p[1]),
                            RichText::new(format!(" {}", t!("scan.plot.minimum")))
                                .color(marker)
                                .strong(),
                        )
                        .anchor(egui::Align2::LEFT_BOTTOM),
                    );
                }
            });
        let hovered = response.response.hover_pos().and_then(|pos| {
            let at = response.transform.value_from_position(pos);
            let c = ((at.x - (x0 - dx / 2.0)) / dx).floor();
            let r = ((at.y - (y0 - dy / 2.0)) / dy).floor();
            ((0.0..nx as f64).contains(&c) && (0.0..ny as f64).contains(&r))
                .then(|| to_point(c as usize, r as usize))
        });
        clicked = finish_interaction(&response.response, energies, section, hovered);
        colour_bar(ui, lo, hi, z_title, bar_width);
    });
    clicked
}

/// The tooltip of the hovered point (all its values: of a projection, the point found lowest),
/// and the point clicked.
fn finish_interaction(
    response: &egui::Response,
    energies: &Energies,
    section: &Section,
    hovered: Option<usize>,
) -> Option<usize> {
    let (scan, relative) = (energies.scan, energies.relative);
    let index = hovered?;
    let energy = match energies.shown(index) {
        None => t!("scan.plot.not_computed").to_owned(),
        Some(e) if e.is_nan() => t!("scan.plot.failed").to_owned(),
        Some(e) => format!(
            "{}E = {e:.3} {}",
            if relative { "Δ" } else { "" },
            scan.unit
        ),
    };
    let projected = (!section.projected.is_empty()).then(|| {
        let axes: Vec<&str> = section
            .projected
            .iter()
            .map(|&k| scan.axes[k].label.as_str())
            .collect();
        t!(
            "scan.window.projected",
            axes = axes.join(t!("scan.window.values_separator"))
        )
    });
    let lines: Vec<String> = scan
        .values_at(index)
        .iter()
        .zip(&scan.axes)
        .map(|(v, a)| format!("{} = {}", a.label, format_value(*v, a.unit())))
        .chain(projected)
        .chain(
            scan.is_unconverged(index)
                .then(|| t!("scan.window.unconverged").to_owned()),
        )
        .chain([energy, t!("scan.plot.click_to_apply").to_owned()])
        .collect();
    response.clone().on_hover_ui_at_pointer(|ui| {
        for line in &lines {
            ui.label(line);
        }
    });
    (response.clicked() && scan.energies[index].is_some()).then_some(index)
}

/// A vertical colour bar from `lo` (bottom) to `hi` (top) with its ends and middle labelled.
fn colour_bar(ui: &mut egui::Ui, lo: f64, hi: f64, title: &str, width: f32) {
    ui.vertical(|ui| {
        ui.set_width(width);
        ui.label(RichText::new(title).size(10.0));
        let (rect, _) = ui.allocate_exact_size(vec2(width, 280.0), egui::Sense::hover());
        let bar =
            egui::Rect::from_min_size(rect.min + vec2(2.0, 6.0), vec2(16.0, rect.height() - 12.0));
        let painter = ui.painter_at(rect);
        let steps = 64;
        for k in 0..steps {
            let t0 = k as f32 / steps as f32;
            let t1 = (k + 1) as f32 / steps as f32;
            let y0 = bar.bottom() - bar.height() * t1;
            let y1 = bar.bottom() - bar.height() * t0;
            painter.rect_filled(
                egui::Rect::from_x_y_ranges(bar.x_range(), y0..=y1),
                0.0,
                viridis(f64::from((t0 + t1) / 2.0)),
            );
        }
        let stroke = ui.visuals().widgets.noninteractive.bg_stroke;
        painter.rect_stroke(bar, 0.0, stroke, egui::StrokeKind::Outside);
        let text = ui.visuals().text_color();
        for (t, v) in [(0.0, lo), (0.5, (lo + hi) / 2.0), (1.0, hi)] {
            let y = bar.bottom() - bar.height() * t;
            painter.line_segment(
                [egui::pos2(bar.right(), y), egui::pos2(bar.right() + 4.0, y)],
                egui::Stroke::new(1.0, text),
            );
            painter.text(
                egui::pos2(bar.right() + 6.0, y),
                egui::Align2::LEFT_CENTER,
                format_energy(v),
                egui::FontId::proportional(10.0),
                text,
            );
        }
    });
}

fn format_energy(v: f64) -> String {
    if v.abs() >= 1000.0 {
        format!("{v:.0}")
    } else if v.abs() >= 10.0 {
        format!("{v:.1}")
    } else {
        format!("{v:.2}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::ScanAxis;
    use crate::three_d::{self, Atom3d, Bond3d, Model3d};

    #[test]
    fn color_ranges_preserve_auto_and_validate_manual() {
        assert_eq!(color_range(-3.0, 7.0, None), (-3.0, 7.0));
        assert_eq!(color_range(4.0, 4.0, None), (4.0, 5.0));
        assert_eq!(color_range(4.0, 2.0, None), (4.0, 5.0));
        assert_eq!(
            color_range(f64::INFINITY, f64::NEG_INFINITY, None),
            (0.0, 1.0)
        );
        for bounds in [
            (2.0, 8.0),
            (2.0, 2.0),
            (2.0, -5.0),
            (f64::NAN, 1.0),
            (f64::MAX, f64::MAX),
        ] {
            let (lo, hi) = color_range(-3.0, 7.0, Some(bounds));
            assert!(lo < hi, "{lo} {hi}");
        }
        assert_eq!(color_range(-3.0, 7.0, Some((2.0, 8.0))), (2.0, 8.0));
    }

    #[test]
    fn manual_colors_clamp_and_preserve_missing_points() {
        for v in [-10.0, 2.0] {
            assert_eq!(heatmap_color(v, 2.0, 8.0), viridis(0.0));
        }
        for v in [8.0, 20.0] {
            assert_eq!(heatmap_color(v, 2.0, 8.0), viridis(1.0));
        }
        assert_eq!(heatmap_color(PENDING, 2.0, 8.0), Color32::TRANSPARENT);
        assert_eq!(heatmap_color(f64::NAN, 2.0, 8.0), FAILED);
    }

    #[test]
    fn changing_energy_baseline_resets_color_auto() {
        let mut win = ScanWindow::default();
        assert!(win.colors.auto);
        win.colors.auto = false;
        win.set_relative(false);
        assert!(!win.colors.auto);
        win.set_relative(true);
        assert!(win.colors.auto);
        win.colors.auto = false;
        win.set_relative(false);
        assert!(win.colors.auto);
    }

    fn chain_model() -> Model3d {
        let pos = [
            [0.0, 0.0, 0.0],
            [1.5, 0.0, 0.0],
            [2.0, 1.4, 0.0],
            [3.5, 1.4, 0.0],
        ];
        Model3d {
            atoms: pos
                .iter()
                .map(|&pos| Atom3d {
                    element: "C".into(),
                    pos,
                })
                .collect(),
            bonds: (0..3)
                .map(|i| Bond3d {
                    begin: i,
                    end: i + 1,
                    order: 1,
                })
                .collect(),
            force_field: ForceField::Uff,
            energy: 0.0,
            energy_unit: "kJ/mol".into(),
            formula: "C4".into(),
            molfile: String::new(),
        }
    }

    fn scan_of(axes: Vec<(Param, Vec<f64>)>, energy: impl Fn(&[f64]) -> Option<f64>) -> Scan {
        let mut scan = Scan::new(
            axes.into_iter()
                .map(|(param, values)| ScanAxis {
                    label: param.label(None, None),
                    param,
                    values,
                })
                .collect(),
            ForceField::Uff,
            "kJ/mol".into(),
        );
        for i in 0..scan.total() {
            scan.energies[i] = energy(&scan.values_at(i));
        }
        scan
    }

    /// Run one frame of the window over `doc` in `view`, returning what it drew's text.
    fn frame(win: &mut ScanWindow, doc: &mut Document, view: ViewMode, dark: bool) -> String {
        let ctx = egui::Context::default();
        ctx.set_visuals(if dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        });
        let mut out = Vec::new();
        let mut texts = String::new();
        // Two passes: the window sizes itself on the first.
        for _ in 0..2 {
            let mut output = ctx.run_ui(Default::default(), |ui| {
                window(ui.ctx(), win, Some(doc), view, &mut out);
            });
            output.textures_delta.clear();
            texts.clear();
            for clipped in &output.shapes {
                collect_text(&clipped.shape, &mut texts);
            }
        }
        texts
    }

    fn collect_text(shape: &egui::Shape, out: &mut String) {
        match shape {
            egui::Shape::Text(t) => {
                out.push_str(t.galley.text());
                out.push('\n');
            }
            egui::Shape::Vec(v) => v.iter().for_each(|s| collect_text(s, out)),
            _ => {}
        }
    }

    #[test]
    fn a_1d_and_a_2d_scan_render_headless_in_both_themes() {
        let mut doc = Document::new(&chembuider_rs::Config::default(), 1);
        let model = chain_model();
        doc.three_d = three_d::Job::with_model(model.clone(), &doc.editor.molecule);
        let dihedral = Param::Dihedral([0, 1, 2, 3]);
        let values: Vec<f64> = (0..13).map(|k| -180.0 + 30.0 * k as f64).collect();
        let scan = scan_of(vec![(dihedral.clone(), values)], |v| {
            // A gap at 0°.
            (v[0] != 0.0)
                .then(|| 1.0 + v[0].to_radians().cos())
                .or(Some(f64::NAN))
        });
        doc.scan = Some(Run::finished(
            scan,
            Source::model(&model, None, vec![dihedral.clone()]),
        ));
        let mut win = ScanWindow {
            open: true,
            ..Default::default()
        };
        for dark in [false, true] {
            let text = frame(&mut win, &mut doc, ViewMode::ThreeD, dark);
            assert!(text.contains("Parameter scan"), "{text}");
            assert!(text.contains("13 / 13 points"), "{text}");
            assert!(text.contains("1 point could not be computed"), "{text}");
            assert!(text.contains("Minimum"), "{text}");
        }

        let rise = Param::Step {
            step: 0,
            field: scan::StepField::StackRise,
        };
        let twist = Param::Step {
            step: 0,
            field: scan::StepField::StackTwist,
        };
        let scan = scan_of(
            vec![
                (rise.clone(), vec![3.0, 3.5, 4.0, 4.5]),
                (twist.clone(), vec![60.0, 30.0, 0.0]),
            ],
            |v| (v[0] < 4.5).then(|| (v[0] - 3.5).powi(2) + v[1] / 100.0),
        );
        assert_eq!(scan.total(), 12);
        doc.scan = Some(Run::finished(
            scan,
            Source::assembly(&doc.assembly.design, vec![rise, twist]),
        ));
        for dark in [false, true] {
            let text = frame(&mut win, &mut doc, ViewMode::Assembly, dark);
            assert!(text.contains("9 / 12 points"), "{text}");
            assert!(text.contains("Minimum"), "{text}");
        }
    }

    #[test]
    fn without_a_view_the_window_says_so() {
        let mut doc = Document::new(&chembuider_rs::Config::default(), 1);
        let mut win = ScanWindow {
            open: true,
            ..Default::default()
        };
        let text = frame(&mut win, &mut doc, ViewMode::TwoD, false);
        assert!(text.contains("3D view"), "{text}");
        let text = frame(&mut win, &mut doc, ViewMode::ThreeD, false);
        assert!(text.contains("No 3D model"), "{text}");
    }

    /// Applying a point of an assembly scan is one undo step; of a model scan it sets the
    /// dihedral, also undoable.
    #[test]
    fn a_clicked_point_is_applied_as_one_undo_step() {
        use crate::assembly::model::{Component, Design, Step};
        let mut doc = Document::new(&chembuider_rs::Config::default(), 1);
        let model = chain_model();
        doc.assembly.replace(Design {
            components: vec![Component::from_model("c4", &model)],
            steps: vec![Step::new_stack()],
            ..Default::default()
        });
        let rise = Param::Step {
            step: 0,
            field: scan::StepField::StackRise,
        };
        let twist = Param::Step {
            step: 0,
            field: scan::StepField::StackTwist,
        };
        let scan = scan_of(
            vec![
                (rise.clone(), vec![3.0, 4.0]),
                (twist.clone(), vec![0.0, 30.0]),
            ],
            |_| Some(1.0),
        );
        doc.scan = Some(Run::finished(
            scan,
            Source::assembly(&doc.assembly.design, vec![rise.clone(), twist.clone()]),
        ));
        let before = doc.assembly.design.clone();
        let message = apply_point(&mut doc, 3);
        assert!(message.contains("Applied"), "{message}");
        assert_eq!(rise.read_design(&doc.assembly.design), Some(4.0));
        assert_eq!(twist.read_design(&doc.assembly.design), Some(30.0));
        assert!(doc.assembly.undo());
        assert_eq!(doc.assembly.design, before, "one step");

        doc.three_d = three_d::Job::with_model(model.clone(), &doc.editor.molecule);
        let dihedral = Param::Dihedral([0, 1, 2, 3]);
        let scan = scan_of(vec![(dihedral.clone(), vec![-60.0, 60.0])], |_| Some(1.0));
        doc.scan = Some(Run::finished(
            scan,
            Source::model(&model, None, vec![dihedral.clone()]),
        ));
        apply_point(&mut doc, 1);
        let (m, edit3d) = doc.three_d.model_and_edit().unwrap();
        let now = dihedral.read_coords(&edit::coords(m)).unwrap();
        assert!((now - 60.0).abs() < 1e-6, "{now}");
        assert_eq!(edit3d.undo(m).as_deref(), Some("Apply scan point"));
        let back = dihedral.read_coords(&edit::coords(m)).unwrap();
        assert!(
            (back - 180.0).abs() < 1e-6 || (back + 180.0).abs() < 1e-6,
            "{back}"
        );
    }

    /// A relaxed scan says which points did not converge; relaxing is offered for the 3D model
    /// only; and clicking a relaxed point puts back its minimised structure, not the model
    /// with the dihedral merely set.
    #[test]
    fn a_relaxed_scan_marks_unconverged_points_and_applies_its_minimised_structures() {
        let mut doc = Document::new(&chembuider_rs::Config::default(), 1);
        let model = chain_model();
        doc.three_d = three_d::Job::with_model(model.clone(), &doc.editor.molecule);
        let dihedral = Param::Dihedral([0, 1, 2, 3]);
        let mut scan = scan_of(vec![(dihedral.clone(), vec![-60.0, 60.0, 180.0])], |v| {
            Some(v[0].to_radians().cos())
        });
        scan.unconverged[1] = true;
        // "Minimised" at 60°: the dihedral set, and the whole chain moved along x.
        let set = dihedral
            .write_coords(&edit::coords(&model), &edit::graph(&model), 60.0, &[])
            .unwrap();
        let relaxed: Vec<_> = set.iter().map(|p| [p[0] + 0.5, p[1], p[2]]).collect();
        doc.scan = Some(Run::finished_relaxed(
            scan,
            Source::model(&model, None, vec![dihedral.clone()]),
            vec![None, Some(relaxed.clone()), None],
        ));
        let mut win = ScanWindow {
            open: true,
            relax: true,
            ..Default::default()
        };
        for dark in [false, true] {
            let text = frame(&mut win, &mut doc, ViewMode::ThreeD, dark);
            assert!(
                text.contains("Relax the other degrees of freedom"),
                "{text}"
            );
            assert!(text.contains("optimized at each point"), "{text}");
            assert!(text.contains("1 point did not converge"), "{text}");
        }
        // From the assembly view the box is there but off: a single-point scan.
        let text = frame(&mut win, &mut doc, ViewMode::Assembly, false);
        assert!(
            text.contains("Relax the other degrees of freedom"),
            "{text}"
        );
        assert!(text.contains("no optimization"), "{text}");

        apply_point(&mut doc, 1);
        let (m, _) = doc.three_d.model_and_edit().unwrap();
        assert_eq!(edit::coords(m), relaxed);
    }

    /// A model scan over the chain's two end bonds and its dihedral, `energy` at each point.
    fn scan_3d(energy: impl Fn(&[f64]) -> Option<f64>) -> (Scan, Vec<Param>) {
        let params = vec![
            Param::Bond([0, 1]),
            Param::Bond([2, 3]),
            Param::Dihedral([0, 1, 2, 3]),
        ];
        let scan = scan_of(
            vec![
                (params[0].clone(), vec![1.4, 1.5, 1.6]),
                (params[1].clone(), vec![1.3, 1.5]),
                (params[2].clone(), vec![-120.0, 0.0, 120.0, 180.0]),
            ],
            energy,
        );
        (scan, params)
    }

    /// A three-parameter scan draws a heatmap of two of them and offers, for the third, a
    /// slice or the lowest energy; a curve of one, the others reduced, draws too.
    #[test]
    fn a_3d_scan_renders_a_section_headless_in_both_themes() {
        let mut doc = Document::new(&chembuider_rs::Config::default(), 1);
        let model = chain_model();
        doc.three_d = three_d::Job::with_model(model.clone(), &doc.editor.molecule);
        let (scan, params) = scan_3d(|v| Some((v[0] - 1.5).powi(2) + v[1] + v[2].abs() / 100.0));
        assert_eq!(scan.total(), 24);
        doc.scan = Some(Run::finished(scan, Source::model(&model, None, params)));
        let mut win = ScanWindow {
            open: true,
            ..Default::default()
        };
        for dark in [false, true] {
            let text = frame(&mut win, &mut doc, ViewMode::ThreeD, dark);
            assert!(text.contains("24 / 24 points"), "{text}");
            assert!(text.contains("Plot"), "{text}");
            assert!(text.contains("Slice") && text.contains("Lowest"), "{text}");
            assert!(text.contains("Minimum"), "{text}");
        }
        assert_eq!(win.view.shown(), [0, 1]);
        assert_eq!(win.view.reduce[2], Reduce::Minimum);
        // A curve along the dihedral, the bonds sliced.
        win.view = ViewForm {
            across: 2,
            up: None,
            reduce: vec![Reduce::Slice(2), Reduce::Slice(0), Reduce::Minimum],
        };
        for dark in [false, true] {
            let text = frame(&mut win, &mut doc, ViewMode::ThreeD, dark);
            assert!(text.contains("Slices at the minimum"), "{text}");
            assert!(text.contains("1.600 Å"), "the slice's value: {text}");
        }
    }

    /// Clicking a cell of a projection applies the point that has its lowest energy, with
    /// every one of its values, not merely the values plotted.
    #[test]
    fn a_projected_cell_applies_the_point_that_has_its_minimum() {
        let mut doc = Document::new(&chembuider_rs::Config::default(), 1);
        let model = chain_model();
        doc.three_d = three_d::Job::with_model(model.clone(), &doc.editor.molecule);
        // Lowest at a dihedral of 0° for every pair of bonds.
        let (scan, params) = scan_3d(|v| Some(v[0] + v[1] + (v[2] / 60.0).powi(2)));
        let view = ViewForm {
            across: 0,
            up: Some(1),
            reduce: vec![Reduce::Minimum; 3],
        };
        let section = Section::of(&scan, &view);
        assert_eq!(section.projected, [2]);
        let cell = 2 * 2 + 1; // bond 1 = 1.6, bond 2 = 1.5
        let index = section.cells[cell];
        assert_eq!(scan.values_at(index), [1.6, 1.5, 0.0]);
        doc.scan = Some(Run::finished(
            scan,
            Source::model(&model, None, params.clone()),
        ));
        let message = apply_point(&mut doc, index);
        assert!(message.contains("Applied"), "{message}");
        let (m, _) = doc.three_d.model_and_edit().unwrap();
        let coords = edit::coords(m);
        for (p, want) in params.iter().zip([1.6, 1.5, 0.0]) {
            let now = p.read_coords(&coords).unwrap();
            assert!((now - want).abs() < 1e-6, "{p:?}: {now}, not {want}");
        }
    }

    /// Settings beyond the cap on points (all axes together) cannot start: the window says how
    /// many and the cap, next to every axis's points as well as above the button.
    #[test]
    fn a_scan_of_too_many_points_cannot_start() {
        let mut doc = Document::new(&chembuider_rs::Config::default(), 1);
        let model = chain_model();
        doc.three_d = three_d::Job::with_model(model.clone(), &doc.editor.molecule);
        let form = |param: Param, points| AxisForm {
            param: Some(param),
            start: 1.0,
            end: 2.0,
            points,
        };
        let mut win = ScanWindow {
            open: true,
            axes: vec![
                form(Param::Bond([0, 1]), MAX_POINTS),
                form(Param::Bond([1, 2]), MAX_POINTS),
                form(Param::Bond([2, 3]), 2),
            ],
            ..Default::default()
        };
        let text = frame(&mut win, &mut doc, ViewMode::ThreeD, false);
        assert!(text.contains("Too many points (80000)"), "{text}");
        assert!(text.contains("at most 40000 points"), "{text}");
        assert_eq!(text.matches("→ 80000 points in all").count(), 3, "{text}");
        let axes: Vec<Axis> = win.axes.iter().filter_map(AxisForm::axis).collect();
        assert!(refusal(&axes, model.atoms.len(), false).is_some());
        assert!(Run::start(Source::model(&model, None, vec![]), &axes, ForceField::Uff).is_err());
        // One point fewer along the last axis: allowed (rigid; relaxed, the relaxed caps).
        win.axes[2].points = 1;
        let axes: Vec<Axis> = win.axes.iter().filter_map(AxisForm::axis).collect();
        assert_eq!(refusal(&axes, model.atoms.len(), false), None);
        let relaxed = refusal(&axes, model.atoms.len(), true).unwrap();
        assert!(relaxed.contains("Too many points to relax"), "{relaxed}");
        let text = frame(&mut win, &mut doc, ViewMode::ThreeD, false);
        assert!(!text.contains("Too many points"), "{text}");
        // The same parameter twice is refused too.
        win.axes[1].param = Some(Param::Bond([0, 1]));
        let axes: Vec<Axis> = win.axes.iter().filter_map(AxisForm::axis).collect();
        assert!(refusal(&axes, 4, false).unwrap().contains("same parameter"));
    }

    /// Axes are added, moved and removed; the last one stays.
    #[test]
    fn axes_can_be_reordered_and_removed() {
        let form = |points| AxisForm {
            points,
            ..Default::default()
        };
        let mut axes = vec![form(1), form(2), form(3)];
        let points = |axes: &[AxisForm]| axes.iter().map(|f| f.points).collect::<Vec<_>>();
        edit_axes(&mut axes, 2, AxisAction::Up);
        assert_eq!(points(&axes), [1, 3, 2]);
        edit_axes(&mut axes, 0, AxisAction::Down);
        assert_eq!(points(&axes), [3, 1, 2]);
        edit_axes(&mut axes, 0, AxisAction::Up);
        edit_axes(&mut axes, 2, AxisAction::Down);
        assert_eq!(points(&axes), [3, 1, 2], "nothing past the ends");
        edit_axes(&mut axes, 1, AxisAction::Remove);
        assert_eq!(points(&axes), [3, 2]);
        edit_axes(&mut axes, 0, AxisAction::Remove);
        edit_axes(&mut axes, 0, AxisAction::Remove);
        assert_eq!(points(&axes), [2], "one axis stays");

        // The window starts with one axis; "+ Add axis" is there.
        let mut doc = Document::new(&chembuider_rs::Config::default(), 1);
        doc.three_d = three_d::Job::with_model(chain_model(), &doc.editor.molecule);
        let mut win = ScanWindow {
            open: true,
            ..Default::default()
        };
        let text = frame(&mut win, &mut doc, ViewMode::ThreeD, false);
        assert_eq!(win.axes.len(), 1);
        assert!(text.contains("+ Add axis"), "{text}");
    }

    /// The view fits the scan shown: the first two axes plotted for a new number of axes,
    /// slices kept within their axis, and never the same axis across and up.
    #[test]
    fn the_view_fits_the_scan() {
        let mut view = ViewForm::default();
        view.fit(&[3, 4, 5]);
        assert_eq!(view.shown(), [0, 1]);
        assert_eq!(view.reduce, [Reduce::Minimum; 3]);
        view.reduce[2] = Reduce::Slice(9);
        view.up = Some(0);
        view.fit(&[3, 4, 5]);
        assert_eq!(view.reduce[2], Reduce::Slice(4));
        assert_eq!(view.up, None, "not the axis across");
        view.across = 2;
        view.up = Some(1);
        view.fit(&[3, 4, 5, 2]);
        assert_eq!(view.shown(), [0, 1], "another scan: reset");
    }

    /// However many cells a plot draws (a 200 × 200 map, of two axes or of three, one of them
    /// a single value; a section of three axes of 34 values), a frame finds the minimum — the
    /// ΔE baseline — once, never per cell, and works the section out only when it changes.
    #[test]
    fn a_frame_passes_over_the_points_once_however_many_cells_it_draws() {
        use crate::scan::passes::{self, MINIMUM, SECTION};
        let mut doc = Document::new(&chembuider_rs::Config::default(), 1);
        let model = chain_model();
        doc.three_d = three_d::Job::with_model(model.clone(), &doc.editor.molecule);
        let params = [
            Param::Bond([0, 1]),
            Param::Bond([2, 3]),
            Param::Dihedral([0, 1, 2, 3]),
        ];
        let values = |n: usize| (0..n).map(|k| 1.0 + 0.01 * k as f64).collect::<Vec<_>>();
        for sizes in [&[200, 200][..], &[200, 200, 1], &[34, 34, 34]] {
            let axes = params
                .iter()
                .zip(sizes)
                .map(|(p, &n)| (p.clone(), values(n)))
                .collect();
            // A pending point and a failed one among them.
            let scan = scan_of(axes, |v| {
                let e = (v[0] - 1.5).powi(2) + (v[1] - 1.2).powi(2);
                (v[0] != 1.0 || v[1] != 1.0).then_some(if v[1] == 1.01 { f64::NAN } else { e })
            });
            doc.scan = Some(Run::finished(
                scan,
                Source::model(&model, None, params[..sizes.len()].to_vec()),
            ));
            let mut win = ScanWindow {
                open: true,
                ..Default::default()
            };
            for relative in [true, false] {
                win.set_relative(relative);
                passes::take(&MINIMUM);
                passes::take(&SECTION);
                // Two frames.
                let text = frame(&mut win, &mut doc, ViewMode::ThreeD, false);
                assert!(text.contains("Minimum"), "{sizes:?}: {text}");
                assert_eq!(passes::take(&MINIMUM), 2, "{sizes:?}: once a frame");
                let first = usize::from(sizes.len() > 2 && relative);
                assert_eq!(passes::take(&SECTION), first, "{sizes:?}: kept");
            }
            if sizes.len() > 2 {
                // Another view: a curve along the first axis, the others minimised over.
                win.view.up = None;
                frame(&mut win, &mut doc, ViewMode::ThreeD, false);
                assert_eq!(passes::take(&MINIMUM), 2, "{sizes:?}: once a frame");
                assert_eq!(passes::take(&SECTION), 1, "{sizes:?}: once for the view");
            }
        }
    }

    /// The section kept is worked out again for more points in, another view or another run.
    #[test]
    fn a_kept_section_follows_the_scan_and_the_view() {
        use crate::scan::passes::{self, SECTION};
        let model = chain_model();
        let (mut scan, params) = scan_3d(|v| Some(v[0] + v[1] + v[2] / 100.0));
        // Point 5 = (1.4, 1.5, 0°), not in yet.
        scan.energies[5] = None;
        let mut run = Run::finished(scan, Source::model(&model, None, params.clone()));
        let mut view = ViewForm::default();
        view.fit(&run.scan.shape());
        let mut cache = SectionCache::default();
        passes::take(&SECTION);
        let before = cache.get(&run, &view).cells.clone();
        assert_eq!(cache.get(&run, &view).cells, before);
        assert_eq!(passes::take(&SECTION), 1, "the same scan and view: kept");

        run.scan.energies[5] = Some(-10.0);
        let after = cache.get(&run, &view).cells.clone();
        assert_eq!(passes::take(&SECTION), 1, "a point in");
        assert_ne!(after, before);
        assert_eq!(after[1], 5, "the new lowest of its cell");

        view.reduce[2] = Reduce::Slice(3);
        let sliced = cache.get(&run, &view).cells.clone();
        assert_eq!(passes::take(&SECTION), 1, "another view");
        assert_eq!(sliced, Section::of(&run.scan, &view).cells);
        passes::take(&SECTION);

        let other = Run::finished(run.scan.clone(), Source::model(&model, None, params));
        cache.get(&other, &view);
        assert_eq!(passes::take(&SECTION), 1, "another run");
    }

    #[test]
    fn the_colormap_runs_dark_to_light() {
        let lum = |c: Color32| 0.3 * c.r() as f64 + 0.59 * c.g() as f64 + 0.11 * c.b() as f64;
        let samples: Vec<f64> = (0..=20).map(|k| lum(viridis(k as f64 / 20.0))).collect();
        assert!(
            samples.windows(2).all(|w| w[1] >= w[0] - 1.0),
            "{samples:?}"
        );
        assert_eq!(viridis(-1.0), viridis(0.0));
        assert_eq!(viridis(f64::NAN), viridis(0.0));
        assert_eq!(cells(&[3.0, 2.0, 1.0]), (1.0, 1.0, true));
        assert_eq!(cells(&[5.0]), (5.0, 1.0, false));
    }
}
