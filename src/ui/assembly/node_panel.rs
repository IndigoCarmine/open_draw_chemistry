//! The 構築 tab's node section: the parameters of the node selected in the node editor, and
//! the custom nodes — the design's own and the user's library.

use egui::{DragValue, RichText, Ui};

use super::nodes::{self, GraphEdit, NodeEditorView};
use super::style::{self, Look};
use super::{Ink, step_params};
use crate::assembly::AssemblyState;
use crate::assembly::graph::defs::{self, DefId, NodeDef, Status};
use crate::assembly::graph::eval::is_name;
use crate::assembly::graph::expr::{Expr, eval_const};
use crate::assembly::graph::kinds::{
    Coords, CurveSample, EachPivot, NodeKind, PortValue, SocketType, Var, next_var_key,
    next_var_name,
};
use crate::assembly::graph::{InKey, NodeId, access, library, ops};
use crate::assembly::model::{Design, node_name};
use crate::ui::Command;

/// The node section: what is selected in the editor, then the custom nodes.
pub(super) fn section(
    ui: &mut Ui,
    state: &mut AssemblyState,
    view: &mut NodeEditorView,
    look: &Look,
    ink: &Ink,
    out: &mut Vec<Command>,
) {
    style::caption(ui, look, t!("assembly.panel.section_node"));
    // Only an assembly steps cannot hold comes here: say why there are no step cards.
    ui.label(
        RichText::new(t!("assembly.panel.graph_note"))
            .size(11.5)
            .color(look.muted),
    );
    if !view.open && ui.button(t!("assembly.panel.open_editor")).clicked() {
        view.open = true;
    }
    let scope = view.rt.editor.scope.clone();
    let selected: Vec<NodeId> = view.rt.editor.selected.iter().copied().collect();
    style::card(ui, look, false, 0.0, |ui| match selected.as_slice() {
        [] => {
            ui.label(look.muted(t!("assembly.panel.no_node")));
        }
        [id] => node_card(ui, state, view, &scope, *id, look, ink),
        many => {
            ui.label(RichText::new(tn!("assembly.panel.nodes_selected", many.len())).size(12.5));
            ui.horizontal_wrapped(|ui| {
                let chosen = view.rt.editor.selected.clone();
                if ui.button(t!("assembly.editor.group")).clicked() {
                    nodes::apply(state, &mut view.rt, GraphEdit::Group(chosen.clone()));
                }
                if ui.button(t!("assembly.panel.delete")).clicked() {
                    nodes::apply(state, &mut view.rt, GraphEdit::Remove(chosen));
                }
            });
        }
    });
    custom_nodes(ui, state, view, look, ink, out);
}

/// The selected node: its name, whether it is bypassed, what is wrong with it, and its
/// settings.
fn node_card(
    ui: &mut Ui,
    state: &mut AssemblyState,
    view: &mut NodeEditorView,
    scope: &[NodeId],
    id: NodeId,
    look: &Look,
    ink: &Ink,
) {
    let Some((graph, owner)) = access::graph_at(&state.design, scope) else {
        return;
    };
    let Some(original) = graph.node(id).cloned() else {
        return;
    };
    let graph = graph.into_owned();
    let owner = owner.cloned();
    let ev = state.design.evaluation();
    let mut node = original.clone();
    let defs = state.design.defs.clone();

    ui.horizontal(|ui| {
        ui.label(
            RichText::new(node.kind.title(&defs))
                .size(13.0)
                .strong()
                .color(look.ink),
        );
        ui.add(
            egui::TextEdit::singleline(&mut node.label)
                .hint_text(t!("assembly.panel.label_hint"))
                .desired_width(110.0),
        )
        .on_hover_text(t!("assembly.panel.label_tip"));
    });
    let passes = node
        .kind
        .outputs(&defs, owner.as_ref())
        .iter()
        .any(|o| o.ty == SocketType::Set);
    if passes {
        ui.checkbox(&mut node.bypass, t!("assembly.panel.bypass"))
            .on_hover_text(t!("assembly.panel.bypass_tip"));
    }
    // Inputs fed by wires: their value comes from there.
    let wired: Vec<String> = node
        .kind
        .inputs(&defs, owner.as_ref())
        .into_iter()
        .filter(|i| i.ty != SocketType::Set)
        .filter_map(|i| {
            let link = graph.link_into(id, i.key)?;
            let from = node_name(&graph, &defs, &[], link.from);
            Some(t!("assembly.panel.wired", input = i.label, from = from))
        })
        .collect();
    ui.add_space(2.0);
    let vars_hint = variables_hint(&state.design, &graph, scope, id);
    ui.add_enabled_ui(!node.bypass, |ui| {
        kind_params(ui, &mut node.kind, &state.design, &vars_hint, ink);
    });
    // The node's errors come and go as an expression is typed: under the fields, so the field
    // being typed in keeps its place (and its id, and so the focus).
    for issue in ev
        .errors
        .iter()
        .filter(|i| i.node == id && i.scope == scope)
    {
        ui.label(RichText::new(&issue.message).size(11.5).color(ink.warn));
    }
    for issue in ev
        .warnings
        .iter()
        .filter(|i| i.node == id && i.scope == scope)
    {
        ui.label(RichText::new(&issue.message).size(11.5).color(look.muted));
    }

    for w in wired {
        ui.label(look.muted(w));
    }

    // What can be done with it.
    ui.add_space(4.0);
    let mut edit = None;
    ui.horizontal_wrapped(|ui| {
        if let NodeKind::Group(_) = &node.kind {
            if ui
                .button(t!("assembly.panel.enter"))
                .on_hover_text(t!("assembly.panel.enter_tip"))
                .clicked()
            {
                view.rt.editor.scope.push(id);
                view.rt.editor.selected.clear();
            }
            if ui.button(t!("assembly.panel.ungroup")).clicked() {
                edit = Some(GraphEdit::Ungroup(id));
            }
        }
        if ops::removable(&node) {
            if ui.button(t!("assembly.panel.duplicate")).clicked() {
                edit = Some(GraphEdit::Duplicate([id].into()));
            }
            if ui.button(t!("assembly.panel.delete")).clicked() {
                edit = Some(GraphEdit::Remove([id].into()));
            }
        }
    });
    // Inside a custom node: an unwired input can become one of the custom node's inputs.
    if let Some(def) = &owner {
        let exposable: Vec<_> = node
            .kind
            .inputs(&defs, Some(def))
            .into_iter()
            .filter(|i| graph.link_into(id, i.key).is_none())
            .collect();
        if !exposable.is_empty() {
            ui.menu_button(t!("assembly.panel.expose"), |ui| {
                for i in exposable {
                    if ui.button(&i.label).clicked() {
                        let key = i.key;
                        let def_id = def.id.clone();
                        state.edit(|d| {
                            let snapshot = d.defs.clone();
                            if let Some(def) = defs::find_mut(&mut d.defs, &def_id) {
                                ops::expose(def, &snapshot, id, key);
                            }
                        });
                        ui.close();
                    }
                }
            })
            .response
            .on_hover_text(t!("assembly.panel.expose_tip"));
        }
    }
    // A custom node in use, or its inside: its name and its inputs and outputs.
    let def_id = match (&node.kind, &owner) {
        (NodeKind::Group(g), _) => Some(g.def.clone()),
        (NodeKind::GroupInput | NodeKind::GroupOutput, Some(def)) => Some(def.id.clone()),
        _ => None,
    };
    if node != original {
        nodes::set_node(state, scope, node);
    }
    if let Some(def_id) = def_id {
        ui.add_space(4.0);
        def_editor(ui, state, &def_id, look);
    }
    if let Some(edit) = edit {
        nodes::apply(state, &mut view.rt, edit);
    }
}

/// The variables an expression of node `id` can use: its own, the attributes of the molecules
/// fed to it, and the fixed ones.
fn variables_hint(
    design: &Design,
    graph: &crate::assembly::graph::Graph,
    scope: &[NodeId],
    id: NodeId,
) -> String {
    let ev = design.evaluation();
    let mut names: Vec<String> = Vec::new();
    if let Some(link) = graph.link_into(id, InKey::Set(0))
        && let Some(set) = ev.sets.get(&(scope.to_vec(), link.from, link.out))
    {
        names.extend(set.attrs.iter().map(|(n, _)| n.to_string()));
    }
    for fixed in ["k", "count", "comp", "x", "y", "z", "i", "n"] {
        if !names.iter().any(|n| n == fixed) {
            names.push(fixed.to_owned());
        }
    }
    names.join(" ")
}

/// The settings of a node of each kind. Returns whether any changed.
fn kind_params(
    ui: &mut Ui,
    kind: &mut NodeKind,
    design: &Design,
    vars_hint: &str,
    ink: &Ink,
) -> bool {
    let mut changed = false;
    match kind {
        NodeKind::Monomer { component } => {
            let current = design
                .component_index(*component)
                .and_then(|i| design.components.get(i))
                .map_or_else(|| "?".to_owned(), |c| c.name.clone());
            egui::ComboBox::from_label(t!("assembly.node.monomer"))
                .selected_text(current)
                .show_ui(ui, |ui| {
                    for (i, c) in design.components.iter().enumerate() {
                        let cid = design.component_id(i);
                        changed |= ui.selectable_value(component, cid, &c.name).changed();
                    }
                });
        }
        NodeKind::Op(step) => changed |= step_params(ui, step, ink),
        NodeKind::Merge { inputs } => {
            changed |= super::field_row(
                ui,
                t!("assembly.panel.inputs"),
                t!("assembly.panel.inputs_tip"),
                |ui| ui.add(DragValue::new(inputs).range(1..=32)).changed(),
            );
        }
        NodeKind::Select(s) => {
            ui.horizontal(|ui| {
                ui.label(RichText::new(t!("assembly.panel.keep_if")).size(12.0));
                egui::ComboBox::from_id_salt("select_presets")
                    .selected_text(t!("assembly.panel.presets"))
                    .show_ui(ui, |ui| {
                        for (label, expr) in [
                            (t!("assembly.panel.preset_every_other"), "k % 2 == 0"),
                            (t!("assembly.panel.preset_first_half"), "k < count / 2"),
                            (t!("assembly.panel.preset_above"), "z > 0"),
                            (t!("assembly.panel.preset_monomer"), "comp == 0"),
                            (t!("assembly.panel.preset_even_layers"), "layer % 2 == 0"),
                        ] {
                            if ui.selectable_label(false, label).clicked() {
                                s.expr = expr.to_owned();
                                changed = true;
                            }
                        }
                    });
            });
            changed |= expr_field(ui, &mut s.expr, &s.vars, vars_hint, true, ink);
            changed |= vars_editor(ui, &mut s.vars);
        }
        NodeKind::Repeat(r) => {
            changed |= super::count(
                ui,
                t!("assembly.node.socket.count"),
                &mut r.n,
                t!("assembly.panel.repeat_count_tip"),
            );
            changed |= super::field_row(
                ui,
                t!("assembly.panel.attr_name"),
                t!("assembly.panel.repeat_name_tip"),
                |ui| {
                    ui.add(egui::TextEdit::singleline(&mut r.name).desired_width(80.0))
                        .changed()
                },
            );
            ui.label(
                RichText::new(t!("assembly.panel.repeat_help"))
                    .size(11.0)
                    .color(ink.muted),
            );
            let hint = "i n";
            for (axis, e) in ["x", "y", "z"].iter().zip(r.offset.iter_mut()) {
                changed |= labelled_expr(
                    ui,
                    &format!("{} {axis}", t!("assembly.model.array.offset")),
                    e,
                    &r.vars,
                    hint,
                    ink,
                );
            }
            for (axis, e) in ["x", "y", "z"].iter().zip(r.euler_deg.iter_mut()) {
                changed |= labelled_expr(
                    ui,
                    &format!("{} {axis} °", t!("assembly.model.array.rotation")),
                    e,
                    &r.vars,
                    hint,
                    ink,
                );
            }
            changed |= vars_editor(ui, &mut r.vars);
        }
        NodeKind::MoveEach(m) => {
            for (axis, e) in ["x", "y", "z"].iter().zip(m.offset.iter_mut()) {
                changed |= labelled_expr(
                    ui,
                    &format!("{} {axis}", t!("assembly.model.transform.offset")),
                    e,
                    &m.vars,
                    vars_hint,
                    ink,
                );
            }
            for (axis, e) in ["x", "y", "z"].iter().zip(m.euler_deg.iter_mut()) {
                changed |= labelled_expr(
                    ui,
                    &format!("{} {axis} °", t!("assembly.model.transform.rotation")),
                    e,
                    &m.vars,
                    vars_hint,
                    ink,
                );
            }
            changed |= super::field_row(
                ui,
                t!("assembly.model.transform.pivot"),
                t!("assembly.panel.each_pivot_tip"),
                |ui| {
                    let mut c = false;
                    for (p, label) in [
                        (EachPivot::Own, t!("assembly.panel.pivot_own")),
                        (EachPivot::Set, t!("assembly.panel.pivot_set")),
                        (
                            EachPivot::Origin,
                            t!("assembly.model.transform.pivot_origin"),
                        ),
                    ] {
                        c |= ui.selectable_value(&mut m.pivot, p, label).changed();
                    }
                    c
                },
            );
            ui.label(
                RichText::new(t!("assembly.panel.variables", names = vars_hint))
                    .size(11.0)
                    .color(ink.muted),
            );
            changed |= vars_editor(ui, &mut m.vars);
        }
        NodeKind::Circle(c) => {
            changed |= super::count(
                ui,
                t!("assembly.node.socket.resolution"),
                &mut c.resolution,
                t!("assembly.panel.circle_resolution_tip"),
            );
            changed |= super::number(
                ui,
                t!("assembly.node.socket.radius"),
                &mut c.radius,
                0.05,
                0.0..=1.0e5,
                " Å",
                t!("assembly.panel.circle_radius_tip"),
            );
        }
        NodeKind::ParamCurve(c) => {
            changed |= super::field_row(
                ui,
                t!("assembly.panel.coords"),
                t!("assembly.panel.coords_tip"),
                |ui| {
                    let mut m = false;
                    for (coords, label) in [
                        (Coords::Cartesian, t!("assembly.panel.coords_cartesian")),
                        (Coords::Polar, t!("assembly.panel.coords_polar")),
                    ] {
                        m |= ui.selectable_value(&mut c.coords, coords, label).changed();
                    }
                    m
                },
            );
            let axes = match c.coords {
                Coords::Cartesian => ["x", "y", "z"],
                Coords::Polar => ["r", "θ °", "z"],
            };
            for (axis, e) in axes.iter().zip(c.exprs.iter_mut()) {
                changed |= curve_expr(ui, axis, e, &c.vars, ink);
            }
            changed |= super::number(
                ui,
                t!("assembly.node.socket.t_min"),
                &mut c.t_min,
                0.01,
                f64::MIN..=f64::MAX,
                "",
                t!("assembly.panel.t_range_tip"),
            );
            changed |= super::number(
                ui,
                t!("assembly.node.socket.t_max"),
                &mut c.t_max,
                0.01,
                f64::MIN..=f64::MAX,
                "",
                t!("assembly.panel.t_range_tip"),
            );
            changed |= super::count(
                ui,
                t!("assembly.node.socket.resolution"),
                &mut c.resolution,
                t!("assembly.panel.curve_resolution_tip"),
            );
            changed |= ui
                .checkbox(&mut c.closed, t!("assembly.panel.closed"))
                .on_hover_text(t!("assembly.panel.closed_tip"))
                .changed();
            ui.label(
                RichText::new(t!("assembly.panel.param_curve_help"))
                    .size(11.0)
                    .color(ink.muted),
            );
            changed |= vars_editor(ui, &mut c.vars);
        }
        NodeKind::CurveToPoints(c) => {
            changed |= super::field_row(
                ui,
                t!("assembly.panel.curve_mode"),
                t!("assembly.panel.curve_mode_tip"),
                |ui| {
                    let mut m = false;
                    for (mode, label) in [
                        (CurveSample::Count, t!("assembly.panel.curve_count")),
                        (CurveSample::Length, t!("assembly.panel.curve_length")),
                        (CurveSample::Evaluated, t!("assembly.panel.curve_evaluated")),
                    ] {
                        m |= ui.selectable_value(&mut c.mode, mode, label).changed();
                    }
                    m
                },
            );
            match c.mode {
                CurveSample::Count => {
                    changed |= super::count(
                        ui,
                        t!("assembly.node.socket.count"),
                        &mut c.count,
                        t!("assembly.panel.curve_count_tip"),
                    );
                }
                CurveSample::Length => {
                    changed |= super::number(
                        ui,
                        t!("assembly.node.socket.length"),
                        &mut c.length,
                        0.05,
                        0.001..=1.0e5,
                        " Å",
                        t!("assembly.panel.curve_length_tip"),
                    );
                }
                CurveSample::Evaluated => {}
            }
            ui.label(
                RichText::new(t!("assembly.panel.curve_frame_help"))
                    .size(11.0)
                    .color(ink.muted),
            );
        }
        NodeKind::InstanceOnPoints(p) => {
            changed |= ui
                .checkbox(&mut p.align, t!("assembly.panel.align_to_points"))
                .on_hover_text(t!("assembly.panel.align_to_points_tip"))
                .changed();
            changed |= super::number(
                ui,
                t!("assembly.node.socket.selection"),
                &mut p.selection,
                0.05,
                f64::MIN..=f64::MAX,
                "",
                t!("assembly.panel.selection_tip"),
            );
            changed |= super::vector(
                ui,
                t!("assembly.node.socket.rotation"),
                &mut p.rotation_deg,
                0.5,
                "°",
                t!("assembly.panel.instance_rotation_tip"),
            );
            ui.label(
                RichText::new(t!("assembly.panel.instance_help"))
                    .size(11.0)
                    .color(ink.muted),
            );
        }
        NodeKind::Index => {
            ui.label(
                RichText::new(t!("assembly.panel.index_help"))
                    .size(11.0)
                    .color(ink.muted),
            );
        }
        NodeKind::SetAttr(a) => {
            changed |= super::field_row(
                ui,
                t!("assembly.panel.attr_name"),
                t!("assembly.panel.attr_name_tip"),
                |ui| {
                    ui.add(egui::TextEdit::singleline(&mut a.name).desired_width(80.0))
                        .changed()
                },
            );
            if !is_name(a.name.trim()) {
                ui.label(
                    RichText::new(t!("assembly.node.error.bad_name", name = a.name))
                        .size(11.0)
                        .color(ink.warn),
                );
            }
            changed |= expr_field(ui, &mut a.expr, &a.vars, vars_hint, true, ink);
            changed |= vars_editor(ui, &mut a.vars);
        }
        NodeKind::Value { value } => {
            changed |= super::field_row(
                ui,
                t!("assembly.node.socket.value"),
                t!("assembly.panel.value_tip"),
                |ui| {
                    ui.add(
                        DragValue::new(value)
                            .speed(0.05)
                            .max_decimals(4)
                            .custom_parser(eval_const),
                    )
                    .changed()
                },
            );
        }
        NodeKind::Vector { value } | NodeKind::Combine { value } | NodeKind::Separate { value } => {
            changed |= super::vector(
                ui,
                t!("assembly.node.socket.vector"),
                value,
                0.05,
                "",
                t!("assembly.panel.value_tip"),
            );
        }
        NodeKind::Expression(e) => {
            changed |= expr_field(ui, &mut e.expr, &e.vars, "", false, ink);
            changed |= vars_editor(ui, &mut e.vars);
        }
        NodeKind::Group(g) => {
            if let Some(def) = defs::find(&design.defs, &g.def) {
                for p in def.inputs.iter().filter(|p| p.ty != SocketType::Set) {
                    let mut value =
                        g.inputs
                            .get(&p.key)
                            .copied()
                            .or(p.default)
                            .unwrap_or(match p.ty {
                                SocketType::Vec3 => PortValue::Vec3([0.0; 3]),
                                _ => PortValue::Number(0.0),
                            });
                    let c = match &mut value {
                        PortValue::Number(v) => super::field_row(ui, &p.name, "", |ui| {
                            ui.add(
                                DragValue::new(v)
                                    .speed(0.05)
                                    .max_decimals(4)
                                    .custom_parser(eval_const),
                            )
                            .changed()
                        }),
                        PortValue::Vec3(v) => super::vector(ui, &p.name, v, 0.05, "", ""),
                    };
                    if c {
                        g.inputs.insert(p.key, value);
                        changed = true;
                    }
                }
            }
        }
        NodeKind::GroupInput | NodeKind::GroupOutput => {
            ui.label(
                RichText::new(t!("assembly.panel.ports_help"))
                    .size(11.0)
                    .color(ink.muted),
            );
        }
        NodeKind::Output => {
            let n = design.evaluation().output.len();
            ui.label(RichText::new(tn!("assembly.view.unit_molecules", n)).size(12.0));
            ui.label(
                RichText::new(t!("assembly.panel.output_help"))
                    .size(11.0)
                    .color(ink.muted),
            );
        }
    }
    changed
}

fn labelled_expr(
    ui: &mut Ui,
    label: &str,
    expr: &mut String,
    vars: &[Var],
    hint: &str,
    ink: &Ink,
) -> bool {
    let mut changed = false;
    super::field_row(ui, label, "", |ui| {
        changed |= ui
            .add(
                egui::TextEdit::singleline(expr)
                    .font(egui::TextStyle::Monospace)
                    .desired_width(f32::INFINITY),
            )
            .changed();
        false
    });
    if let Err(e) = check(expr, vars, hint, true) {
        ui.label(RichText::new(e).size(11.0).color(ink.warn));
    }
    changed
}

/// One of a parametric curve's expressions, labelled with its axis: it reads `t`, `i`, `n`
/// and the node's variables.
fn curve_expr(ui: &mut Ui, axis: &str, expr: &mut String, vars: &[Var], ink: &Ink) -> bool {
    let mut changed = false;
    super::field_row(ui, axis, "", |ui| {
        changed |= ui
            .add(
                egui::TextEdit::singleline(expr)
                    .font(egui::TextStyle::Monospace)
                    .desired_width(f32::INFINITY),
            )
            .on_hover_text(t!("assembly.panel.expr_tip"))
            .changed();
        false
    });
    if let Err(e) = check(expr, vars, "t i n", false) {
        ui.label(RichText::new(e).size(11.0).color(ink.warn));
    }
    changed
}

/// An expression with its error under it, and the names it may use.
fn expr_field(
    ui: &mut Ui,
    expr: &mut String,
    vars: &[Var],
    hint: &str,
    per_item: bool,
    ink: &Ink,
) -> bool {
    let changed = ui
        .add(
            egui::TextEdit::singleline(expr)
                .font(egui::TextStyle::Monospace)
                .desired_width(f32::INFINITY),
        )
        .on_hover_text(t!("assembly.panel.expr_tip"))
        .changed();
    if let Err(e) = check(expr, vars, hint, per_item) {
        ui.label(RichText::new(e).size(11.0).color(ink.warn));
    }
    if per_item {
        ui.label(
            RichText::new(t!("assembly.panel.variables", names = hint))
                .size(11.0)
                .color(ink.muted),
        );
    }
    changed
}

/// Whether `expr` reads with the node's variables and the names in `hint` (any other name is
/// taken as an attribute some molecules may have when `per_item`).
fn check(expr: &str, vars: &[Var], hint: &str, per_item: bool) -> Result<(), String> {
    let known = |name: &str| {
        vars.iter().any(|v| v.name == name)
            || hint.split(' ').any(|h| h == name)
            || per_item && is_name(name) && !matches!(name, "pi" | "e")
    };
    Expr::parse(expr, &|n| known(n).then_some(0))
        .map(|_| ())
        .map_err(|e| crate::assembly::graph::expr::message(&e))
}

/// The variables of an expression node: name, value (when unwired), and buttons to remove or
/// add one.
fn vars_editor(ui: &mut Ui, vars: &mut Vec<Var>) -> bool {
    let mut changed = false;
    let mut remove = None;
    for (k, v) in vars.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            changed |= ui
                .add(
                    egui::TextEdit::singleline(&mut v.name)
                        .font(egui::TextStyle::Monospace)
                        .desired_width(48.0),
                )
                .changed();
            changed |= ui
                .add(
                    DragValue::new(&mut v.value)
                        .speed(0.05)
                        .max_decimals(4)
                        .custom_parser(eval_const),
                )
                .changed();
            if ui
                .small_button("✕")
                .on_hover_text(t!("assembly.panel.remove_var"))
                .clicked()
            {
                remove = Some(k);
            }
        });
    }
    if let Some(k) = remove {
        vars.remove(k);
        changed = true;
    }
    if ui
        .small_button(t!("assembly.panel.add_var"))
        .on_hover_text(t!("assembly.panel.add_var_tip"))
        .clicked()
    {
        vars.push(Var {
            key: next_var_key(vars),
            name: next_var_name(vars),
            value: 0.0,
        });
        changed = true;
    }
    changed
}

/// A custom node's name, description and ports. Edits apply to every node using it.
fn def_editor(ui: &mut Ui, state: &mut AssemblyState, def_id: &DefId, look: &Look) {
    let Some(original) = defs::find(&state.design.defs, def_id).cloned() else {
        return;
    };
    let mut def = original.clone();
    ui.label(
        RichText::new(t!("assembly.panel.custom_node"))
            .size(11.0)
            .color(look.muted),
    );
    super::field_row(ui, t!("assembly.panel.name"), "", |ui| {
        ui.add(egui::TextEdit::singleline(&mut def.name).desired_width(f32::INFINITY))
            .changed()
    });
    ui.add(
        egui::TextEdit::multiline(&mut def.description)
            .hint_text(t!("assembly.panel.description_hint"))
            .desired_rows(1)
            .desired_width(f32::INFINITY),
    );
    for (title, ports) in [
        (t!("assembly.panel.inputs"), &mut def.inputs),
        (t!("assembly.panel.outputs"), &mut def.outputs),
    ] {
        ui.label(RichText::new(title).size(11.0).color(look.muted));
        let mut remove = None;
        for (k, p) in ports.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                let kind = match p.ty {
                    SocketType::Set => "●",
                    SocketType::Number => "○",
                    SocketType::Vec3 => "◆",
                    SocketType::Curve => "∿",
                    SocketType::Points => "⁘",
                };
                ui.label(RichText::new(kind).size(11.0).color(look.muted));
                ui.add(egui::TextEdit::singleline(&mut p.name).desired_width(100.0));
                if let Some(PortValue::Number(v)) = &mut p.default {
                    ui.add(DragValue::new(v).speed(0.05).max_decimals(4))
                        .on_hover_text(t!("assembly.panel.default_tip"));
                }
                if ui
                    .small_button("✕")
                    .on_hover_text(t!("assembly.panel.remove_port"))
                    .clicked()
                {
                    remove = Some(k);
                }
            });
        }
        if let Some(k) = remove {
            ports.remove(k);
        }
    }
    if def != original {
        let ports_changed = def.inputs.len() != original.inputs.len()
            || def.outputs.len() != original.outputs.len();
        if let Some(slot) = defs::find_mut(&mut state.design.defs, def_id) {
            *slot = def;
        }
        if ports_changed {
            let design = &mut state.design;
            let mut defs = std::mem::take(&mut design.defs);
            let pruned = match &mut design.graph {
                Some(g) => ops::prune_all(g, &mut defs),
                None => ops::prune_all(&mut Default::default(), &mut defs),
            };
            design.defs = defs;
            let _ = pruned;
        }
        state.touch();
    }
}

/// The design's custom nodes, with how each stands to the library, and the library's.
pub(super) fn custom_nodes(
    ui: &mut Ui,
    state: &mut AssemblyState,
    view: &mut NodeEditorView,
    look: &Look,
    ink: &Ink,
    out: &mut Vec<Command>,
) {
    style::caption(ui, look, t!("assembly.panel.section_custom"));
    let used = state
        .design
        .graph
        .as_ref()
        .map(|g| defs::graph_uses_all(g, &state.design.defs))
        .unwrap_or_default();
    let embedded: Vec<NodeDef> = state.design.defs.clone();
    let lib = library::NodeLibrary::default();
    let mut action: Option<Action> = None;
    style::card(ui, look, false, 0.0, |ui| {
        if embedded.is_empty() {
            ui.label(look.muted(t!("assembly.panel.no_custom")));
        }
        for def in &embedded {
            let theirs = view
                .rt
                .library()
                .iter()
                .find(|e| e.def.id == def.id)
                .map(|e| e.def.clone());
            let status = defs::status(def, theirs.as_ref());
            ui.horizontal(|ui| {
                ui.label(RichText::new(&def.name).size(12.5));
                let (badge, color) = match status {
                    Status::Local => (t!("assembly.panel.status_local"), look.muted),
                    Status::InSync => (t!("assembly.panel.status_in_sync"), look.muted),
                    Status::UpdateAvailable => (t!("assembly.panel.status_update"), look.accent),
                    Status::Modified => (t!("assembly.panel.status_modified"), look.accent),
                    Status::Conflict => (t!("assembly.panel.status_conflict"), ink.warn),
                };
                ui.label(RichText::new(badge).size(10.5).color(color));
                if !used.contains(&def.id) {
                    ui.label(
                        RichText::new(t!("assembly.panel.unused"))
                            .size(10.5)
                            .color(look.muted),
                    );
                }
                ui.menu_button("…", |ui| {
                    if ui.button(t!("assembly.panel.add_node")).clicked() {
                        action = Some(Action::AddNode(def.id.clone()));
                        ui.close();
                    }
                    if ui
                        .button(t!("assembly.panel.save_to_library"))
                        .on_hover_text(t!("assembly.panel.save_to_library_tip"))
                        .clicked()
                    {
                        action = Some(Action::Save(def.id.clone()));
                        ui.close();
                    }
                    let can_take = matches!(
                        status,
                        Status::UpdateAvailable | Status::Modified | Status::Conflict
                    );
                    if ui
                        .add_enabled(
                            can_take,
                            egui::Button::new(t!("assembly.panel.take_library")),
                        )
                        .on_hover_text(t!("assembly.panel.take_library_tip"))
                        .clicked()
                    {
                        action = Some(Action::Take(def.id.clone()));
                        ui.close();
                    }
                    if ui
                        .button(t!("assembly.panel.fork"))
                        .on_hover_text(t!("assembly.panel.fork_tip"))
                        .clicked()
                    {
                        action = Some(Action::Fork(def.id.clone()));
                        ui.close();
                    }
                    if ui.button(t!("assembly.panel.export_node")).clicked() {
                        out.push(Command::AssemblyExportNode(def.id.clone()));
                        ui.close();
                    }
                    if ui
                        .add_enabled(
                            !used.contains(&def.id),
                            egui::Button::new(t!("assembly.panel.remove_def")),
                        )
                        .clicked()
                    {
                        action = Some(Action::RemoveDef(def.id.clone()));
                        ui.close();
                    }
                });
            });
        }
        ui.add_space(2.0);
        egui::CollapsingHeader::new(t!("assembly.panel.library"))
            .id_salt("assembly_node_library")
            .show(ui, |ui| {
                let entries: Vec<library::Entry> = view.rt.library().to_vec();
                let others: Vec<&library::Entry> = entries
                    .iter()
                    .filter(|e| defs::find(&embedded, &e.def.id).is_none())
                    .collect();
                if others.is_empty() {
                    ui.label(look.muted(t!("assembly.panel.library_empty")));
                }
                for e in others {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&e.def.name).size(12.5))
                            .on_hover_text(&e.def.description);
                        if ui.small_button(t!("assembly.panel.add_node")).clicked() {
                            action = Some(Action::Adopt(Box::new(e.clone())));
                        }
                        if ui
                            .small_button("✕")
                            .on_hover_text(t!("assembly.panel.delete_from_library"))
                            .clicked()
                        {
                            action = Some(Action::Delete(e.def.id.clone()));
                        }
                    });
                }
                ui.horizontal(|ui| {
                    if ui.small_button(t!("assembly.panel.import_node")).clicked() {
                        out.push(Command::AssemblyImportNode);
                    }
                    if ui
                        .small_button(t!("assembly.panel.reload_library"))
                        .clicked()
                    {
                        view.rt.reload_library();
                    }
                });
            });
    });
    if let Some(action) = action {
        run(action, state, view, &lib);
    }
}

enum Action {
    AddNode(DefId),
    Save(DefId),
    Take(DefId),
    Fork(DefId),
    RemoveDef(DefId),
    Adopt(Box<library::Entry>),
    Delete(DefId),
}

/// Where a node added from the panel goes: below the nodes there are.
fn free_pos(state: &AssemblyState, scope: &[NodeId]) -> [f32; 2] {
    access::graph_at(&state.design, scope)
        .map(|(g, _)| {
            let y = g.nodes.iter().map(|n| n.pos[1]).fold(0.0f32, f32::max);
            [0.0, y + 160.0]
        })
        .unwrap_or([0.0, 0.0])
}

fn run(
    action: Action,
    state: &mut AssemblyState,
    view: &mut NodeEditorView,
    lib: &library::NodeLibrary,
) {
    let rt = &mut view.rt;
    match action {
        Action::AddNode(id) => {
            let pos = free_pos(state, &rt.editor.scope);
            let kind = NodeKind::Group(crate::assembly::graph::kinds::GroupRef {
                def: id,
                inputs: Default::default(),
            });
            nodes::apply(
                state,
                rt,
                GraphEdit::Add {
                    kind: Box::new(kind),
                    pos,
                    wire: None,
                },
            );
        }
        Action::Save(id) => {
            let Some(def) = defs::find(&state.design.defs, &id).cloned() else {
                return;
            };
            match lib.save(&def, &state.design.defs) {
                Ok(stamp) => {
                    state.edit(|d| {
                        if let Some(def) = defs::find_mut(&mut d.defs, &id) {
                            def.revision = stamp.revision;
                            def.stamp = Some(stamp);
                        }
                    });
                    rt.report = Some((
                        t!("assembly.panel.saved_to_library", name = def.name),
                        false,
                    ));
                }
                Err(e) => rt.report = Some((e, true)),
            }
            rt.reload_library();
        }
        Action::Take(id) => {
            if let Some(entry) = rt.library().iter().find(|e| e.def.id == id).cloned() {
                state.edit(|d| {
                    library::adopt(&mut d.defs, &entry, true);
                    let mut defs = std::mem::take(&mut d.defs);
                    if let Some(g) = &mut d.graph {
                        ops::prune_all(g, &mut defs);
                    }
                    d.defs = defs;
                });
            }
        }
        Action::Fork(id) => {
            state.edit(|d| {
                if let Some(def) = defs::find(&d.defs, &id).cloned() {
                    let mut copy = def;
                    copy.id = DefId::fresh();
                    copy.name = t!("assembly.panel.copy_of", name = copy.name);
                    copy.stamp = None;
                    copy.revision = 0;
                    d.defs.push(copy);
                }
            });
        }
        Action::RemoveDef(id) => state.edit(|d| d.defs.retain(|def| def.id != id)),
        Action::Adopt(entry) => {
            let pos = free_pos(state, &rt.editor.scope);
            nodes::apply(state, rt, GraphEdit::AddFromLibrary(entry, pos));
        }
        Action::Delete(id) => {
            if let Err(e) = lib.remove(&id) {
                rt.report = Some((e, true));
            }
            rt.reload_library();
        }
    }
}
