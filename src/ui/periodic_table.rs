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
            Category::Nonmetal => "非金属",
            Category::NobleGas => "貴ガス",
            Category::Alkali => "アルカリ金属",
            Category::AlkalineEarth => "アルカリ土類金属",
            Category::Transition => "遷移金属",
            Category::PostTransition => "典型金属",
            Category::Metalloid => "半金属",
            Category::Halogen => "ハロゲン",
            Category::Lanthanide => "ランタノイド",
            Category::Actinide => "アクチノイド",
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

/// Japanese name, English name and standard atomic weight of each element, indexed by Z − 1.
/// Weights in brackets are the mass number of the longest-lived isotope.
const INFO: [(&str, &str, &str); 118] = [
    ("水素", "Hydrogen", "1.008"),
    ("ヘリウム", "Helium", "4.0026"),
    ("リチウム", "Lithium", "6.94"),
    ("ベリリウム", "Beryllium", "9.0122"),
    ("ホウ素", "Boron", "10.81"),
    ("炭素", "Carbon", "12.011"),
    ("窒素", "Nitrogen", "14.007"),
    ("酸素", "Oxygen", "15.999"),
    ("フッ素", "Fluorine", "18.998"),
    ("ネオン", "Neon", "20.180"),
    ("ナトリウム", "Sodium", "22.990"),
    ("マグネシウム", "Magnesium", "24.305"),
    ("アルミニウム", "Aluminium", "26.982"),
    ("ケイ素", "Silicon", "28.085"),
    ("リン", "Phosphorus", "30.974"),
    ("硫黄", "Sulfur", "32.06"),
    ("塩素", "Chlorine", "35.45"),
    ("アルゴン", "Argon", "39.95"),
    ("カリウム", "Potassium", "39.098"),
    ("カルシウム", "Calcium", "40.078"),
    ("スカンジウム", "Scandium", "44.956"),
    ("チタン", "Titanium", "47.867"),
    ("バナジウム", "Vanadium", "50.942"),
    ("クロム", "Chromium", "51.996"),
    ("マンガン", "Manganese", "54.938"),
    ("鉄", "Iron", "55.845"),
    ("コバルト", "Cobalt", "58.933"),
    ("ニッケル", "Nickel", "58.693"),
    ("銅", "Copper", "63.546"),
    ("亜鉛", "Zinc", "65.38"),
    ("ガリウム", "Gallium", "69.723"),
    ("ゲルマニウム", "Germanium", "72.630"),
    ("ヒ素", "Arsenic", "74.922"),
    ("セレン", "Selenium", "78.971"),
    ("臭素", "Bromine", "79.904"),
    ("クリプトン", "Krypton", "83.798"),
    ("ルビジウム", "Rubidium", "85.468"),
    ("ストロンチウム", "Strontium", "87.62"),
    ("イットリウム", "Yttrium", "88.906"),
    ("ジルコニウム", "Zirconium", "91.224"),
    ("ニオブ", "Niobium", "92.906"),
    ("モリブデン", "Molybdenum", "95.95"),
    ("テクネチウム", "Technetium", "[98]"),
    ("ルテニウム", "Ruthenium", "101.07"),
    ("ロジウム", "Rhodium", "102.91"),
    ("パラジウム", "Palladium", "106.42"),
    ("銀", "Silver", "107.87"),
    ("カドミウム", "Cadmium", "112.41"),
    ("インジウム", "Indium", "114.82"),
    ("スズ", "Tin", "118.71"),
    ("アンチモン", "Antimony", "121.76"),
    ("テルル", "Tellurium", "127.60"),
    ("ヨウ素", "Iodine", "126.90"),
    ("キセノン", "Xenon", "131.29"),
    ("セシウム", "Caesium", "132.91"),
    ("バリウム", "Barium", "137.33"),
    ("ランタン", "Lanthanum", "138.91"),
    ("セリウム", "Cerium", "140.12"),
    ("プラセオジム", "Praseodymium", "140.91"),
    ("ネオジム", "Neodymium", "144.24"),
    ("プロメチウム", "Promethium", "[145]"),
    ("サマリウム", "Samarium", "150.36"),
    ("ユウロピウム", "Europium", "151.96"),
    ("ガドリニウム", "Gadolinium", "157.25"),
    ("テルビウム", "Terbium", "158.93"),
    ("ジスプロシウム", "Dysprosium", "162.50"),
    ("ホルミウム", "Holmium", "164.93"),
    ("エルビウム", "Erbium", "167.26"),
    ("ツリウム", "Thulium", "168.93"),
    ("イッテルビウム", "Ytterbium", "173.05"),
    ("ルテチウム", "Lutetium", "174.97"),
    ("ハフニウム", "Hafnium", "178.49"),
    ("タンタル", "Tantalum", "180.95"),
    ("タングステン", "Tungsten", "183.84"),
    ("レニウム", "Rhenium", "186.21"),
    ("オスミウム", "Osmium", "190.23"),
    ("イリジウム", "Iridium", "192.22"),
    ("白金", "Platinum", "195.08"),
    ("金", "Gold", "196.97"),
    ("水銀", "Mercury", "200.59"),
    ("タリウム", "Thallium", "204.38"),
    ("鉛", "Lead", "207.2"),
    ("ビスマス", "Bismuth", "208.98"),
    ("ポロニウム", "Polonium", "[209]"),
    ("アスタチン", "Astatine", "[210]"),
    ("ラドン", "Radon", "[222]"),
    ("フランシウム", "Francium", "[223]"),
    ("ラジウム", "Radium", "[226]"),
    ("アクチニウム", "Actinium", "[227]"),
    ("トリウム", "Thorium", "232.04"),
    ("プロトアクチニウム", "Protactinium", "231.04"),
    ("ウラン", "Uranium", "238.03"),
    ("ネプツニウム", "Neptunium", "[237]"),
    ("プルトニウム", "Plutonium", "[244]"),
    ("アメリシウム", "Americium", "[243]"),
    ("キュリウム", "Curium", "[247]"),
    ("バークリウム", "Berkelium", "[247]"),
    ("カリホルニウム", "Californium", "[251]"),
    ("アインスタイニウム", "Einsteinium", "[252]"),
    ("フェルミウム", "Fermium", "[257]"),
    ("メンデレビウム", "Mendelevium", "[258]"),
    ("ノーベリウム", "Nobelium", "[259]"),
    ("ローレンシウム", "Lawrencium", "[262]"),
    ("ラザホージウム", "Rutherfordium", "[267]"),
    ("ドブニウム", "Dubnium", "[268]"),
    ("シーボーギウム", "Seaborgium", "[269]"),
    ("ボーリウム", "Bohrium", "[270]"),
    ("ハッシウム", "Hassium", "[269]"),
    ("マイトネリウム", "Meitnerium", "[278]"),
    ("ダームスタチウム", "Darmstadtium", "[281]"),
    ("レントゲニウム", "Roentgenium", "[282]"),
    ("コペルニシウム", "Copernicium", "[285]"),
    ("ニホニウム", "Nihonium", "[286]"),
    ("フレロビウム", "Flerovium", "[289]"),
    ("モスコビウム", "Moscovium", "[290]"),
    ("リバモリウム", "Livermorium", "[293]"),
    ("テネシン", "Tennessine", "[294]"),
    ("オガネソン", "Oganesson", "[294]"),
];

/// One element of the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Element {
    pub z: u16,
    pub symbol: &'static str,
    pub name_ja: &'static str,
    pub name_en: &'static str,
    pub weight: &'static str,
}

/// Element `z` (1–118).
pub fn element(z: u16) -> Option<Element> {
    let symbol = elements::symbol(z)?;
    let (name_ja, name_en, weight) = INFO[usize::from(z) - 1];
    Some(Element {
        z,
        symbol,
        name_ja,
        name_en,
        weight,
    })
}

/// Hover text of a cell.
pub fn tooltip(e: &Element) -> String {
    format!(
        "{}  {}（{}）\n原子番号 {}\n原子量 {}\n{}",
        e.symbol,
        e.name_ja,
        e.name_en,
        e.z,
        e.weight,
        category(e.z).label()
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
            assert!(!e.name_ja.is_empty() && !e.name_en.is_empty() && !e.weight.is_empty());
            assert!(symbols.insert(e.symbol), "duplicate {}", e.symbol);
            assert_eq!(elements::atomic_number(e.symbol), Some(z));
        }
        assert_eq!(symbols.len(), 118);
        assert!(element(0).is_none());
        assert!(element(119).is_none());
        assert_eq!(element(6).unwrap().name_ja, "炭素");
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
        assert!(t.contains("酸素") && t.contains("Oxygen") && t.contains('8'));
        assert!(t.contains("15.999"));
    }
}
