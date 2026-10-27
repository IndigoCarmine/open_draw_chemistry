//! Hydrogen bonds and π-stacking in the assembly view: found on each new build like the clash
//! check ([`super::place`]) — at once for a small assembly, on a worker thread for a large one —
//! drawn over the viewer ([`crate::ui::interactions`]) and listed in 情報. A crystal's
//! periodic box is honoured, so contacts across its faces count.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use super::super::interactions::{self as contacts, ShowInteractions};
use super::super::viewport3d::{Frame3d, Viewport3d};
use super::{AssemblyView, Ink};
use crate::assembly::{AssemblyState, Built};
use crate::three_d::interactions::{self, Interactions, Options};

/// Up to this many atoms the search runs on the UI thread (a few milliseconds).
const INLINE_ATOMS: usize = 8_000;

/// An assembly (its state id), a revision of it, its atom count, and what is searched for.
type Key = (u64, u64, usize, ShowInteractions);

/// The search's last answer and the one being worked out.
#[derive(Default)]
pub(super) struct ContactCheck {
    done: Option<(Key, Arc<Interactions>)>,
    running: Option<(Key, Receiver<Interactions>)>,
}

fn options(show: ShowInteractions) -> Options {
    Options {
        hbonds: show.hbonds,
        pi_stacking: show.pi_stacking,
        intramolecular: show.intramolecular,
        periodic: None,
    }
}

impl ContactCheck {
    /// The contacts of `built` (assembly `id`) that `show` asks for, the search started when
    /// they are new, and whether the answer is for this very build (otherwise it is the last one
    /// while this one is worked out). `None` until an answer for these atoms is ready, or when
    /// nothing is shown.
    pub(super) fn get(
        &mut self,
        ctx: &egui::Context,
        id: u64,
        built: &Arc<Built>,
        show: ShowInteractions,
    ) -> Option<(Arc<Interactions>, bool)> {
        if !show.any() {
            return None;
        }
        if let Some((key, rx)) = &self.running {
            match rx.try_recv() {
                Ok(result) => {
                    self.done = Some((*key, Arc::new(result)));
                    self.running = None;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => self.running = None,
            }
        }
        let key = (id, built.revision, built.atoms.len(), show);
        let current = self.done.as_ref().is_some_and(|(k, _)| *k == key);
        if !current && self.running.is_none() {
            if built.atoms.len() <= INLINE_ATOMS {
                let found = interactions::of_built(built, options(show));
                self.done = Some((key, Arc::new(found)));
            } else {
                let (tx, rx) = channel();
                let job = Arc::clone(built);
                let ctx = ctx.clone();
                std::thread::spawn(move || {
                    // The receiver may be gone by now: nothing to tell then.
                    let _ = tx.send(interactions::of_built(&job, options(show)));
                    ctx.request_repaint();
                });
                self.running = Some((key, rx));
            }
        }
        // An older answer still names the right atoms while their number is the same.
        self.done
            .as_ref()
            .filter(|((did, _, atoms, _), _)| *did == id && *atoms == key.2)
            .map(|(k, c)| (Arc::clone(c), *k == key))
    }
}

/// The contacts over the viewer.
pub(super) fn overlay(
    ui: &egui::Ui,
    viewport: &Viewport3d,
    frame: &Frame3d,
    built: &Built,
    found: &Interactions,
    show: ShowInteractions,
) {
    contacts::draw(
        &ui.painter_at(frame.rect),
        |p| viewport.project(frame.rect, p),
        |a| built.atoms.get(a).map(|a| a.pos),
        found,
        show,
    );
}

/// The 表示 section's toggles: the two kinds and whether contacts inside a molecule count.
pub(super) fn toggles(ui: &mut egui::Ui, show: &mut ShowInteractions) {
    contacts::toggles(ui, show);
    ui.add_enabled(
        show.any(),
        egui::Checkbox::new(&mut show.intramolecular, "分子内も含める"),
    )
    .on_hover_text("同じ分子の中の水素結合・π スタッキングも表示します（オフなら分子間だけ）");
}

/// The contacts in the 情報 section; clicking one selects the molecule it starts from.
pub(super) fn info(
    ui: &mut egui::Ui,
    state: &mut AssemblyState,
    view: &mut AssemblyView,
    ink: &Ink,
) {
    let show = view.interactions;
    if !show.any() {
        return;
    }
    let Ok(built) = state.built() else { return };
    let Some((found, current)) = view.contacts.get(ui.ctx(), state.id(), &built, show) else {
        ui.label(ink.muted("水素結合・π スタッキング：確認中…"));
        return;
    };
    let label = |a: usize| {
        let m = built.molecule_of_atom[a];
        format!(
            "分子 {} {}{}",
            m + 1,
            built.atoms[a].element,
            a - built.molecule_start[m] + 1
        )
    };
    let pending = if current { "" } else { "（更新中…）" };
    let clicked = contacts::list(ui, &found, show, pending, "クリックでその分子を選択", label);
    let atom = match clicked {
        Some(contacts::Clicked::HBond(k)) => found.hbonds.get(k).map(|hb| hb.hydrogen),
        Some(contacts::Clicked::Stack(k)) => found
            .stacks
            .get(k)
            .and_then(|s| found.rings.get(s.a))
            .and_then(|r| r.atoms.first().copied()),
        None => None,
    };
    if let Some(a) = atom {
        state.selected = built.molecule_of_atom.get(a).copied();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assembly::math::{self, Rigid};
    use crate::assembly::{Component, Design, Instance};
    use crate::theme::dark;
    use crate::three_d::{Atom3d, Bond3d};

    fn water() -> Component {
        let a = 104.5f64.to_radians();
        let atom = |e: &str, pos| Atom3d {
            element: e.into(),
            pos,
        };
        let bond = |end| Bond3d {
            begin: 0,
            end,
            order: 1,
        };
        Component {
            name: "water".into(),
            atoms: vec![
                atom("O", [0.0; 3]),
                atom("H", [0.96, 0.0, 0.0]),
                atom("H", [0.96 * a.cos(), 0.96 * a.sin(), 0.0]),
            ],
            bonds: vec![bond(1), bond(2)],
            orient: None,
        }
    }

    /// Two hand-placed waters, the first's H pointing at the second's O 2.9 Å away.
    fn dimer() -> AssemblyState {
        let at = [20.0, 0.0, 0.0];
        AssemblyState::with_design(Design {
            components: vec![water()],
            manual: vec![
                Instance {
                    component: 0,
                    transform: Rigid::translation(at),
                },
                Instance {
                    component: 0,
                    transform: Rigid::rotation(math::rot_z(-52.25))
                        .then(&Rigid::translation(math::add(at, [2.9, 0.0, 0.0]))),
                },
            ],
            ..Default::default()
        })
    }

    #[test]
    fn a_hand_placed_water_dimer_shows_its_hydrogen_bond_and_the_toggles_work() {
        let ctx = egui::Context::default();
        let mut state = dimer();
        let mut view = AssemblyView::default();
        let built = state.built().unwrap();
        let show = ShowInteractions::default();
        let (found, current) = view.contacts.get(&ctx, state.id(), &built, show).unwrap();
        assert!(current);
        assert_eq!(found.hbonds.len(), 1, "{:?}", found.hbonds);
        assert!((found.hbonds[0].distance - 1.94).abs() < 1e-6);
        // Between molecules, so still there with intramolecular contacts off.
        let inter = ShowInteractions {
            intramolecular: false,
            ..show
        };
        let (found, _) = view.contacts.get(&ctx, state.id(), &built, inter).unwrap();
        assert_eq!(found.hbonds.len(), 1);
        // Both kinds off: nothing searched, nothing shown.
        let off = ShowInteractions {
            hbonds: false,
            pi_stacking: false,
            ..show
        };
        assert!(view.contacts.get(&ctx, state.id(), &built, off).is_none());

        // The panel's toggles and list draw headless.
        let ink = Ink {
            muted: dark::MUTED,
            warn: dark::MUTED,
        };
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            toggles(ui, &mut view.interactions);
            info(ui, &mut state, &mut view, &ink);
        });
        output.textures_delta.clear();
    }
}
