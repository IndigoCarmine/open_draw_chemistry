//! AcaudaOffice's editor for embedded ChemDraw objects, through the `acauda-embed` crate: the
//! host starts this executable with `--ibis-embed <socket> --token <token> --mode …`.
//!
//! There is no COM here, so it works alike on Windows, macOS and Linux. The object is the same
//! ChemDraw object [`crate::ole_server`] edits (the CDX in `CONTENTS`), and it goes through the
//! same tabs: [`Server`] stands where the COM server does, at the other end of an
//! [`AppBridge`], and turns the host's events into [`ToApp`] and [`ToServer`] into its updates.
//! Requests that need no window (a new object, a picture, import, export) are answered by
//! [`Requests`] before any window opens.
//!
//! The picture is the Windows-only PNG of `chembuider_rs::molecule::image` where it exists, and
//! otherwise an SVG drawn here from the same shared scene ([`svg`]).

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::Path;
use std::sync::mpsc::RecvTimeoutError;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use acauda_embed::editor::{self, Created, Editor, EditorEvent, Headless, ObjectId, Start};
use acauda_embed::{Clsid, HandlerArgs, OleContent, Preview};
use chembuider_rs::Molecule;
use chembuider_rs::config::StyleConfig;
use chembuider_rs::widget::SCALE_FACTOR;
use chembuider_rs::widget::fonts::standalone_fonts;
use chembuider_rs::widget::rich_text::RichGalley;
use chembuider_rs::widget::scene::{MoleculeScene, RenderTransform, SceneStroke};

use crate::io::{self, Format};
use crate::ole_server::compobj::{CHEMDRAW_PROG_ID, CHEMDRAW_USER_TYPE, guid_bytes};
use crate::ole_server::{self, AppBridge, CHEMDRAW_CLSID, HostNames, ServerSide, ToApp, ToServer};

/// Names the one instance every object goes to (the same as the macOS bundle's identifier).
pub const APP_ID: &str = "io.github.indigocarmine.open-draw-chemistry";
/// The stream a ChemDraw object keeps its drawing in (a CDX document).
pub const CONTENTS: &str = "CONTENTS";
/// The container's name on the tabs and in the status line.
const HOST_APP: &str = "AcaudaOffice";

/// How long an edit waits before it goes to the host, however many follow it.
const LIVE_UPDATE_DELAY: Duration = Duration::from_millis(300);
/// How long a window waits for its first object: the host may have gone away.
const FIRST_OBJECT_TIMEOUT: Duration = Duration::from_secs(60);
/// How often the host's events are looked at. Everything sent the other way goes at once.
const POLL: Duration = Duration::from_millis(15);

/// EMU (Office's unit) per point.
const EMU_PER_PT: f32 = 12_700.0;
/// Points per pixel of the drawing: its 1.5-unit bond is ChemDraw's 14.4 pt one.
const PT_PER_PX: f32 = 14.4 / (1.5 * SCALE_FACTOR);
/// Empty space around the drawing, in pixels.
const MARGIN_PX: f32 = 12.0;
/// The picture of an object with nothing drawn yet, in pixels (an inch by half an inch).
const BLANK_PX: [f32; 2] = [72.0 / PT_PER_PX, 36.0 / PT_PER_PX];
/// No side of a PNG picture is larger: sharp on a slide, small in the document.
#[cfg(windows)]
const PREVIEW_MAX_SIDE: u32 = 2400;

/// What the process was started for.
pub enum Launch {
    /// Not by the host: run as usual.
    NotStarted,
    /// A request was answered, or the object went to the window already open: exit.
    Done,
    /// Open the window, with this as its server (see [`crate::app::OpenDrawApp::with_ole`]).
    Serve(Server, AppBridge),
}

/// Does what the host started the process for, from the command line `args`.
pub fn start(args: &[OsString]) -> acauda_embed::Result<Launch> {
    // Not `editor::start`: it reads `std::env::args`, which panics on a file name that is not
    // Unicode. Such an argument cannot be one of the host's.
    let args = args.iter().map(|a| a.to_string_lossy().into_owned());
    Ok(
        match editor::start_with(APP_ID, HandlerArgs::parse(args)?, Requests)? {
            Start::NotStarted => Launch::NotStarted,
            Start::Done => Launch::Done,
            Start::Interactive(editor) => {
                let (server, bridge) = Server::start(editor);
                Launch::Serve(server, bridge)
            }
        },
    )
}

/// The thread between the app and the host.
pub struct Server {
    thread: JoinHandle<()>,
    notify: ole_server::Notifier,
}

impl Server {
    /// Starts serving `editor`'s objects; the bridge goes to the app.
    pub fn start(editor: Editor) -> (Self, AppBridge) {
        // Started by the host, so the process ends with its last object, as under COM. The
        // thread waits on the channel itself: it needs no waking.
        let (mut bridge, side) = ole_server::channel(Box::new(|| {}), true);
        bridge.live_updates = Some(LIVE_UPDATE_DELAY);
        let notify = bridge.notifier();
        let thread = std::thread::spawn(move || serve(editor, side, FIRST_OBJECT_TIMEOUT));
        (Self { thread, notify }, bridge)
    }

    /// The window has closed: delivers what is still on its way, then ends the connections.
    pub fn shutdown(self) {
        self.notify.send(ToServer::Shutdown);
        let _ = self.thread.join();
    }
}

/// The thread's loop, until the app has gone. Returning drops `editor`, which tells the host.
fn serve(editor: Editor, side: ServerSide, first_object_timeout: Duration) {
    // The objects open here, as they came: their class and other streams are given back.
    let mut objects: HashMap<u64, OleContent> = HashMap::new();
    let started = Instant::now();
    let mut waiting = true;
    loop {
        while let Some(event) = editor.try_next() {
            match event {
                EditorEvent::Opened {
                    object,
                    content,
                    title,
                    ..
                } => {
                    waiting = false;
                    let id = object.0 as u64;
                    side.send(ToApp::Open {
                        id,
                        cdx: drawing(&content).map(<[u8]>::to_vec),
                        host: HostNames {
                            app: HOST_APP.to_owned(),
                            document: title,
                        },
                    });
                    objects.insert(id, content);
                }
                EditorEvent::Closed { object } => {
                    let id = object.0 as u64;
                    if objects.remove(&id).is_some() {
                        side.send(ToApp::Close { id, reply: None });
                    }
                }
            }
        }
        if waiting && started.elapsed() >= first_object_timeout {
            waiting = false;
            side.send(ToApp::Idle);
        }
        match side.rx.recv_timeout(POLL) {
            Ok(ToServer::Update { id, cdx }) => {
                let Some(content) = objects.get_mut(&id) else {
                    continue;
                };
                let (picture, size) = match io::decode(Format::Cdx, &cdx) {
                    Ok(loaded) => preview(&loaded.molecule),
                    Err(e) => {
                        ole_server::log(&format!("update {id}: {e}"));
                        continue;
                    }
                };
                content.set_stream(CONTENTS, cdx);
                let object = ObjectId(id as usize);
                if let Err(e) = editor.update(object, content.clone(), picture, Some(size)) {
                    ole_server::log(&format!("update {id}: {e}"));
                }
            }
            Ok(ToServer::Closed { id }) => {
                objects.remove(&id);
                editor.close(ObjectId(id as usize));
            }
            Ok(ToServer::Shutdown) | Err(RecvTimeoutError::Disconnected) => return,
            Err(RecvTimeoutError::Timeout) => {}
        }
    }
}

/// The requests answered without a window.
struct Requests;

impl Headless for Requests {
    fn new_object(&mut self) -> Result<Created, String> {
        let mol = Molecule::default();
        let cdx = io::encode(Format::Cdx, &mol, None).map_err(|e| e.to_string())?;
        Ok(created(cdx, &mol))
    }

    fn import(&mut self, extension: &str, data: Vec<u8>) -> Result<Created, String> {
        let loaded = io::decode(format_of(extension)?, &data).map_err(|e| e.to_string())?;
        // A CDX goes in as it is, so ChemDraw's own content stays whole.
        Ok(created(loaded.original, &loaded.molecule))
    }

    fn export(&mut self, content: OleContent, extension: &str) -> Result<Vec<u8>, String> {
        let format = format_of(extension)?;
        let cdx =
            drawing(&content).ok_or_else(|| format!("the object has no {CONTENTS} stream"))?;
        if format == Format::Cdx {
            return Ok(cdx.to_vec());
        }
        let loaded = io::decode(Format::Cdx, cdx).map_err(|e| e.to_string())?;
        io::encode(format, &loaded.molecule, Some(&loaded.original)).map_err(|e| e.to_string())
    }

    fn render(&mut self, content: OleContent) -> Result<(Preview, Option<(i64, i64)>), String> {
        let (picture, size) = match drawing(&content) {
            Some(cdx) => {
                let loaded = io::decode(Format::Cdx, cdx).map_err(|e| e.to_string())?;
                preview(&loaded.molecule)
            }
            None => preview(&Molecule::default()),
        };
        Ok((picture, Some(size)))
    }
}

/// The format of a file with this extension, as the host names it (no dot, any case).
fn format_of(extension: &str) -> Result<Format, String> {
    Format::from_path(Path::new(&format!("object.{extension}")))
        .ok_or_else(|| io::Error::UnknownFormat.to_string())
}

/// The object's CDX; `None` when it has none yet.
fn drawing(content: &OleContent) -> Option<&[u8]> {
    content.stream(CONTENTS).filter(|cdx| !cdx.is_empty())
}

/// A new object holding `cdx`, the drawing of `mol`. It is ChemDraw's, as under COM.
fn created(cdx: Vec<u8>, mol: &Molecule) -> Created {
    let mut content = OleContent {
        clsid: Clsid(guid_bytes(CHEMDRAW_CLSID)),
        prog_id: CHEMDRAW_PROG_ID.to_owned(),
        user_type: CHEMDRAW_USER_TYPE.to_owned(),
        ..Default::default()
    };
    content.set_stream(CONTENTS, cdx);
    let (preview, size) = preview(mol);
    Created {
        content,
        preview,
        size_emu: Some(size),
    }
}

/// The picture of `mol` and its size in EMU, at ChemDraw's scale. A blank one when nothing is
/// drawn: an update must have a picture.
pub fn preview(mol: &Molecule) -> (Preview, (i64, i64)) {
    let emu = |pt: [f32; 2]| {
        (
            (pt[0] * EMU_PER_PT).round() as i64,
            (pt[1] * EMU_PER_PT).round() as i64,
        )
    };
    #[cfg(windows)]
    if let Some((png, pt)) = chembuider_rs::molecule::image::preview_png(mol, PREVIEW_MAX_SIDE) {
        return (Preview::png(png), emu(pt));
    }
    let (svg, px) = svg(mol);
    (
        Preview::svg(svg),
        emu([px[0] * PT_PER_PX, px[1] * PT_PER_PX]),
    )
}

/// `mol` as an SVG, and its size in pixels. Laid out as the widget lays it out, from the scene
/// `chembuider_rs::molecule::image` draws too, except for drop shadows; the host draws the
/// text with its own sans-serif font unless a span names one.
fn svg(mol: &Molecule) -> (String, [f32; 2]) {
    let xf = RenderTransform {
        coord_scale: SCALE_FACTOR,
        offset: egui::Vec2::ZERO,
        visual_scale: 1.0,
    };
    let mut fonts = standalone_fonts();
    let mut view = fonts.with_pixels_per_point(1.0);
    let scene = MoleculeScene::build(mol, &StyleConfig::chemdraw(), xf, |job| {
        view.layout_job(job)
    });
    let Some(bounds) = scene.bounds(mol, xf) else {
        let [w, h] = BLANK_PX;
        return (
            format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w:.0}" height="{h:.0}" viewBox="0 0 {w:.0} {h:.0}"/>"#
            ),
            BLANK_PX,
        );
    };

    let w = bounds.width() + 2.0 * MARGIN_PX;
    let h = bounds.height() + 2.0 * MARGIN_PX;
    let (x, y) = (bounds.min.x - MARGIN_PX, bounds.min.y - MARGIN_PX);
    let mut s = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w:.0}" height="{h:.0}" viewBox="{x:.2} {y:.2} {w:.2} {h:.2}">"#
    );

    // Labels cut a hole through the ring fills and bonds under them, so the background stays
    // transparent around the labels.
    s.push_str(&format!(
        r#"<defs><mask id="labels" maskUnits="userSpaceOnUse" x="{x:.2}" y="{y:.2}" width="{w:.2}" height="{h:.2}"><rect x="{x:.2}" y="{y:.2}" width="{w:.2}" height="{h:.2}" fill="white"/>"#
    ));
    for label in scene.labels.values() {
        let r = label.rect.expand(scene.label_bg_pad);
        s.push_str(&format!(
            r#"<rect x="{:.2}" y="{:.2}" width="{:.2}" height="{:.2}" fill="black"/>"#,
            r.min.x,
            r.min.y,
            r.width(),
            r.height()
        ));
    }
    s.push_str(r#"</mask></defs><g mask="url(#labels)">"#);
    for fill in &scene.ring_fills {
        push_polygon(&mut s, &fill.points, Some(fill.rgba));
    }
    for bond in &scene.bonds {
        for line in &bond.strokes {
            push_line(&mut s, line, bond.color);
        }
        for fill in &bond.fills {
            push_polygon(&mut s, fill, bond.color);
        }
    }
    s.push_str("</g>");
    for label in scene.labels.values() {
        push_glyphs(&mut s, &label.galley, label.rect.min);
    }
    // Page objects (arrows, texts) over everything else.
    for object in &scene.objects {
        for line in &object.strokes {
            push_line(&mut s, line, None);
        }
        for fill in &object.fills {
            push_polygon(&mut s, fill, None);
        }
        for text in &object.texts {
            push_glyphs(&mut s, &text.galley, text.rect.min);
        }
    }
    s.push_str("</svg>");
    (s, [w, h])
}

/// An SVG paint attribute for straight RGBA, `None` being black.
fn paint(attr: &str, rgba: Option<[u8; 4]>) -> String {
    let [r, g, b, a] = rgba.unwrap_or([0, 0, 0, 255]);
    format!(
        r#"{attr}="rgb({r},{g},{b})" {attr}-opacity="{:.3}""#,
        f32::from(a) / 255.0
    )
}

fn push_line(s: &mut String, line: &SceneStroke, rgba: Option<[u8; 4]>) {
    s.push_str(&format!(
        r#"<line x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}" {} stroke-width="{:.3}" stroke-linecap="round"/>"#,
        line.a.x,
        line.a.y,
        line.b.x,
        line.b.y,
        paint("stroke", rgba),
        line.width
    ));
}

fn push_polygon(s: &mut String, points: &[egui::Pos2], rgba: Option<[u8; 4]>) {
    let points: Vec<String> = points
        .iter()
        .map(|p| format!("{:.2},{:.2}", p.x, p.y))
        .collect();
    s.push_str(&format!(
        r#"<polygon points="{}" {}/>"#,
        points.join(" "),
        paint("fill", rgba)
    ));
}

/// Each glyph of `galley` at its place in the layout, `base` being the galley's top left.
fn push_glyphs(s: &mut String, galley: &RichGalley, base: egui::Pos2) {
    for g in galley.glyph_placements() {
        let ch = xml_escape(g.chr.encode_utf8(&mut [0; 4]));
        let family = match g.font {
            Some(font) => format!(
                "'{}', sans-serif",
                xml_escape(&font.as_str().replace('\'', ""))
            ),
            None => "sans-serif".to_owned(),
        };
        let italic = if g.italic {
            r#" font-style="italic""#
        } else {
            ""
        };
        let fill = paint("fill", Some(g.color.to_srgba_unmultiplied()));
        let y = base.y + g.baseline_y;
        // A faux-bold glyph is struck twice, as egui does on screen.
        for dx in [Some(0.0), g.bold_offset].into_iter().flatten() {
            s.push_str(&format!(
                r#"<text x="{:.3}" y="{y:.3}" font-size="{:.3}" font-family="{family}"{italic} {fill} xml:space="preserve">{ch}</text>"#,
                base.x + g.x + dx,
                g.size
            ));
        }
    }
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use acauda_embed::{HandlerMessage, HostListener, HostMessage, HostSession, Mode, OleStream};
    use chembuider_rs::BondOrder;

    const WAIT: Duration = Duration::from_secs(20);

    fn ethanol() -> Molecule {
        let mut mol = Molecule::default();
        let c1 = mol.add_atom("C".to_owned(), [0.0, 0.0], 0);
        let c2 = mol.add_atom("C".to_owned(), [1.3, 0.75], 0);
        let o = mol.add_atom("O".to_owned(), [2.6, 0.0], 0);
        mol.add_bond(c1, c2, BondOrder::Single);
        mol.add_bond(c2, o, BondOrder::Single);
        mol
    }

    fn cdx() -> Vec<u8> {
        io::encode(Format::Cdx, &ethanol(), None).unwrap()
    }

    fn sample() -> Vec<u8> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/chemdraw26_benzene.cdx");
        std::fs::read(path).unwrap()
    }

    #[test]
    fn new_objects_are_empty_chemdraw_objects_with_a_picture() {
        let new = Requests.new_object().unwrap();
        assert_eq!(new.content.prog_id, "ChemDraw_x64.Document.6.0");
        // In the byte order of the compound file.
        assert_eq!(new.content.clsid.0[..4], [0x21, 0x6D, 0xBA, 0x41]);
        assert_eq!(new.content.user_type, CHEMDRAW_USER_TYPE);
        let loaded = io::decode(Format::Cdx, drawing(&new.content).unwrap()).unwrap();
        assert!(loaded.molecule.is_blank());
        assert!(!new.preview.is_empty());
        assert_eq!(new.size_emu, Some((914_400, 457_200)));
    }

    #[test]
    fn cdx_is_imported_and_exported_as_it_is() {
        let file = sample();
        let new = Requests.import("CDX", file.clone()).unwrap();
        assert_eq!(new.content.stream(CONTENTS), Some(&file[..]));
        let (w, h) = new.size_emu.unwrap();
        assert!(w > 20 * 12_700 && h > 20 * 12_700, "{w} × {h}");
        assert_eq!(Requests.export(new.content, "cdx").unwrap(), file);
    }

    #[test]
    fn cdxml_goes_both_ways() {
        let new = Requests.import("cdx", sample()).unwrap();
        let atoms = io::decode(Format::Cdx, drawing(&new.content).unwrap())
            .unwrap()
            .molecule
            .atoms
            .len();
        let xml = Requests.export(new.content, "cdxml").unwrap();
        assert!(String::from_utf8_lossy(&xml).contains("<CDXML"));
        let back = Requests.import("cdxml", xml).unwrap();
        let loaded = io::decode(Format::Cdx, drawing(&back.content).unwrap()).unwrap();
        assert_eq!(loaded.molecule.atoms.len(), atoms);
    }

    #[test]
    fn what_cannot_be_read_is_refused() {
        assert!(Requests.import("cdx", b"junk".to_vec()).is_err());
        assert!(Requests.import("mol", Vec::new()).is_err());
        let new = Requests.new_object().unwrap();
        assert!(Requests.export(new.content, "png").is_err());
        assert!(Requests.export(OleContent::default(), "cdx").is_err());
    }

    #[test]
    fn an_object_without_a_drawing_gets_a_blank_picture() {
        let (picture, size) = Requests.render(OleContent::default()).unwrap();
        assert!(!picture.is_empty());
        assert_eq!(size, Some((914_400, 457_200)));
    }

    #[test]
    fn the_svg_draws_bonds_and_labels_at_chemdraw_s_scale() {
        let (svg, px) = svg(&ethanol());
        assert!(svg.starts_with("<svg") && svg.ends_with("</svg>"), "{svg}");
        assert_eq!(svg.matches("<line").count(), 2);
        assert!(svg.contains(">O</text>"), "{svg}");
        assert!(px[0] > 2.6 * SCALE_FACTOR, "{px:?}");
    }

    #[test]
    fn the_svg_of_nothing_is_a_blank_picture() {
        let (svg, px) = svg(&Molecule::default());
        assert!(svg.starts_with("<svg") && svg.ends_with("/>"), "{svg}");
        assert_eq!(px, BLANK_PX);
    }

    #[test]
    fn text_is_escaped_in_the_svg() {
        let mut mol = Molecule::default();
        mol.add_text([0.0, 0.0], "a<b&c");
        let (svg, _) = svg(&mol);
        assert!(
            svg.contains(">&lt;</text>") && svg.contains(">&amp;</text>"),
            "{svg}"
        );
    }

    /// The bridge of a running [`serve`], the thread and the host connected to it.
    struct Connected {
        host: HostSession,
        bridge: AppBridge,
        thread: JoinHandle<()>,
    }

    impl Connected {
        fn new(first_object_timeout: Duration) -> Self {
            let listener = HostListener::bind().unwrap();
            let args = HandlerArgs {
                socket: listener.name().to_owned(),
                token: listener.token().to_owned(),
                mode: Mode::Interactive,
            };
            let app = format!(
                "test.odc.{}.{:?}",
                std::process::id(),
                std::thread::current().id()
            );
            let Start::Interactive(editor) =
                editor::start_with(&app, Some(args), Requests).unwrap()
            else {
                panic!("not the window")
            };
            let host = listener.accept(WAIT, None).unwrap();
            let (bridge, side) = ole_server::channel(Box::new(|| {}), true);
            let thread = std::thread::spawn(move || serve(editor, side, first_object_timeout));
            Self {
                host,
                bridge,
                thread,
            }
        }

        fn next(&self) -> ToApp {
            let end = Instant::now() + WAIT;
            loop {
                if let Some(message) = self.bridge.try_recv() {
                    return message;
                }
                assert!(Instant::now() < end, "nothing from the server");
                std::thread::sleep(Duration::from_millis(5));
            }
        }

        fn open(&mut self, content: OleContent) -> ToApp {
            self.host
                .send(&HostMessage::Open {
                    content,
                    title: "doc.pptx".into(),
                    locale: "ja".into(),
                })
                .unwrap();
            self.next()
        }

        fn shutdown(self) -> HostSession {
            self.bridge.send(ToServer::Shutdown);
            self.thread.join().unwrap();
            assert!(self.bridge.try_recv().is_none());
            self.host
        }
    }

    fn object() -> OleContent {
        let mut content = OleContent {
            clsid: Clsid([7; 16]),
            prog_id: "ChemDraw.Document.6.0".into(),
            user_type: "Theirs".into(),
            streams: vec![OleStream {
                path: "Other/Data".into(),
                bytes: vec![1, 2],
            }],
        };
        content.set_stream(CONTENTS, cdx());
        content
    }

    #[test]
    fn an_object_goes_to_the_app_and_comes_back_as_it_came() {
        let mut c = Connected::new(WAIT);
        let ToApp::Open {
            id,
            cdx: opened,
            host,
        } = c.open(object())
        else {
            panic!("not opened")
        };
        assert_eq!(opened, Some(cdx()));
        assert_eq!(
            (host.app.as_str(), host.document.as_str()),
            (HOST_APP, "doc.pptx")
        );

        let mut mol = ethanol();
        mol.add_atom("N".to_owned(), [4.0, 0.0], 0);
        let edited = io::encode(Format::Cdx, &mol, None).unwrap();
        c.bridge.send(ToServer::Update {
            id,
            cdx: edited.clone(),
        });
        let HandlerMessage::Update {
            content,
            preview,
            size_emu,
        } = c.host.recv().unwrap()
        else {
            panic!("no update")
        };
        assert_eq!(content.stream(CONTENTS), Some(&edited[..]));
        assert_eq!(content.stream("Other/Data"), Some(&[1u8, 2][..]));
        let original = object();
        assert_eq!(
            (content.clsid, &content.prog_id, &content.user_type),
            (original.clsid, &original.prog_id, &original.user_type)
        );
        assert!(!preview.is_empty());
        assert!(size_emu.is_some());

        // The tab closes: the host is told, and nothing comes back to a tab that has gone.
        c.bridge.send(ToServer::Closed { id });
        assert_eq!(c.host.recv().unwrap(), HandlerMessage::Closed);
        c.shutdown();
    }

    #[test]
    fn the_host_closing_an_object_closes_its_tab() {
        let mut c = Connected::new(WAIT);
        let ToApp::Open { id, cdx, .. } = c.open(OleContent::default()) else {
            panic!("not opened")
        };
        assert_eq!(cdx, None, "a new object");
        c.host.send(&HostMessage::Close).unwrap();
        assert!(matches!(
            c.next(),
            ToApp::Close { id: closed, reply: None } if closed == id
        ));
        c.shutdown();
    }

    #[test]
    fn the_window_closing_tells_the_host() {
        let mut c = Connected::new(WAIT);
        c.open(object());
        let mut host = c.shutdown();
        assert_eq!(host.recv().unwrap(), HandlerMessage::Closed);
    }

    #[test]
    fn a_host_that_never_opens_an_object_leaves_the_app_idle() {
        let c = Connected::new(Duration::ZERO);
        assert!(matches!(c.next(), ToApp::Idle));
        c.shutdown();
    }
}
