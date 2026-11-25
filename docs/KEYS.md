# Keyboard and mouse reference

[日本語](KEYS_ja.md)

Every keyboard operation, and every mouse operation that depends on a modifier key, in every screen of Open DrawChemistry.

- Covers the app at commit `1822865` and the 2D editor (chembuider-rs) at rev `5d46290`.
- This reference was written from the input handling in the source code. It was not checked item by item in the running app. What could not be settled is listed in [9. Not verified](#9-not-verified).
- Keys are written as on Windows and Linux. For macOS see [7. Differences on macOS](#7-differences-on-macos).
- "Drag" and "click" mean the left button unless stated otherwise.

## Contents

1. [Quick reference](#1-quick-reference)
2. [2D editor](#2-2d-editor)
3. [3D view](#3-3d-view)
4. [Assembly view](#4-assembly-view)
5. [Windows and panels](#5-windows-and-panels)
6. [Common to the whole app](#6-common-to-the-whole-app)
7. [Differences on macOS](#7-differences-on-macos)
8. [Where the controls get complicated](#8-where-the-controls-get-complicated)
9. [Not verified](#9-not-verified)

---

## 1. Quick reference

Only the most used operations. The conditions are in the sections below.

### In every screen

| Operation | Effect |
| --- | --- |
| `Ctrl`+`N` / `Ctrl`+`O` / `Ctrl`+`S` / `Ctrl`+`Shift`+`S` / `Ctrl`+`W` | New / Open / Save / Save As / Close tab |
| `Ctrl`+`Z` / `Ctrl`+`Y` (or `Ctrl`+`Shift`+`Z`) | Undo / Redo (the edits of the screen shown) |
| `Ctrl`+`+` / `Ctrl`+`-` / `Ctrl`+`0` | Make the whole UI larger / smaller / back to 100% |

### 2D editor

| Operation | Effect |
| --- | --- |
| Wheel | Zoom (about the pointer) |
| Middle-button drag | Move the view |
| `Ctrl`+`0` | Fit the whole drawing (the UI scale also goes back to 100%) |
| `Ctrl`+`1` / `2` / `3` | Select tool / Bond tool / Eraser |
| `Ctrl`+`C` / `Ctrl`+`V` | Copy / Paste |
| `Ctrl`+`K` | Start or stop Clean Up |
| `Delete` | Bond tool: delete the atom or bond under the pointer. Select tool: delete the selection |
| A letter key over an atom (Bond tool) | `o` OH, `n` NH₂, `a` or `3` benzene ring, `6` six-membered ring, `1` one more carbon, and so on ([table](#atom-keys)) |
| `1` / `2` / `3` over a bond (Bond tool) | Single / double / triple bond |
| `` ` `` over an atom (Bond tool) | Open the label box. Type a fragment name, then `x` expands it into the structure |
| Right-click a bond | Menu of bond types |
| Drag with the Select tool | From an atom: move. From empty space: lasso |
| Drag the round handle above the selection (Select tool) | Rotate the selection in 15° steps (`Shift`: any angle) |
| `Ctrl`+`R` | Rotate the selection by a typed angle |
| `Ctrl`+`Shift`+`F` | Flip the selection over the one bond that leaves it |

### 3D view and assembly view

| Operation | Effect |
| --- | --- |
| Drag | Orbit the view |
| `Shift`+drag, right drag, middle drag | Pan the view |
| Wheel | Zoom |
| Double-click empty space | Reset the view |
| `Esc` | Clear the selection |
| 3D: click an atom | Pick it for measuring and editing (up to four) |
| 3D: `Ctrl`+click, `Ctrl`+drag | Select atoms for deletion. `Delete` removes them |
| 3D: right-click an atom | Fix that atom / release it |
| 3D: `Shift`+drag / `Alt`+drag from a molecule | Move / rotate that molecule (when the model has two or more molecules) |
| Assembly: click an atom | Select its molecule |
| Assembly: `Shift`+drag / `Alt`+drag | Move / rotate the selected hand-placed molecule |
| Assembly: `M` | Measuring mode on / off |

---

## 2. 2D editor

The drawing canvas. The same operation means different things depending on the tool chosen in the palette on the left. There are six tools: the Bond tool (the single, double, triple, wedge, hashed wedge, wavy and dative buttons are all the Bond tool, differing only in the bond they draw), the Select tool, the Eraser, the Arrow tool (reaction arrow, equilibrium arrow), the Curly arrow tool (curly arrow, fish-hook arrow) and the Text tool.

### 2.1 Terms

| Term | Meaning |
| --- | --- |
| Hover | The atom, bond or object (arrow, text, bracket, table) under the mouse pointer. An atom wins over an object, an object over a bond. |
| Hotspot | The atom the keyboard acts on. Placing or extending atoms by click or by key moves it to the new atom. |
| Bond target | The bond the keyboard acts on. Set with `Shift`+arrow in the Bond tool. |
| Selection | The atoms, bonds and objects chosen by clicking or lassoing with the Select tool. |

### 2.2 Operations that work in every tool

#### Mouse

| Operation | Condition | Effect |
| --- | --- | --- |
| Wheel | Pointer over the canvas | Zoom (about the pointer) |
| `Ctrl`+wheel, pinch | Pointer over the canvas | Zoom |
| Middle-button drag | No input box open, Clean Up not running | Move the view |
| Right-click | On a bond | Menu: bond type, double-bond position, delete; Make 3D; format (colours, shadow) |
| Right-click | Elsewhere | Menu: Make 3D and format. It acts on the selected molecules if any, else the molecule clicked, else the whole drawing |

#### Keys

The "While typing" column says what happens while the cursor is in the label, text, table-cell or angle box, or in a text field of another panel.

| Key | Effect | While typing |
| --- | --- | --- |
| `Ctrl`+`Z` | Undo. While Clean Up runs: cancel it and go back to before it started | In the text, table-cell and angle boxes: undoes the typing in the box. In the label box and in other panels' fields: undoes the canvas |
| `Ctrl`+`Y`, `Ctrl`+`Shift`+`Z` | Redo | Same |
| `Ctrl`+`C` | Copy. The whole drawing when nothing is selected | Off (copies the text in the field) |
| `Ctrl`+`X` | Same as copy. Nothing is cut | Off (cuts the text in the field) |
| `Ctrl`+`V` | Paste | Off (pastes into the field) |
| `Ctrl`+`0` | Fit the whole drawing. The UI scale also goes back to 100% ([6.2](#62-ui-scale)) | Fit is off. The UI scale is still reset |
| `Ctrl`+`1` / `2` / `3` | Switch to the Select tool / Bond tool / Eraser | On |
| `Ctrl`+`K` | Start or stop Clean Up. With atoms or bonds selected, only that part is cleaned up | On |
| `Ctrl`+`Shift`+`K` | Clean up the reaction scheme (arrows stay, structures are tidied and re-spaced) | On |
| `Ctrl`+`R` | Open the angle box ([2.6](#26-input-boxes)). Only with two or more atoms selected | Off |
| `Ctrl`+`Shift`+`F` | Flip the selection. Only with two or more atoms selected and exactly one bond joining the selection to the rest. Solid and hashed wedges are swapped so the configuration is kept | Off |

There is no key for the Arrow, Curly arrow and Text tools (use the palette or the Reaction menu).

### 2.3 Bond tool

#### Mouse

| Operation | Condition | Effect |
| --- | --- | --- |
| Click | Empty space | Place an atom (the element chosen in the palette) |
| Click | On an atom | Grow one bond from it. If an atom is already where it lands, bond to that one; if they are already bonded, change the order |
| Click | On a bond, with the single, double or triple button chosen | Cycle the order: single → double → triple → single |
| Click | On a single bond, with the wedge, hashed wedge, wavy or dative button chosen | Replace it with that kind. A wedge's narrow end, and a dative bond's tail, is the atom nearer the click |
| Click | On a bond already of the chosen kind | A wedge, hashed wedge or dative bond is turned round. A wavy bond goes back to a plain single bond |
| Click | On a double or triple bond, with the wedge, hashed wedge, wavy or dative button chosen | Cycle the order (the kind is not applied) |
| Drag | From an atom | Draw a bond. It joins the atom where it is released, or places a new one. A dative bond (a coordinate bond) points from the atom the drag began on: drag from the ligand's donor atom to the metal |
| Drag | From an atom to a neighbour it is already bonded to | Does to that bond what a click does. A wedge's narrow end, and a dative bond's tail, is the atom the drag began on |
| Drag | From empty space | Lasso (as in the Select tool) |
| Drag | From a bond | Nothing |

#### Keys

Letter keys do nothing while `Ctrl` or `Alt` is held, and do not auto-repeat. Arrow keys do auto-repeat.

| Key | Condition | Effect |
| --- | --- | --- |
| `Delete` / `Backspace` | Something is hovered or targeted by the keyboard | Delete it. In order: hovered atom, object, bond, then the bond target, then the hotspot. Selected atoms are not deleted |
| `Enter`, `g`, `G` | Hovering an atom | Make it the hotspot |
| `Space`, `Tab` | | Clear the hotspot and the bond target |
| Arrow keys | A hotspot exists | Move the hotspot to the neighbour in that direction |
| Arrow keys | No hotspot | Move the place where keys put atoms by one bond length |
| `Shift`+arrow | A hotspot exists | Make the bond to the neighbour in that direction the bond target |
| `` ` `` (backtick) | Hovering an atom, or a hotspot exists | Open the label box ([2.6](#26-input-boxes)) |
| `x`, `X` | Hovering an atom, or a hotspot exists | If the label is a fragment name (`Boc`, `Ph`, …; chained names such as `MeOPh` too), expand it into the structure |
| Letter and digit keys | See "What a key acts on" below | [Atom keys](#atom-keys) or [bond keys](#bond-keys) |

#### What a key acts on

When a letter or digit key is pressed, what it acts on is decided in this order.

1. An atom is hovered → the [atom key](#atom-keys) is applied to it.
2. No atom is hovered and the key is an "element key" (marked ● in the atom key table) → one atom of that element is placed at the pointer. This step is skipped when the pointer is off the canvas and a hotspot exists.
3. A bond is hovered, or a bond target exists → the [bond key](#bond-keys) is applied to it.
4. A hotspot exists → the atom key is applied to it.

Step 2 comes before step 3. So while the mouse hovers a bond, the keys `w`, `h`, `b`, `B`, `d` and `9`, which are also element keys, do not act as bond keys: an atom is placed at the pointer instead. On a bond target set with `Shift`+arrow they do act as bond keys (with the pointer off the canvas).

#### Atom keys

Capitals are typed with `Shift`. ● marks the keys that are also element keys (step 2 above).

| Key | On an atom | ● |
| --- | --- | --- |
| `0`, `1` | Add one carbon | |
| `z` / `Z` | Add a zigzag chain of 2 / 4 carbons | |
| `v` | Three-membered ring | |
| `4`, `u` | Four-membered ring | |
| `5` / `6` / `7` / `8` | Five- / six- / seven- / eight-membered ring | |
| `a`, `3`, `P` | Benzene ring | |
| `2` | Oxo (=O) | |
| `c`, `m` | Me (one carbon) | ● |
| `e` | Et | |
| `A` | Ac | |
| `E` | CO₂Me | |
| `F` | CF₃ | |
| `Q`, `y` | Boc | |
| `H` | Cbz | |
| `o` | OH | ● |
| `O` | O | ● |
| `q` | OMe | ● |
| `n`, `w` | NH₂ | ● |
| `N` | N | ● |
| `9` | N₃ | ● |
| `s` | SH | ● |
| `S` | SiH₃ | ● |
| `p` | PH₂ | ● |
| `f` | F | ● |
| `C`, `l` | Cl | ● |
| `b` | Br | ● |
| `i`, `I` | I | ● |
| `h` | H | ● |
| `d` | D | ● |
| `B` | BH₂ | ● |
| `L` | Li | ● |
| `M` | MgBr | ● |
| `+`, `=` | Charge +1 | |
| `-` | Charge −1 | |

- A group that begins with a heteroatom (`o`, `n`, …) replaces the element of the atom it is pressed on (`o` on a carbon makes that carbon the O of OH).
- A skeleton that begins with carbon (rings, `e`, `A`, `Q`, …) leaves the atom's element alone (`a` on an N gives pyridine).
- Pressing a ring key again draws the next ring on from the one just drawn.
- `3` on an atom draws the same benzene ring as `a`; on a bond it still makes a triple bond ([bond keys](#bond-keys)). A `chembuilder_config.json` saved while `3` added three carbons is read with `3` as the benzene ring too.

#### Bond keys

| Key | On a bond | Works while the mouse hovers the bond? |
| --- | --- | --- |
| `1` / `2` / `3` | Single / double / triple (a wedge or other kind is removed) | Yes |
| `w` | Wedge (toward the viewer) | No (an N is placed) |
| `W` | Hashed wedge (away) | Yes |
| `h` | Hashed wedge (away) | No (an H is placed) |
| `H` | Bold | Yes |
| `b`, `B` | Bold | No (a Br, a B is placed) |
| `d` | Dashed | No (a D is placed) |
| `D` | Dashed | Yes |
| `y` | Wavy | Yes |
| `a`, `A` | Fuse a benzene ring (the bond is one of its sides) | Yes |
| `v` | Fuse a three-membered ring | Yes |
| `4`–`8` | Fuse a four- to eight-membered ring | Yes |
| `9` | Fuse a nine-membered ring | No (an N is placed) |
| `0` | Fuse a ten-membered ring | Yes |

Pressing a kind key (wedge, bold, dashed, wavy) again puts the plain bond back.

The key assignments can be changed with a `chembuilder_config.json` in the folder the app is started from. The tables above are the defaults.

### 2.4 Select tool

#### Mouse

| Operation | Condition | Effect |
| --- | --- | --- |
| Click | On an atom, bond or object | Add it to the selection, or remove it if selected (it adds without a modifier key) |
| Click | Empty space | Clear the selection |
| Double-click | On an atom or bond | Select the whole connected molecule |
| Double-click | On a text or a bracket's label | Open the text box |
| Double-click | On a cell of a reaction table | Open the cell's box |
| Drag | From an atom | Move that atom, or the whole selection if it is part of it |
| Drag | From an object | Move that object, or the whole selection if it is part of it |
| Drag | From an end of an arrow that is the only thing selected | Move that end. Its direction snaps to 15° |
| `Shift`+drag | Same | Move that end in any direction |
| Drag | From empty space | Lasso. It replaces the selection |
| `Alt`+drag | One or more atoms selected (start anywhere) | Rotate the selection freely about its centre |
| Drag the rotation handle | Two or more atoms selected (a round handle appears above the selection) | Rotate the selection in 15° steps |
| `Shift`+drag the rotation handle | Same | Rotate freely |
| Double-click the rotation handle | Same | Open the angle box |
| Click the flip button | The button is shown left of the handle (one bond joins the selection to the rest) | Flip the selection (same as `Ctrl`+`Shift`+`F`) |

#### Keys

| Key | Condition | Effect |
| --- | --- | --- |
| `Delete` / `Backspace` | Something is selected | Delete the selected atoms, bonds and objects |
| `Delete` / `Backspace` | Nothing is selected | As in the Bond tool (delete what is hovered or targeted by the keyboard) |
| `Enter` | Hovering an atom | Make it the hotspot |
| `g`, `G` | Hovering an atom, or a hotspot exists | Make it the hotspot and add it to the selection |
| `Space`, `Tab` | | Clear the hotspot and the bond target |
| Arrow keys | A hotspot exists | Move the hotspot to the neighbour in that direction |
| `Ctrl`+arrow, `Shift`+arrow | Atoms selected | Move the selected atoms by one bond length |
| `Alt`+`→` / `Alt`+`←` | Atoms selected | Rotate the selected atoms 15° clockwise / anticlockwise |
| [Atom keys](#atom-keys) | Hovering an atom, or a hotspot exists | Applied to that atom |

These do not work in the Select tool: the [bond keys](#bond-keys), `x` (expand a label), `` ` `` (label box), the bond target by `Shift`+arrow, and element keys over empty space.

### 2.5 Eraser, Arrow tool, Curly arrow tool, Text tool

| Tool | Operation | Condition | Effect |
| --- | --- | --- | --- |
| Eraser | Click | On an atom, object or bond | Delete it. The Eraser has no keys |
| Arrow | Drag | | Draw an arrow. Its direction snaps to 15° |
| Arrow | `Shift`+drag | | Draw an arrow in any direction |
| Arrow | Click | | Place an arrow of the default length pointing right |
| Arrow | `Delete` / `Backspace` | Something is hovered or targeted by the keyboard | Delete it (as in the Bond tool) |
| Curly arrow | Drag from an atom | | A curly arrow from that atom's lone pair (drawn as two dots) to the atom or bond where the drag ends (to empty space: a free end). The arrow records what it means — its source and its target — and is drawn automatically from that, clear of the atoms, labels and bonds around it, following them when they move. It is left selected; the status bar shows its roles, e.g. `O: → C` |
| Curly arrow | Drag from a bond | | The same, from the bond's electrons, moving onto the atom of the bond nearer the press. Pressed on C=O near O and released on H, it records C=O, O and H (`C=O → (O) → H`) and is drawn as an S: off the bond, round O on the outside, into H along the O···H bond being made |
| Curly arrow | `Shift` held at the release | Ending on an atom | The target is the gap between that atom and the source's atom (the bond being formed) |
| Curly arrow | Click | On a curly arrow | Select it alone, showing its handles |
| Any tool | Drag a small ring handle (a Bézier control point, on a thin line from its end of the curve) | One curly arrow selected | Shape it by hand: an S, a long U or L round other atoms. The shape is kept relative to the arrow's ends, so it follows the atoms. Reaction > Back to automatic shape gives the automatic shape back |
| Any tool | Double-click the curve of a curly arrow | | Add a knot there (a square handle), splitting the curve into two pieces with handles of their own. Drag the knot to route the arrow |
| Any tool | Double-click a knot / a control point | One curly arrow selected | Take the knot out / back to the automatic shape |
| Any tool | Drag the diamond handle at either end | One curly arrow selected | Re-point that end, which changes what the arrow means: the source onto an atom (its lone pair) or a bond (onto the atom of it nearer the pointer), the target onto an atom, a bond or empty space (a free end). `Shift` at the release on an atom: the target is the gap between that atom and the source's. An automatic arrow is drawn anew |
| Text | Click | On a text | Open that text's box |
| Text | Click | Elsewhere | Open a new text box there |

### 2.6 Input boxes

While a box is open, the canvas tools' keys and mouse operations are off.

| Box | Opened by | Commit | Cancel | Also |
| --- | --- | --- | --- | --- |
| Atom label | `` ` `` in the Bond tool | `Enter`, or a click outside the box | `Esc` | The click outside only commits; it does not also place an atom |
| Text | A click with the Text tool, a double-click with the Select tool | `Enter`, or a click outside the box | `Esc` | `Shift`+`Enter` starts a new line. `Ctrl`+`G` turns the Symbol font (Greek letters) on or off. A click on the toolbar's format section is part of the typing |
| Reaction table cell | A double-click on the cell with the Select tool | `Enter`, or a click outside the box | `Esc` | A value that cannot be read is not committed by `Enter`; the box stays |
| Rotation angle | `Ctrl`+`R`, a double-click on the rotation handle, Structure → Rotate in the menu | `Enter` (rotates by the angle; positive is clockwise) | `Esc`, or a click outside the box | Something that is not a number keeps the box open on `Enter`. Decimals, negative values and full-width digits are accepted |

Inside a label or a text, `_2` is a subscript, `^+` a superscript, `**…**` bold and `//…//` italic.

### 2.7 When operations are off

| State | Off | Still on |
| --- | --- | --- |
| Clean Up is running | The tools' mouse operations and keys, the middle-drag pan | The `Ctrl` keys ([2.2](#22-operations-that-work-in-every-tool)), wheel zoom |
| A box on the canvas is open | Same | As the "While typing" column of [2.2](#22-operations-that-work-in-every-tool) says |
| The cursor is in a text field of another panel | Same | Same |
| `Esc` during a drag | A lasso is abandoned | |

---

## 3. 3D view

Shows and edits the 3D model. The keys work only while the cursor is not in a field of the right panel. While the model is being generated or optimised, only the view can be moved.

### 3.1 View

| Operation | Condition | Effect |
| --- | --- | --- |
| Drag | | Orbit |
| `Shift`+drag | When it is not "moving a molecule" below | Pan |
| Right drag, middle drag | | Pan |
| Wheel | No rotatable bond is picked | Zoom |
| Double-click | Empty space | Reset the view (same as Reset view in the right panel) |

### 3.2 Picking, selecting and fixing atoms

The 3D view has two kinds of selection: the one used for measuring and setting values (orange; "pick" below) and the one for deleting (pink; "delete selection" below).

| Operation | Condition | Effect |
| --- | --- | --- |
| Click | On an atom | Add it to the pick, or remove it. Up to four (two: distance, three: angle, four: dihedral) |
| Click | On a bond | Pick its two atoms |
| Click | Empty space | Clear both the pick and the delete selection |
| `Ctrl`+click | On an atom | Add it to the delete selection, or remove it |
| `Ctrl`+click | On a bond | Add its two atoms to the delete selection, or remove them |
| `Ctrl`+click | Empty space | Nothing |
| `Ctrl`+drag | Anywhere | Add the atoms inside the rectangle to the delete selection (the view does not move) |
| Right-click | On an atom | Fix that atom / release it |
| Right-click | Empty space | Release the fixed atom |

### 3.3 Moving a molecule, turning a bond

| Operation | Condition | Effect |
| --- | --- | --- |
| `Shift`+drag | The model has two or more molecules and the drag starts on one (an atom or a bond) | Move that molecule parallel to the screen |
| `Alt`+drag | Same | Rotate that molecule |
| `Shift`+`Alt`+drag | Same | Rotate that molecule (`Alt` wins) |
| `Shift`+drag, `Alt`+drag | From the molecule holding the fixed atom | It does not move (a message says so) |
| `Alt`+drag | One molecule only, or from empty space | Orbit (as without a modifier) |
| Wheel | The two atoms of a rotatable bond are picked | Turn that bond 5° per notch (no zoom) |
| `Shift`+wheel | Same | 1° per notch |
| `Ctrl`+wheel | Same | 15° per notch |

While `Ctrl` is held a drag is always the delete-selection rectangle, even with `Shift` or `Alt` held too.

### 3.4 Keys

| Key | Effect |
| --- | --- |
| `Ctrl`+`Z` | Undo the 3D edit |
| `Ctrl`+`Y`, `Ctrl`+`Shift`+`Z` | Redo the 3D edit |
| `Delete` / `Backspace` | Delete the atoms in the delete selection; if there are none, the picked atoms |
| `Esc` | Clear both the pick and the delete selection |
| `Ctrl`+`C` (`Ctrl`+`X`) | Copy the view as shown (the panel's Copy button, and Edit ▸ Copy, do the same). On Windows it pastes into Word, PowerPoint and Excel as an object with a transparent background holding the model as MOL2; double-click it there to edit it here again. Elsewhere it copies the picture. Nothing is cut |

### 3.5 Right panel

| Operation | Condition | Effect |
| --- | --- | --- |
| `Enter` | In the value box (bond length, angle, dihedral) | Set the typed value (same as the Set button) |
| Click | A row of the hydrogen-bond list | Pick the donor, the hydrogen and the acceptor |

---

## 4. Assembly view

Builds an assembly out of molecules. The keys work only while the cursor is not in a field of the right panel.

### 4.1 Viewer

| Operation | Condition | Effect |
| --- | --- | --- |
| Drag | | Orbit |
| `Shift`+drag | When it is not "moving the selected molecule" below | Pan |
| Right drag, middle drag | | Pan |
| Wheel | | Zoom |
| Double-click | Empty space | Reset the view |
| Click | On an atom or bond, normally | Select its molecule (a click on a periodic image of a crystal selects the molecule it repeats) |
| Click | Empty space, normally | Clear the selection |
| Click | On an atom, measuring mode | Add it to the measured atoms, or remove it. Up to four |
| Click | On a bond, measuring mode | Measure its two atoms |
| Click | Empty space, measuring mode | Clear the measured atoms |
| Click | On an atom, while orienting a monomer | Choose the atoms that set its orientation (three) |
| `Shift`+drag | A hand-placed molecule is selected, not measuring (start anywhere) | Move the selected molecule parallel to the screen |
| `Alt`+drag | Same | Rotate the selected molecule about its centre |
| `Shift`+`Alt`+drag | Same | Rotate (`Alt` wins) |

A right-click does nothing.

### 4.2 Keys

| Key | Effect |
| --- | --- |
| `Ctrl`+`Z` | Undo the assembly edit |
| `Ctrl`+`Y`, `Ctrl`+`Shift`+`Z` | Redo the assembly edit |
| `M` | Measuring mode on / off |
| `Esc` | Stop orienting if that is going on; otherwise stop measuring if that is on; otherwise clear the selection |

### 4.3 Right panel

| Operation | Condition | Effect |
| --- | --- | --- |
| Double-click | A monomer's name | Turn it into a field to rename it. Leaving the field ends it |
| Drag | The grip (⠿) on the left of a step's card | Reorder the steps |

---

## 5. Windows and panels

| Where | Operation | Effect |
| --- | --- | --- |
| Tabs | Click | Switch to that document |
| Tabs | Middle-click, or click × | Close that tab |
| Whole window | Drop a file | Open it |
| Palette (periodic table) | `Enter` in the symbol field | Make that element the one the Bond tool places |
| Toolbar (text size list) | `Enter` in the field | Use the typed size |
| Manage Fragments | `Enter` in the rename field | Rename |
| Parameter scan plot | Click a point (cell) | Apply that point's values to the structure |
| Parameter scan plot | Drag | Move the range shown |
| Parameter scan plot | Wheel | Scroll the range shown |
| Parameter scan plot | `Ctrl`+wheel, pinch | Zoom |
| Parameter scan plot | Right drag | Zoom into the rectangle |
| Parameter scan plot | Drag on an axis's ticks | Zoom that axis only |
| Parameter scan plot | Double-click | Reset the range shown |
| Conformer search | Click a number | Apply that conformer to the 3D model |
| Unsaved changes, update available | `Esc`, or a click outside the dialog | Cancel |

Apart from the click on a point, the plot's operations are the defaults of the plot component (egui_plot).

---

## 6. Common to the whole app

### 6.1 Files and editing

| Key | Effect | Where it works |
| --- | --- | --- |
| `Ctrl`+`N` | New | Every screen, even with the cursor in a text field |
| `Ctrl`+`O` | Open | Same |
| `Ctrl`+`S` | Save | Same |
| `Ctrl`+`Shift`+`S` | Save As | Same |
| `Ctrl`+`W` | Close tab | Same |
| `Ctrl`+`Z` / `Ctrl`+`Y` / `Ctrl`+`Shift`+`Z` | Undo / Redo | Each screen has its own history ([2.2](#22-operations-that-work-in-every-tool), [3.4](#34-keys), [4.2](#42-keys)) |
| `Ctrl`+`C` / `Ctrl`+`V` | Copy / Paste | The 2D editor's canvas ([2.2](#22-operations-that-work-in-every-tool)); `Ctrl`+`C` also in the 3D view, which copies the view ([3.4](#34-keys)). In a text field they act on its text |

Undo and Redo in the Edit menu and on the toolbar act on the assembly in the assembly view, and on the 2D drawing everywhere else, the 3D view included. In the 3D view the `Ctrl`+`Z` key undoes the 3D edit.

### 6.2 UI scale

| Key | Effect |
| --- | --- |
| `Ctrl`+`+`, `Ctrl`+`=` | Make the whole UI 10% larger |
| `Ctrl`+`-` | Make the whole UI 10% smaller |
| `Ctrl`+`0` | Set the UI scale to 100% |

- The scale is remembered for the next launch. It starts at 110% (not the 100% that `Ctrl`+`0` goes back to).
- In the 2D editor `Ctrl`+`0` also fits the drawing, at the same time.

### 6.3 Text fields and number fields

| Operation | Effect |
| --- | --- |
| `Ctrl`+`C` / `X` / `V` / `A` / `Z` in a text field | Copy / cut / paste / select all / undo, for the text in the field |
| Drag a number field | Change the value |
| `Shift`+drag a number field | Change the value in finer steps |
| Click a number field | Type the value. `Enter` commits, `Esc` cancels |
| `↑` / `↓` with the cursor in a number field | Change the value |
| `Esc` while a menu or list is open | Close it |

---

## 7. Differences on macOS

In the code, which key stands for `Ctrl` depends on the operation.

| Operation | Key on macOS |
| --- | --- |
| New, Open, Save, Save As, Close tab | `Cmd` |
| UI scale (`+`, `-`, `0`) | `Cmd` |
| Undo / Redo in the 3D view and the assembly view | `Cmd` |
| The delete selection in the 3D view (click, drag), and the wheel that turns a bond by 15° | `Cmd` |
| Copy and paste in the 2D editor, the Symbol font in the text box (`G`) | `Cmd` |
| In the 2D editor: Undo / Redo, fit (`0`), tool switching (`1` / `2` / `3`), Clean Up (`K`), rotate (`R`), flip (`Shift`+`F`), moving the selection (arrow keys) | `Control` |

So on macOS, fitting the drawing (`Control`+`0`) and resetting the UI scale (`Cmd`+`0`) are different keys.

---

## 8. Where the controls get complicated

Facts only.

### 8.1 The same operation means different things by screen, tool or state

| Operation | What it means where |
| --- | --- |
| `Shift`+drag | 3D: pan; but with two or more molecules and the drag starting on one, move that molecule. Assembly: pan; but with a hand-placed molecule selected, move it, wherever the drag starts. 2D Select tool: on the rotation handle or an arrow's end, drop the 15° steps. 2D Arrow tool: drop the 15° steps |
| `Alt`+drag | 2D Select tool: rotate the selection. 3D: from a molecule, rotate it; otherwise orbit. Assembly: rotate the selected hand-placed molecule |
| `Ctrl`+drag | 3D: delete-selection rectangle. Other screens: same as without a modifier |
| `Ctrl`+`0` | 2D: fits the drawing and resets the UI scale at once. 3D, assembly: resets the UI scale only |
| `Ctrl`+`Z` | 2D: the drawing. 3D: the 3D edits. Assembly: the assembly. Text, table-cell and angle boxes: the typing in the box. The label box and other panels' fields (while 2D is shown): the drawing |
| Undo in the menu and toolbar | Assembly view: the assembly. 3D view: the 2D drawing, which is not shown (not what the `Ctrl`+`Z` key undoes there) |
| `Delete` | 2D Bond and Arrow tools: what is hovered (the selection stays). 2D Select tool: the selection, else what is hovered. 3D: the delete selection, else the pick. Assembly: nothing |
| Click | 2D Bond tool: draw. 2D Select tool: add to or remove from the selection. 3D: pick (for measuring). Assembly: select a molecule; in measuring mode, pick an atom |
| Double-click | 2D Select tool: select the whole molecule, edit a text or a cell, and on the rotation handle open the angle box. 3D and assembly viewers: reset the view. Assembly panel: rename. Scan plot: reset the range |
| Right-click | 2D: menu. 3D: fix an atom. Assembly: nothing |
| Right drag | 3D, assembly: pan. Scan plot: zoom into the rectangle. 2D: nothing |
| Wheel | 2D, 3D, assembly: zoom. 3D with a rotatable bond picked: turn the bond. Scan plot: scroll (zoom is `Ctrl`+wheel) |
| `Shift` (changes the step) | 2D rotation handle and arrows: drop the 15° steps for any angle. 3D bond turning by wheel: 5° becomes 1°. Dragging a number field: finer |
| `Ctrl`+wheel | 2D: zoom. 3D with a bond picked: turn it 15° at a time. Scan plot: zoom |
| Arrow keys | 2D Bond tool: move the hotspot, or with no hotspot the place where atoms are put. 2D Select tool: move the hotspot only |
| `Shift`+arrow | 2D Bond tool: set the bond target. 2D Select tool: move the selection |
| `g` | 2D Bond tool: make the hovered atom the hotspot. 2D Select tool: make it the hotspot and add it to the selection (works without a hover too) |
| `w`, `h`, `b`, `B`, `d`, `9` | On an atom: NH₂, H, Br, BH₂, D, N₃. On a bond target (keyboard): wedge, hashed wedge, bold, bold, dashed, nine-membered ring. With the mouse over a bond or over empty space: place one atom |
| `a`, `1`–`8`, `0`, `v`, `y`, `H`, … | One action on an atom, another on a bond ([atom keys](#atom-keys), [bond keys](#bond-keys)) |
| `M` | 2D (on an atom): MgBr. Assembly: measuring mode |
| `Esc` | 2D: cancel an input box, abandon a lasso (it does not clear the selection). 3D: clear the pick and the delete selection. Assembly: stop orienting, else measuring, else clear the selection |
| `Enter` | 2D canvas: make the hovered atom the hotspot. Input boxes: commit |
| `Tab` | 2D canvas: clear the hotspot |
| `Ctrl`+`X` | 2D canvas: copy (nothing is cut). 3D view: copy the view. Text fields: cut |

### 8.2 The same function has different operations by screen

| Function | 2D editor | 3D view | Assembly view | Scan plot |
| --- | --- | --- | --- | --- |
| Move the view | Middle drag only | `Shift`+drag, right drag, middle drag | Same as 3D | Drag, wheel |
| Zoom | Wheel | Wheel | Wheel | `Ctrl`+wheel |
| Show everything | `Ctrl`+`0`, toolbar, menu | Double-click empty space, panel button | Double-click empty space | Double-click |
| Select | Click with the Select tool, lasso, double-click | Click (pick); `Ctrl`+click and `Ctrl`+drag (delete selection) | Click (whole molecules) | — |
| Clear the selection | Click empty space with the Select tool (`Esc` does not) | `Esc`, click empty space | `Esc`, click empty space | — |
| Delete | `Delete` (what it deletes depends on the tool), Eraser | `Delete`, panel button | Panel buttons | — |
| Move a molecule | Drag with the Select tool, `Ctrl` / `Shift`+arrow | `Shift`+drag from the molecule | Select it, then `Shift`+drag (from anywhere) | — |
| Rotate a molecule | `Alt`+drag, rotation handle, `Ctrl`+`R`, `Alt`+`←` / `→` | `Alt`+drag from the molecule | Select it, then `Alt`+drag | — |
| Redo | `Ctrl`+`Y`, `Ctrl`+`Shift`+`Z` | Same | Same | — |

### 8.3 What works only after switching tools (2D editor)

| Operation | Tools it works in |
| --- | --- |
| [Bond keys](#bond-keys), `x` (expand a label), `` ` `` (label box), the bond target by `Shift`+arrow, element keys over empty space | Bond tool only |
| Moving by `Ctrl` / `Shift`+arrow, rotating by `Alt`+`←` / `→`, `Alt`+drag, the rotation handle, the flip button, adding to the selection with `g`, deleting the selection with `Delete`, editing by double-click | Select tool only |
| [Atom keys](#atom-keys), `Enter`, `Space`, `Tab`, moving the hotspot with the arrows | Bond tool and Select tool |
| `Ctrl`+`R`, `Ctrl`+`Shift`+`F` (given a selection) | Every tool |
| Lasso | Select tool and Bond tool (a selection lassoed in the Bond tool is not deleted by `Delete` in the Bond tool) |
| Switching tools by key | Select, Bond and Eraser only (the Arrow, Curly arrow and Text tools have no key) |

### 8.4 What the menu shows, and where the key is read

| Shown in the menu | Read by | Screens it works in |
| --- | --- | --- |
| `Ctrl`+`N`, `Ctrl`+`O`, `Ctrl`+`S`, `Ctrl`+`Shift`+`S`, `Ctrl`+`W` | The app | Every screen |
| `Ctrl`+`Z`, `Ctrl`+`Y` | The 2D editor, the 3D view and the assembly view, each for its own history | The screen shown |
| `Ctrl`+`C`, `Ctrl`+`V` | The app, only while the 2D editor is shown | 2D only (from the menu they act on the 2D drawing even while 3D or the assembly is shown) |
| `Ctrl`+`0` (Fit) | The 2D editor | 2D only (from the menu it acts on the 2D drawing even while 3D or the assembly is shown) |
| `Ctrl`+`R`, `Ctrl`+`Shift`+`F` | The 2D editor | 2D only |

### 8.5 Operations shown in no menu and no tooltip

- `Ctrl`+`K` (Clean Up; the menu item shows no key), `Ctrl`+`Shift`+`K` (reaction clean-up; there is no menu item either)
- `Ctrl`+`1` / `2` / `3` (tool switching)
- `Ctrl`+`Shift`+`Z` (redo; only `Ctrl`+`Y` is shown)
- `Ctrl`+`+` / `-` (UI scale)
- That `Ctrl`+`X` on the canvas copies
- The atom keys, the bond keys, `x`, `` ` ``, `Enter`, `g`, `Space`, `Tab` and the arrow keys (only the usage note in Manage Fragments mentions `x`)
- `Ctrl` / `Shift`+arrow, `Alt`+`←` / `→` and `Alt`+drag in the Select tool
- The double-click on the rotation handle, and `Shift` dropping the steps
- The middle-drag pan in 2D
- Closing a tab with a middle click
- The right-drag and middle-drag pan in 3D and the assembly
- The right drag, axis drag and `Ctrl`+wheel in the scan plot

### 8.6 Keys that reach the canvas while typing (while 2D is shown)

`Ctrl`+`1` / `2` / `3`, `Ctrl`+`K` and `Ctrl`+`Shift`+`K` act on the canvas whichever field the cursor is in. `Ctrl`+`Z` and `Ctrl`+`Y` undo and redo the canvas while the cursor is in the label box or in another panel's field (a fragment name, an element symbol, …). `Ctrl`+`N` / `O` / `S` / `W` work whichever field the cursor is in.

---

## 9. Not verified

- Nothing here was checked in the running app; it is all read from the code.
- The macOS section is read from how the UI library (egui) treats modifier keys.
- That `w`, `h`, `b`, `B`, `d` and `9` place an atom instead of acting as bond keys while the mouse hovers a bond is read from the order of the checks in the code.
- Whether the atom keys in the Select tool, and moving or rotating with `Ctrl` / `Shift` / `Alt`+arrow, are each taken back by one Undo.
- For `Esc` during a drag, only abandoning a lasso was confirmed. What it does in the middle of moving or rotating atoms was not checked.
- Whether `Tab` on the 2D canvas also moves the keyboard focus to a field, besides clearing the hotspot.
- What a double-click on an atom does in the 3D view.
- The scan plot's operations (other than the click on a point) and the number fields' operations are read from the UI libraries' defaults.
- What letter keys do while a Japanese input method (IME) is on.
