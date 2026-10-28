//! The SMILES row under the 解析 panel: label, the string (monospace, wrapping) and a copy
//! button, styled like the library panel's rows.
//!
//! The SMILES is computed only when the analysed structures change, on a background thread
//! (OpenBabel can take a moment on a big drawing). The state lives in egui's memory, so the
//! caller only passes what it draws anyway.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use chembuider_rs::{Config, Molecule};

use crate::smiles;

/// Atoms above which the SMILES is not computed: the panel would wait too long on it.
const MAX_ATOMS: usize = 999;

#[derive(Default)]
struct State {
    /// The structures the result (or the running work) is for.
    source: Option<Molecule>,
    /// `None` while the work runs.
    result: Option<Result<String, String>>,
    /// Bumped on every new request, so a stale thread's answer is dropped.
    generation: u64,
    copied: Option<String>,
}

type Shared = Arc<Mutex<State>>;

/// Draws the SMILES row for the structures of `mol` the analysis panel shows.
pub fn show(ui: &mut egui::Ui, mol: &Molecule, selection: &HashSet<u32>, config: &Config) {
    let target = smiles::target(mol, selection);
    if target.atoms.is_empty() {
        return;
    }
    let id = egui::Id::unique("analysis_smiles_row");
    let shared: Shared = ui
        .ctx()
        .data_mut(|d| d.get_temp_mut_or_default::<Shared>(id).clone());
    let Ok(mut state) = shared.lock() else {
        return;
    };
    if state.source.as_ref() != Some(&target) {
        state.generation += 1;
        state.copied = None;
        state.source = Some(target.clone());
        if target.atoms.len() > MAX_ATOMS {
            state.result = Some(Err(format!(
                "構造が大きすぎます（原子 {MAX_ATOMS} 個まで）"
            )));
        } else {
            state.result = None;
            let generation = state.generation;
            let shared = shared.clone();
            let config = config.clone();
            let ctx = ui.ctx().clone();
            std::thread::spawn(move || {
                let result = smiles::compute(&target, &config);
                if let Ok(mut state) = shared.lock()
                    && state.generation == generation
                {
                    state.result = Some(result);
                    ctx.request_repaint();
                }
            });
        }
    }

    let result = state.result.clone();
    let copied = state.copied.clone();
    let mut clicked = None;
    ui.add_space(4.0);
    egui::Grid::new("analysis_smiles")
        .num_columns(2)
        .show(ui, |ui| {
            ui.label("SMILES");
            ui.vertical(|ui| match &result {
                None => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.weak("計算中…");
                    });
                }
                Some(Ok(smiles)) => {
                    let text = smiles.clone();
                    ui.horizontal_wrapped(|ui| {
                        ui.add(egui::Label::new(egui::RichText::new(&text).monospace()).wrap());
                        let copy = ui.small_button("📋").on_hover_text("SMILES をコピー");
                        if copy.clicked() {
                            ui.ctx().copy_text(text.clone());
                            clicked = Some(text.clone());
                        }
                    });
                    if let Some(copied) = &copied {
                        ui.weak(format!("コピーしました: {copied}"));
                    }
                }
                Some(Err(reason)) if reason == smiles::NEEDS_OPENBABEL => {
                    ui.weak(reason);
                }
                Some(Err(reason)) => {
                    ui.weak(format!("— (未対応: {reason})"));
                }
            });
            ui.end_row();
        });
    if clicked.is_some() {
        state.copied = clicked;
    }
}
