//! The periodic table picker behind the palette's 周期表… button: all 118 elements in the
//! standard 18-column layout, with the lanthanides and actinides in two rows below.
//!
//! Symbols come from [`chembuider_rs::molecule::elements`] so the picker and the file formats
//! agree on them; the names and weights shown in tooltips are kept here.

use chembuider_rs::molecule::elements;
use egui::{Color32, Rect, Sense, Stroke, StrokeKind, pos2, vec2};

/// Highest atomic number in the table (oganesson).
pub const LAST_Z: u16 = 118;
/// Columns (groups) of the table.
pub const COLUMNS: usize = 18;
/// Rows of the table: periods 1–7, a spacer, then the lanthanide and actinide rows.
pub const ROWS: usize = 10;
const LANTHANIDE_ROW: usize = 8;
const ACTINIDE_ROW: usize = 9;

/// Where the lanthanides and actinides would sit in the main body (group 3, periods 6 and 7):
/// cells that show the range moved below instead of an element.
pub const GAP_MARKERS: [(usize, usize, &str); 2] = [(5, 2, "57–71"), (6, 2, "89–103")];

/// Family of an element, for the cell's colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Nonmetal,
    NobleGas,
    Alkali,
    AlkalineEarth,
    Transition,
    PostTransition,
    Metalloid,
    Halogen,
    Lanthanide,
    Actinide,
}

impl Category {
    pub fn label(self) -> &'static str {
        match self {
            Category::Nonmetal => t!("periodic_table.category.nonmetal"),
            Category::NobleGas => t!("periodic_table.category.noble_gas"),
            Category::Alkali => t!("periodic_table.category.alkali"),
            Category::AlkalineEarth => t!("periodic_table.category.alkaline_earth"),
            Category::Transition => t!("periodic_table.category.transition"),
            Category::PostTransition => t!("periodic_table.category.post_transition"),
            Category::Metalloid => t!("periodic_table.category.metalloid"),
            Category::Halogen => t!("periodic_table.category.halogen"),
            Category::Lanthanide => t!("periodic_table.category.lanthanide"),
            Category::Actinide => t!("periodic_table.category.actinide"),
        }
    }

    /// The hue the cell is tinted towards.
    fn hue(self) -> Color32 {
        match self {
            Category::Nonmetal => Color32::from_rgb(0x4C, 0xA8, 0x6A),
            Category::NobleGas => Color32::from_rgb(0x8A, 0x6F, 0xC8),
            Category::Alkali => Color32::from_rgb(0xD9, 0x5A, 0x4E),
            Category::AlkalineEarth => Color32::from_rgb(0xE0, 0x95, 0x3C),
            Category::Transition => Color32::from_rgb(0xC9, 0xA8, 0x4A),
            Category::PostTransition => Color32::from_rgb(0x7E, 0x9A, 0xA8),
            Category::Metalloid => Color32::from_rgb(0x5E, 0xA8, 0xA0),
            Category::Halogen => Color32::from_rgb(0x4F, 0x8C, 0xD0),
            Category::Lanthanide => Color32::from_rgb(0xC8, 0x6A, 0x9A),
            Category::Actinide => Color32::from_rgb(0xA8, 0x6A, 0xC0),
        }
    }

    /// A muted tint of the category's hue over the window surface of the current theme.
    pub fn fill(self, dark_mode: bool) -> Color32 {
        let (base, t) = if dark_mode {
            (crate::theme::dark::RAISED, 0.22)
        } else {
            (Color32::WHITE, 0.16)
        };
        base.lerp_to_gamma(self.hue(), t)
    }
}

/// Category of the element with atomic number `z` (1–118).
pub fn category(z: u16) -> Category {
    match z {
        2 | 10 | 18 | 36 | 54 | 86 | 118 => Category::NobleGas,
        3 | 11 | 19 | 37 | 55 | 87 => Category::Alkali,
        4 | 12 | 20 | 38 | 56 | 88 => Category::AlkalineEarth,
        9 | 17 | 35 | 53 | 85 | 117 => Category::Halogen,
        5 | 14 | 32 | 33 | 51 | 52 => Category::Metalloid,
        1 | 6 | 7 | 8 | 15 | 16 | 34 => Category::Nonmetal,
        57..=71 => Category::Lanthanide,
        89..=103 => Category::Actinide,
        21..=30 | 39..=48 | 72..=80 | 104..=112 => Category::Transition,
        _ => Category::PostTransition,
    }
}

/// (row, column) of element `z` in the table, both 0-based; `None` outside 1–118.
pub fn position(z: u16) -> Option<(usize, usize)> {
    let z = usize::from(z);
    Some(match z {
        1 => (0, 0),
        2 => (0, 17),
        3..=4 => (1, z - 3),
        5..=10 => (1, z + 7),
        11..=12 => (2, z - 11),
        13..=18 => (2, z - 1),
        19..=36 => (3, z - 19),
        37..=54 => (4, z - 37),
        55..=56 => (5, z - 55),
        57..=71 => (LANTHANIDE_ROW, z - 54),
        72..=86 => (5, z - 69),
        87..=88 => (6, z - 87),
        89..=103 => (ACTINIDE_ROW, z - 86),
        104..=118 => (6, z - 101),
        _ => return None,
    })
}

/// English (IUPAC) name and standard atomic weight of each element, indexed by Z − 1. The
/// English name is data (shown next to the translated name in other languages); the name in
/// the UI language comes from [`name`]. Weights in brackets are the mass number of the
/// longest-lived isotope.
const INFO: [(&str, &str); 118] = [
    ("Hydrogen", "1.008"),
    ("Helium", "4.0026"),
    ("Lithium", "6.94"),
    ("Beryllium", "9.0122"),
    ("Boron", "10.81"),
    ("Carbon", "12.011"),
    ("Nitrogen", "14.007"),
    ("Oxygen", "15.999"),
    ("Fluorine", "18.998"),
    ("Neon", "20.180"),
    ("Sodium", "22.990"),
    ("Magnesium", "24.305"),
    ("Aluminium", "26.982"),
    ("Silicon", "28.085"),
    ("Phosphorus", "30.974"),
    ("Sulfur", "32.06"),
    ("Chlorine", "35.45"),
    ("Argon", "39.95"),
    ("Potassium", "39.098"),
    ("Calcium", "40.078"),
    ("Scandium", "44.956"),
    ("Titanium", "47.867"),
    ("Vanadium", "50.942"),
    ("Chromium", "51.996"),
    ("Manganese", "54.938"),
    ("Iron", "55.845"),
    ("Cobalt", "58.933"),
    ("Nickel", "58.693"),
    ("Copper", "63.546"),
    ("Zinc", "65.38"),
    ("Gallium", "69.723"),
    ("Germanium", "72.630"),
    ("Arsenic", "74.922"),
    ("Selenium", "78.971"),
    ("Bromine", "79.904"),
    ("Krypton", "83.798"),
    ("Rubidium", "85.468"),
    ("Strontium", "87.62"),
    ("Yttrium", "88.906"),
    ("Zirconium", "91.224"),
    ("Niobium", "92.906"),
    ("Molybdenum", "95.95"),
    ("Technetium", "[98]"),
    ("Ruthenium", "101.07"),
    ("Rhodium", "102.91"),
    ("Palladium", "106.42"),
    ("Silver", "107.87"),
    ("Cadmium", "112.41"),
    ("Indium", "114.82"),
    ("Tin", "118.71"),
    ("Antimony", "121.76"),
    ("Tellurium", "127.60"),
    ("Iodine", "126.90"),
    ("Xenon", "131.29"),
    ("Caesium", "132.91"),
    ("Barium", "137.33"),
    ("Lanthanum", "138.91"),
    ("Cerium", "140.12"),
    ("Praseodymium", "140.91"),
    ("Neodymium", "144.24"),
    ("Promethium", "[145]"),
    ("Samarium", "150.36"),
    ("Europium", "151.96"),
    ("Gadolinium", "157.25"),
    ("Terbium", "158.93"),
    ("Dysprosium", "162.50"),
    ("Holmium", "164.93"),
    ("Erbium", "167.26"),
    ("Thulium", "168.93"),
    ("Ytterbium", "173.05"),
    ("Lutetium", "174.97"),
    ("Hafnium", "178.49"),
    ("Tantalum", "180.95"),
    ("Tungsten", "183.84"),
    ("Rhenium", "186.21"),
    ("Osmium", "190.23"),
    ("Iridium", "192.22"),
    ("Platinum", "195.08"),
    ("Gold", "196.97"),
    ("Mercury", "200.59"),
    ("Thallium", "204.38"),
    ("Lead", "207.2"),
    ("Bismuth", "208.98"),
    ("Polonium", "[209]"),
    ("Astatine", "[210]"),
    ("Radon", "[222]"),
    ("Francium", "[223]"),
    ("Radium", "[226]"),
    ("Actinium", "[227]"),
    ("Thorium", "232.04"),
    ("Protactinium", "231.04"),
    ("Uranium", "238.03"),
    ("Neptunium", "[237]"),
    ("Plutonium", "[244]"),
    ("Americium", "[243]"),
    ("Curium", "[247]"),
    ("Berkelium", "[247]"),
    ("Californium", "[251]"),
    ("Einsteinium", "[252]"),
    ("Fermium", "[257]"),
    ("Mendelevium", "[258]"),
    ("Nobelium", "[259]"),
    ("Lawrencium", "[262]"),
    ("Rutherfordium", "[267]"),
    ("Dubnium", "[268]"),
    ("Seaborgium", "[269]"),
    ("Bohrium", "[270]"),
    ("Hassium", "[269]"),
    ("Meitnerium", "[278]"),
    ("Darmstadtium", "[281]"),
    ("Roentgenium", "[282]"),
    ("Copernicium", "[285]"),
    ("Nihonium", "[286]"),
    ("Flerovium", "[289]"),
    ("Moscovium", "[290]"),
    ("Livermorium", "[293]"),
    ("Tennessine", "[294]"),
    ("Oganesson", "[294]"),
];

/// Name of element `z` in the UI language (`None` outside 1–118).
pub fn name(z: u16) -> Option<&'static str> {
    Some(match z {
        1 => t!("periodic_table.element.h"),
        2 => t!("periodic_table.element.he"),
        3 => t!("periodic_table.element.li"),
        4 => t!("periodic_table.element.be"),
        5 => t!("periodic_table.element.b"),
        6 => t!("periodic_table.element.c"),
        7 => t!("periodic_table.element.n"),
        8 => t!("periodic_table.element.o"),
        9 => t!("periodic_table.element.f"),
        10 => t!("periodic_table.element.ne"),
        11 => t!("periodic_table.element.na"),
        12 => t!("periodic_table.element.mg"),
        13 => t!("periodic_table.element.al"),
        14 => t!("periodic_table.element.si"),
        15 => t!("periodic_table.element.p"),
        16 => t!("periodic_table.element.s"),
        17 => t!("periodic_table.element.cl"),
        18 => t!("periodic_table.element.ar"),
        19 => t!("periodic_table.element.k"),
        20 => t!("periodic_table.element.ca"),
        21 => t!("periodic_table.element.sc"),
        22 => t!("periodic_table.element.ti"),
        23 => t!("periodic_table.element.v"),
        24 => t!("periodic_table.element.cr"),
        25 => t!("periodic_table.element.mn"),
        26 => t!("periodic_table.element.fe"),
        27 => t!("periodic_table.element.co"),
        28 => t!("periodic_table.element.ni"),
        29 => t!("periodic_table.element.cu"),
        30 => t!("periodic_table.element.zn"),
        31 => t!("periodic_table.element.ga"),
        32 => t!("periodic_table.element.ge"),
        33 => t!("periodic_table.element.as"),
        34 => t!("periodic_table.element.se"),
        35 => t!("periodic_table.element.br"),
        36 => t!("periodic_table.element.kr"),
        37 => t!("periodic_table.element.rb"),
        38 => t!("periodic_table.element.sr"),
        39 => t!("periodic_table.element.y"),
        40 => t!("periodic_table.element.zr"),
        41 => t!("periodic_table.element.nb"),
        42 => t!("periodic_table.element.mo"),
        43 => t!("periodic_table.element.tc"),
        44 => t!("periodic_table.element.ru"),
        45 => t!("periodic_table.element.rh"),
        46 => t!("periodic_table.element.pd"),
        47 => t!("periodic_table.element.ag"),
        48 => t!("periodic_table.element.cd"),
        49 => t!("periodic_table.element.in"),
        50 => t!("periodic_table.element.sn"),
        51 => t!("periodic_table.element.sb"),
        52 => t!("periodic_table.element.te"),
        53 => t!("periodic_table.element.i"),
        54 => t!("periodic_table.element.xe"),
        55 => t!("periodic_table.element.cs"),
        56 => t!("periodic_table.element.ba"),
        57 => t!("periodic_table.element.la"),
        58 => t!("periodic_table.element.ce"),
        59 => t!("periodic_table.element.pr"),
        60 => t!("periodic_table.element.nd"),
        61 => t!("periodic_table.element.pm"),
        62 => t!("periodic_table.element.sm"),
        63 => t!("periodic_table.element.eu"),
        64 => t!("periodic_table.element.gd"),
        65 => t!("periodic_table.element.tb"),
        66 => t!("periodic_table.element.dy"),
        67 => t!("periodic_table.element.ho"),
        68 => t!("periodic_table.element.er"),
        69 => t!("periodic_table.element.tm"),
        70 => t!("periodic_table.element.yb"),
        71 => t!("periodic_table.element.lu"),
        72 => t!("periodic_table.element.hf"),
        73 => t!("periodic_table.element.ta"),
        74 => t!("periodic_table.element.w"),
        75 => t!("periodic_table.element.re"),
        76 => t!("periodic_table.element.os"),
        77 => t!("periodic_table.element.ir"),
        78 => t!("periodic_table.element.pt"),
        79 => t!("periodic_table.element.au"),
        80 => t!("periodic_table.element.hg"),
        81 => t!("periodic_table.element.tl"),
        82 => t!("periodic_table.element.pb"),
        83 => t!("periodic_table.element.bi"),
        84 => t!("periodic_table.element.po"),
        85 => t!("periodic_table.element.at"),
        86 => t!("periodic_table.element.rn"),
        87 => t!("periodic_table.element.fr"),
        88 => t!("periodic_table.element.ra"),
        89 => t!("periodic_table.element.ac"),
        90 => t!("periodic_table.element.th"),
        91 => t!("periodic_table.element.pa"),
        92 => t!("periodic_table.element.u"),
        93 => t!("periodic_table.element.np"),
        94 => t!("periodic_table.element.pu"),
        95 => t!("periodic_table.element.am"),
        96 => t!("periodic_table.element.cm"),
        97 => t!("periodic_table.element.bk"),
        98 => t!("periodic_table.element.cf"),
        99 => t!("periodic_table.element.es"),
        100 => t!("periodic_table.element.fm"),
        101 => t!("periodic_table.element.md"),
        102 => t!("periodic_table.element.no"),
        103 => t!("periodic_table.element.lr"),
        104 => t!("periodic_table.element.rf"),
        105 => t!("periodic_table.element.db"),
        106 => t!("periodic_table.element.sg"),
        107 => t!("periodic_table.element.bh"),
        108 => t!("periodic_table.element.hs"),
        109 => t!("periodic_table.element.mt"),
        110 => t!("periodic_table.element.ds"),
        111 => t!("periodic_table.element.rg"),
        112 => t!("periodic_table.element.cn"),
        113 => t!("periodic_table.element.nh"),
        114 => t!("periodic_table.element.fl"),
        115 => t!("periodic_table.element.mc"),
        116 => t!("periodic_table.element.lv"),
        117 => t!("periodic_table.element.ts"),
        118 => t!("periodic_table.element.og"),
        _ => return None,
    })
}

/// One element of the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Element {
    pub z: u16,
    pub symbol: &'static str,
    /// Name in the UI language.
    pub name: &'static str,
    /// English (IUPAC) name.
    pub name_en: &'static str,
    pub weight: &'static str,
}

/// Element `z` (1–118).
pub fn element(z: u16) -> Option<Element> {
    let symbol = elements::symbol(z)?;
    let (name_en, weight) = INFO[usize::from(z) - 1];
    Some(Element {
        z,
        symbol,
        name: name(z)?,
        name_en,
        weight,
    })
}

/// Hover text of a cell.
pub fn tooltip(e: &Element) -> String {
    // In languages other than English, the English name follows the translated one.
    let name = if e.name == e.name_en {
        e.name.to_owned()
    } else {
        t!(
            "periodic_table.name_with_english",
            name = e.name,
            english = e.name_en
        )
    };
    t!(
        "periodic_table.tooltip",
        symbol = e.symbol,
        name = name,
        z = e.z,
        weight = e.weight,
        category = category(e.z).label()
    )
}

const CELL: egui::Vec2 = vec2(32.0, 36.0);
const GAP: f32 = 2.0;
/// Height of the spacer row between period 7 and the lanthanides.
const SPACER: f32 = 8.0;

/// Draws the table; returns the symbol of the element clicked this frame. `current` (the
/// editor's element) is outlined in the accent colour.
pub fn table(ui: &mut egui::Ui, current: &str) -> Option<&'static str> {
    let width = COLUMNS as f32 * (CELL.x + GAP) - GAP;
    let height = 9.0 * (CELL.y + GAP) + SPACER;
    let (area, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let cell_rect = |row: usize, col: usize| {
        let mut y = row as f32 * (CELL.y + GAP);
        if row >= LANTHANIDE_ROW {
            y += SPACER - (CELL.y + GAP);
        }
        Rect::from_min_size(area.min + vec2(col as f32 * (CELL.x + GAP), y), CELL)
    };

    let dark = ui.visuals().dark_mode;
    let accent = crate::theme::accent(dark);
    let border = ui.visuals().widgets.noninteractive.bg_stroke;
    let text = ui.visuals().text_color();
    let muted = crate::theme::muted(dark);
    let radius = 4.0;
    let painter = ui.painter().clone();

    for (row, col, label) in GAP_MARKERS {
        let rect = cell_rect(row, col);
        painter.rect_stroke(
            rect,
            radius,
            Stroke::new(1.0, border.color),
            StrokeKind::Inside,
        );
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(9.0),
            muted,
        );
    }

    let mut clicked = None;
    for z in 1..=LAST_Z {
        let (Some(e), Some((row, col))) = (element(z), position(z)) else {
            continue;
        };
        let rect = cell_rect(row, col);
        let id = ui.scope_id().with(("periodic_table_cell", z));
        let response = ui
            .interact(rect, id, Sense::click())
            .on_hover_text(tooltip(&e));
        let selected = e.symbol == current;

        let mut fill = category(z).fill(dark);
        if response.hovered() {
            fill = fill.lerp_to_gamma(accent, 0.12);
        }
        let stroke = if selected {
            Stroke::new(2.0, accent)
        } else if response.hovered() {
            Stroke::new(1.0, accent)
        } else {
            Stroke::new(1.0, border.color)
        };
        painter.rect(rect, radius, fill, stroke, StrokeKind::Inside);
        painter.text(
            pos2(rect.left() + 3.0, rect.top() + 2.0),
            egui::Align2::LEFT_TOP,
            z.to_string(),
            egui::FontId::proportional(8.0),
            muted,
        );
        painter.text(
            rect.center() + vec2(0.0, 4.0),
            egui::Align2::CENTER_CENTER,
            e.symbol,
            egui::FontId::proportional(14.0),
            text,
        );
        if response.clicked() {
            clicked = Some(e.symbol);
        }
    }
    clicked
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn all_118_elements_have_data_and_unique_symbols() {
        let mut symbols = HashSet::new();
        for z in 1..=LAST_Z {
            let e = element(z).expect("element");
            assert_eq!(e.z, z);
            assert!(!e.name.is_empty() && !e.name_en.is_empty() && !e.weight.is_empty());
            assert_eq!(
                e.name, e.name_en,
                "the English catalog uses the IUPAC names"
            );
            assert!(symbols.insert(e.symbol), "duplicate {}", e.symbol);
            assert_eq!(elements::atomic_number(e.symbol), Some(z));
        }
        assert_eq!(symbols.len(), 118);
        assert!(element(0).is_none());
        assert!(element(119).is_none());
        assert_eq!(element(6).unwrap().name, "Carbon");
        let _ja = crate::i18n::ScopedLanguage::new(crate::i18n::Lang::Ja);
        assert_eq!(element(6).unwrap().name, "炭素");
        assert_eq!(element(118).unwrap().symbol, "Og");
    }

    #[test]
    fn positions_are_unique_and_inside_the_grid() {
        let markers: HashSet<_> = GAP_MARKERS.iter().map(|(r, c, _)| (*r, *c)).collect();
        let mut seen = HashSet::new();
        for z in 1..=LAST_Z {
            let (row, col) = position(z).expect("position");
            assert!(
                row < ROWS && row != 7 && col < COLUMNS,
                "{z}: ({row}, {col})"
            );
            assert!(seen.insert((row, col)), "{z} overlaps");
            assert!(!markers.contains(&(row, col)), "{z} on a gap marker");
        }
        assert!(position(0).is_none() && position(119).is_none());
    }

    #[test]
    fn atomic_numbers_increase_along_each_row() {
        for row in 0..ROWS {
            let mut zs: Vec<_> = (1..=LAST_Z)
                .filter_map(|z| position(z).filter(|p| p.0 == row).map(|p| (p.1, z)))
                .collect();
            zs.sort();
            assert!(zs.windows(2).all(|w| w[0].1 < w[1].1), "row {row}");
        }
    }

    #[test]
    fn well_known_positions_and_categories() {
        assert_eq!(position(1), Some((0, 0)));
        assert_eq!(position(2), Some((0, 17)));
        assert_eq!(position(6), Some((1, 13)));
        assert_eq!(position(26), Some((3, 7)));
        assert_eq!(position(72), Some((5, 3)));
        assert_eq!(position(118), Some((6, 17)));
        assert_eq!(position(57), Some((LANTHANIDE_ROW, 3)));
        assert_eq!(position(103), Some((ACTINIDE_ROW, 17)));
        assert_eq!(category(6), Category::Nonmetal);
        assert_eq!(category(11), Category::Alkali);
        assert_eq!(category(26), Category::Transition);
        assert_eq!(category(17), Category::Halogen);
        assert_eq!(category(14), Category::Metalloid);
        assert_eq!(category(82), Category::PostTransition);
        assert_eq!(category(60), Category::Lanthanide);
        assert_eq!(category(92), Category::Actinide);
        assert_eq!(category(54), Category::NobleGas);
    }

    #[test]
    fn tooltip_names_the_element() {
        let t = tooltip(&element(8).unwrap());
        assert!(t.contains("Oxygen") && t.contains('8') && t.contains("15.999"));
        assert!(!t.contains("Oxygen (Oxygen)"), "{t}");
        let _ja = crate::i18n::ScopedLanguage::new(crate::i18n::Lang::Ja);
        let t = tooltip(&element(8).unwrap());
        assert!(t.contains("酸素") && t.contains("Oxygen") && t.contains("15.999"));
    }
}
