//! Embedded 3D models: what an OLE object of this app's 3D class ([`super::CLSID_3D`]) holds,
//! and its picture. Platform independent, so the payload is tested anywhere; the COM server
//! ([`super::com`]) and the clipboard (`crate::ole`) write it into a storage.
//!
//! The object's storage, as the 2D objects' (class id, `\x01CompObj`, the native data in
//! `CONTENTS`), plus two streams of its own:
//!
//! - [`CONTENTS`]: the model as Tripos MOL2 (UTF-8): coordinates, atom types, bonds. The
//!   native data, written and read by molecule_optimizer_rs as the 3D export and open do.
//! - [`VIEW_STREAM`]: how the model was shown, as JSON ([`Look3d`]): the style, the camera,
//!   the view's size and the picture's size on the page. Kept apart from the MOL2 so the
//!   MOL2 stays a plain file any program reads.
//! - [`PICTURE_STREAM`]: the picture as a PNG with a transparent background. The server hands
//!   out the container's presentation from it (an EMF with an alpha-blended bitmap, see
//!   `super::picture`) without having to draw the model, which only the app's window can.

use serde::{Deserialize, Serialize};

use crate::three_d::read::{self, Format3d, Opened3d};
use crate::three_d::{self, Export3d, Model3d};
use crate::ui::viewport3d::{CameraState, Picture, Style3d, encode_png};

/// The stream holding the MOL2.
pub const CONTENTS: &str = "CONTENTS";
/// The stream holding [`Look3d`] as JSON.
pub const VIEW_STREAM: &str = "ODC3DView";
/// The stream holding the picture (PNG).
pub const PICTURE_STREAM: &str = "ODC3DPicture";
/// The clipboard format the MOL2 is offered in, and the native format named in `\x01CompObj`.
pub const MOL2_FORMAT_NAME: &str = "chemical/x-mol2";

/// The pixels a point of the view spans in an object's picture: about 190 dpi on a 100 %
/// display, sharp on the page without making the document heavy.
pub const PX_PER_POINT: f32 = 2.0;
/// The longest side of the view drawn for a picture, in pixels (the picture is cropped from it).
/// The container's EMF carries the bitmap uncompressed, so this bounds its size.
pub const MAX_PICTURE_PX: f32 = 1600.0;
/// The largest picture on the page, in HIMETRIC (0.01 mm), on its longer side.
const MAX_EXTENT: i32 = 15_000;
/// The smallest, on either side.
const MIN_EXTENT: i32 = 200;
/// HIMETRIC per point of the view (a point is 1/96 inch on a 100 % display).
const HIMETRIC_PER_POINT: f32 = 2540.0 / 96.0;
/// Clear pixels kept around the model when the picture is cropped.
const CROP_MARGIN_PX: u32 = 8;

/// How the model was shown: enough to draw it again as it was.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Look3d {
    /// The layout of this record; raised when it changes incompatibly.
    pub version: u32,
    pub style: Style3d,
    /// `None` frames the model as a newly loaded one is.
    pub camera: Option<CameraState>,
    /// The view's size in points when the picture was taken (its aspect is the camera's).
    pub view_points: [f32; 2],
    /// The picture's size on the page, in HIMETRIC.
    pub extent: [i32; 2],
}

impl Default for Look3d {
    fn default() -> Self {
        Self {
            version: 1,
            style: Style3d::default(),
            camera: None,
            view_points: [480.0, 360.0],
            extent: [5080, 3810],
        }
    }
}

impl Look3d {
    /// The look of a view showing `style` from `camera`, `points` large.
    pub fn of_view(style: Style3d, camera: Option<CameraState>, points: [f32; 2]) -> Self {
        let mut look = Self {
            style,
            camera,
            ..Self::default()
        };
        if points.iter().all(|p| p.is_finite() && *p >= 1.0) {
            look.view_points = points;
        }
        look
    }

    /// Whether `other` shows the model differently: another style, or the camera elsewhere
    /// by more than rounding. The view's size and the picture's do not count (they follow the
    /// window).
    pub fn differs_visibly(&self, other: &Look3d) -> bool {
        let near = |a: f32, b: f32| (a - b).abs() <= 1e-4 * (1.0 + a.abs().max(b.abs()));
        let same_camera = match (&self.camera, &other.camera) {
            (None, None) => true,
            (Some(a), Some(b)) => {
                // q and -q are the same turn.
                let dot: f32 = a.rotation.iter().zip(&b.rotation).map(|(x, y)| x * y).sum();
                let sign = if dot < 0.0 { -1.0 } else { 1.0 };
                a.center.iter().zip(&b.center).all(|(x, y)| near(*x, *y))
                    && near(a.radius, b.radius)
                    && a.rotation
                        .iter()
                        .zip(&b.rotation)
                        .all(|(x, y)| near(*x, sign * y))
            }
            _ => false,
        };
        self.style != other.style || !same_camera
    }

    /// The pixels a point spans when the view is drawn for a picture: [`PX_PER_POINT`], less
    /// for a view so large that it would pass [`MAX_PICTURE_PX`].
    pub fn picture_px_per_point(&self) -> f32 {
        let longest = self.view_points[0].max(self.view_points[1]).max(1.0);
        PX_PER_POINT.min(MAX_PICTURE_PX / longest)
    }
}

/// An embedded 3D model: the native data and the look, and the picture.
#[derive(Debug, Clone, PartialEq)]
pub struct Object3d {
    /// The model as Tripos MOL2.
    pub mol2: String,
    pub look: Look3d,
    /// The picture as a PNG with a transparent background; empty when none was drawn (the
    /// container keeps showing the one it has).
    pub png: Vec<u8>,
}

impl Object3d {
    /// `model` shown as `look`, with `picture` (drawn from that look) cropped to the model.
    pub fn new(
        model: &Model3d,
        look: Look3d,
        picture: Option<&Picture>,
    ) -> Result<Self, three_d::Error> {
        let mut object = Self {
            mol2: three_d::export(model, Export3d::Mol2)?,
            look,
            png: Vec::new(),
        };
        if let Some(picture) = picture {
            object.set_picture(picture);
        }
        Ok(object)
    }

    /// Take `picture`, cropped to the model, as the object's picture, and size the object on
    /// the page from it. A picture that cannot be encoded leaves the old one.
    pub fn set_picture(&mut self, picture: &Picture) {
        let cropped = crop(picture);
        if let Ok(png) = encode_png(cropped.rgba.clone(), cropped.width, cropped.height) {
            self.look.extent = extent_of(cropped.width, cropped.height, cropped.px_per_point);
            self.png = png;
        }
    }

    /// The model and a 2D depiction of it, read back from the MOL2.
    pub fn open(&self) -> Result<Opened3d, read::Error> {
        read::read(Format3d::Mol2, &self.mol2)
    }

    /// The streams of the object's storage, by name (the class id and `\x01CompObj` are the
    /// writer's). The picture's stream is left out when there is no picture.
    pub fn streams(&self) -> Vec<(&'static str, Vec<u8>)> {
        let mut streams = vec![
            (CONTENTS, self.mol2.as_bytes().to_vec()),
            (
                VIEW_STREAM,
                serde_json::to_vec_pretty(&self.look).unwrap_or_default(),
            ),
        ];
        if !self.png.is_empty() {
            streams.push((PICTURE_STREAM, self.png.clone()));
        }
        streams
    }

    /// The object stored in streams that `read` reads by name. `None` without a MOL2; a look
    /// that is missing or unreadable is the default one (the model is framed).
    pub fn from_streams(read: impl Fn(&str) -> Option<Vec<u8>>) -> Option<Self> {
        let contents = read(CONTENTS)?;
        let mol2 = String::from_utf8_lossy(trim_nul(&contents)).into_owned();
        if mol2.trim().is_empty() {
            return None;
        }
        let look = read(VIEW_STREAM)
            .and_then(|json| serde_json::from_slice::<Look3d>(trim_nul(&json)).ok())
            .map(|mut look| {
                look.camera = look.camera.filter(CameraState::is_usable);
                look.extent = look.extent.map(|e| e.clamp(MIN_EXTENT, 10 * MAX_EXTENT));
                look
            })
            .unwrap_or_default();
        Some(Self {
            mol2,
            look,
            png: read(PICTURE_STREAM).unwrap_or_default(),
        })
    }

    /// The picture's pixels; `None` when there is none or it does not decode.
    pub fn pixels(&self) -> Option<Pixels> {
        if self.png.is_empty() {
            return None;
        }
        let image = image::load_from_memory_with_format(&self.png, image::ImageFormat::Png)
            .ok()?
            .into_rgba8();
        let (width, height) = image.dimensions();
        Some(Pixels {
            width,
            height,
            rgba: image.into_raw(),
        })
    }
}

/// The "Object Descriptor" (an `OBJECTDESCRIPTOR` and its strings) of a 3D object `extent`
/// (HIMETRIC) large: Office needs it to paste the object as one.
pub fn object_descriptor(extent: [i32; 2]) -> Vec<u8> {
    let utf16z = |s: &str| -> Vec<u8> {
        s.encode_utf16()
            .chain([0])
            .flat_map(u16::to_le_bytes)
            .collect()
    };
    let full = utf16z(super::compobj::USER_TYPE_3D);
    let source = utf16z(crate::APP_NAME);
    // cbSize, clsid, dwDrawAspect, sizel, pointl, dwStatus, dwFullUserTypeName, dwSrcOfCopy.
    const FIXED: usize = 52;
    let size = FIXED + full.len() + source.len();
    let mut out = Vec::with_capacity(size);
    out.extend((size as u32).to_le_bytes());
    out.extend(super::compobj::guid_bytes(super::CLSID_3D));
    out.extend(1u32.to_le_bytes()); // DVASPECT_CONTENT
    out.extend(extent[0].to_le_bytes());
    out.extend(extent[1].to_le_bytes());
    out.extend([0u8; 8]); // pointl
    out.extend(16u32.to_le_bytes()); // OLEMISC_CANTLINKINSIDE, as registered
    out.extend((FIXED as u32).to_le_bytes());
    out.extend(((FIXED + full.len()) as u32).to_le_bytes());
    out.extend(full);
    out.extend(source);
    out
}

/// A block of memory read from a stream or a clipboard HGLOBAL may have a NUL after the text.
fn trim_nul(bytes: &[u8]) -> &[u8] {
    let end = bytes.iter().rposition(|&b| b != 0).map_or(0, |i| i + 1);
    &bytes[..end]
}

/// A picture's pixels, straight (not premultiplied) RGBA8 from the top row.
#[derive(Debug, Clone, PartialEq)]
pub struct Pixels {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Pixels {
    /// Bottom-up premultiplied BGRA, as a 32-bit DIB for `AlphaBlend` wants it.
    pub fn premultiplied_bgra_bottom_up(&self) -> Vec<u8> {
        self.bottom_up(|[r, g, b, a]| {
            let premultiply = |c: u8| ((u32::from(c) * u32::from(a) + 127) / 255) as u8;
            [premultiply(b), premultiply(g), premultiply(r), a]
        })
    }

    /// Bottom-up BGRA laid over white and opaque, for the pictures that have no alpha (a
    /// Windows metafile, a plain DIB).
    pub fn over_white_bgra_bottom_up(&self) -> Vec<u8> {
        self.bottom_up(|[r, g, b, a]| {
            let over = |c: u8| {
                ((u32::from(c) * u32::from(a) + 255 * (255 - u32::from(a)) + 127) / 255) as u8
            };
            [over(b), over(g), over(r), 255]
        })
    }

    fn bottom_up(&self, pixel: impl Fn([u8; 4]) -> [u8; 4]) -> Vec<u8> {
        let row = self.width as usize * 4;
        let mut out = Vec::with_capacity(self.rgba.len());
        for line in self.rgba.chunks_exact(row).rev() {
            for px in line.as_chunks::<4>().0 {
                out.extend(pixel(*px));
            }
        }
        out
    }
}

/// `picture` cut down to the model and a margin of clear pixels: the view's empty space is not
/// part of the object. A picture with nothing drawn is kept whole.
pub fn crop(picture: &Picture) -> Picture {
    let (width, height) = (picture.width as usize, picture.height as usize);
    let alpha = |x: usize, y: usize| picture.rgba[(y * width + x) * 4 + 3];
    let mut bounds: Option<[usize; 4]> = None;
    for y in 0..height {
        for x in 0..width {
            if alpha(x, y) != 0 {
                let b = bounds.get_or_insert([x, y, x, y]);
                b[0] = b[0].min(x);
                b[1] = b[1].min(y);
                b[2] = b[2].max(x);
                b[3] = b[3].max(y);
            }
        }
    }
    let Some([x0, y0, x1, y1]) = bounds else {
        return picture.clone();
    };
    let margin = CROP_MARGIN_PX as usize;
    let (x0, y0) = (x0.saturating_sub(margin), y0.saturating_sub(margin));
    let (x1, y1) = ((x1 + margin).min(width - 1), (y1 + margin).min(height - 1));
    let (out_w, out_h) = (x1 - x0 + 1, y1 - y0 + 1);
    let mut rgba = Vec::with_capacity(out_w * out_h * 4);
    for y in y0..=y1 {
        let start = (y * width + x0) * 4;
        rgba.extend_from_slice(&picture.rgba[start..start + out_w * 4]);
    }
    Picture {
        rgba,
        width: out_w as u32,
        height: out_h as u32,
        px_per_point: picture.px_per_point,
    }
}

/// The size on the page (HIMETRIC) of a picture `width` × `height` pixels at `px_per_point`:
/// the size it had on screen, made smaller when its longer side would pass 15 cm.
pub fn extent_of(width: u32, height: u32, px_per_point: f32) -> [i32; 2] {
    let ppp = if px_per_point.is_finite() && px_per_point > 0.0 {
        px_per_point
    } else {
        PX_PER_POINT
    };
    let (mut cx, mut cy) = (
        width as f32 / ppp * HIMETRIC_PER_POINT,
        height as f32 / ppp * HIMETRIC_PER_POINT,
    );
    let longest = cx.max(cy);
    if longest > MAX_EXTENT as f32 {
        let shrink = MAX_EXTENT as f32 / longest;
        cx *= shrink;
        cy *= shrink;
    }
    [
        (cx.round() as i32).max(MIN_EXTENT),
        (cy.round() as i32).max(MIN_EXTENT),
    ]
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::three_d::ForceField;

    /// Ethanol with its hydrogens, at 3D coordinates.
    pub(crate) fn ethanol() -> Model3d {
        let molfile = [
            "ethanol",
            "  test",
            "",
            "  9  8  0  0  0  0  0  0  0  0999 V2000",
            "   -0.9300    0.0700    0.0100 C   0  0  0  0  0  0  0  0  0  0  0  0",
            "    0.5300   -0.3700   -0.0600 C   0  0  0  0  0  0  0  0  0  0  0  0",
            "    1.3200    0.7900    0.0800 O   0  0  0  0  0  0  0  0  0  0  0  0",
            "   -1.5400   -0.8300   -0.0900 H   0  0  0  0  0  0  0  0  0  0  0  0",
            "   -1.1500    0.7600   -0.8100 H   0  0  0  0  0  0  0  0  0  0  0  0",
            "   -1.1400    0.5600    0.9700 H   0  0  0  0  0  0  0  0  0  0  0  0",
            "    0.7400   -1.0600    0.7700 H   0  0  0  0  0  0  0  0  0  0  0  0",
            "    0.7300   -0.8700   -1.0200 H   0  0  0  0  0  0  0  0  0  0  0  0",
            "    2.2500    0.5000    0.0300 H   0  0  0  0  0  0  0  0  0  0  0  0",
            "  1  2  1  0",
            "  2  3  1  0",
            "  1  4  1  0",
            "  1  5  1  0",
            "  1  6  1  0",
            "  2  7  1  0",
            "  2  8  1  0",
            "  3  9  1  0",
            "M  END",
        ]
        .join("\n");
        crate::three_d::model_from_molfile(&molfile, ForceField::Uff)
    }

    /// A picture with one red pixel at (`x`, `y`) on a clear background.
    fn dot(width: u32, height: u32, x: u32, y: u32) -> Picture {
        let mut rgba = vec![0u8; (width * height * 4) as usize];
        let i = ((y * width + x) * 4) as usize;
        rgba[i..i + 4].copy_from_slice(&[255, 0, 0, 255]);
        Picture {
            rgba,
            width,
            height,
            px_per_point: 2.0,
        }
    }

    fn sorted_bonds(model: &Model3d) -> Vec<(usize, usize, u8)> {
        let mut bonds: Vec<_> = model
            .bonds
            .iter()
            .map(|b| (b.begin.min(b.end), b.begin.max(b.end), b.order))
            .collect();
        bonds.sort_unstable();
        bonds
    }

    fn assert_same_model(a: &Model3d, b: &Model3d) {
        assert_eq!(a.atoms.len(), b.atoms.len());
        for (x, y) in a.atoms.iter().zip(&b.atoms) {
            assert_eq!(x.element, y.element);
            for k in 0..3 {
                assert!((x.pos[k] - y.pos[k]).abs() < 1e-3, "{x:?} vs {y:?}");
            }
        }
        assert_eq!(sorted_bonds(a), sorted_bonds(b));
    }

    #[test]
    fn the_mol2_round_trips_atoms_bonds_and_coordinates() {
        let model = ethanol();
        let object = Object3d::new(&model, Look3d::default(), None).unwrap();
        assert!(object.mol2.contains("@<TRIPOS>MOLECULE"));
        assert!(object.mol2.contains("@<TRIPOS>ATOM"));
        assert!(object.mol2.contains("@<TRIPOS>BOND"));
        let opened = object.open().unwrap();
        let back = opened.model.expect("a 3D model");
        assert_same_model(&model, &back);
        // Written again from what was read, it reads the same once more.
        let again = Object3d::new(&back, Look3d::default(), None).unwrap();
        assert_same_model(&model, &again.open().unwrap().model.unwrap());
        // The depiction has the heavy atoms.
        assert_eq!(opened.drawing.atoms.len(), 3);
    }

    #[test]
    fn a_double_bond_survives_the_mol2() {
        // Acetaldehyde: CH3-CH=O.
        let molfile = [
            "acetaldehyde",
            "  test",
            "",
            "  7  6  0  0  0  0  0  0  0  0999 V2000",
            "   -0.9500    0.0300    0.0000 C   0  0  0  0  0  0  0  0  0  0  0  0",
            "    0.5200   -0.2800    0.0000 C   0  0  0  0  0  0  0  0  0  0  0  0",
            "    1.3400    0.6200    0.0000 O   0  0  0  0  0  0  0  0  0  0  0  0",
            "   -1.1800    1.1000    0.0000 H   0  0  0  0  0  0  0  0  0  0  0  0",
            "   -1.3900   -0.4300    0.8900 H   0  0  0  0  0  0  0  0  0  0  0  0",
            "   -1.3900   -0.4300   -0.8900 H   0  0  0  0  0  0  0  0  0  0  0  0",
            "    0.8200   -1.3400    0.0000 H   0  0  0  0  0  0  0  0  0  0  0  0",
            "  1  2  1  0",
            "  2  3  2  0",
            "  1  4  1  0",
            "  1  5  1  0",
            "  1  6  1  0",
            "  2  7  1  0",
            "M  END",
        ]
        .join("\n");
        let model = crate::three_d::model_from_molfile(&molfile, ForceField::Uff);
        assert!(model.bonds.iter().any(|b| b.order == 2));
        let object = Object3d::new(&model, Look3d::default(), None).unwrap();
        let back = object.open().unwrap().model.unwrap();
        assert_same_model(&model, &back);
    }

    #[test]
    fn the_streams_round_trip_the_look_and_the_picture() {
        let look = Look3d {
            style: Style3d::Spheres,
            camera: Some(CameraState {
                center: [0.1, -0.2, 0.3],
                rotation: [0.9, 0.1, 0.3, -0.2],
                radius: 2.5,
            }),
            view_points: [640.0, 400.0],
            ..Look3d::default()
        };
        let mut object = Object3d::new(&ethanol(), look.clone(), None).unwrap();
        object.set_picture(&dot(40, 30, 20, 10));
        let streams = object.streams();
        let names: Vec<&str> = streams.iter().map(|(n, _)| *n).collect();
        assert_eq!(names, [CONTENTS, VIEW_STREAM, PICTURE_STREAM]);
        let read = |name: &str| {
            streams
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, b)| b.clone())
        };
        let back = Object3d::from_streams(read).unwrap();
        assert_eq!(back, object);
        assert_eq!(back.look.style, Style3d::Spheres);
        assert_eq!(back.look.camera, look.camera);

        // The JSON is readable as such (a version and the style by name).
        let json = String::from_utf8(read(VIEW_STREAM).unwrap()).unwrap();
        assert!(json.contains("\"style\": \"Spheres\""), "{json}");
        assert!(json.contains("\"version\": 1"), "{json}");
    }

    #[test]
    fn a_missing_or_broken_look_frames_the_model_and_no_mol2_is_no_object() {
        let mol2 = Object3d::new(&ethanol(), Look3d::default(), None)
            .unwrap()
            .mol2;
        let mut with_nul = mol2.clone().into_bytes();
        with_nul.push(0);
        let only_contents = |name: &str| (name == CONTENTS).then(|| with_nul.clone());
        let object = Object3d::from_streams(only_contents).unwrap();
        assert_eq!(object.mol2, mol2, "the trailing NUL is dropped");
        assert_eq!(object.look, Look3d::default());
        assert!(object.png.is_empty() && object.pixels().is_none());

        let broken = |name: &str| match name {
            CONTENTS => Some(mol2.clone().into_bytes()),
            VIEW_STREAM => Some(
                b"{\"camera\": {\"center\": [0,0,0], \"rotation\": [0,0,0,0], \"radius\": 1}}"
                    .to_vec(),
            ),
            _ => None,
        };
        assert_eq!(Object3d::from_streams(broken).unwrap().look.camera, None);
        assert_eq!(Object3d::from_streams(|_| None), None);
        assert_eq!(Object3d::from_streams(|_| Some(b"  \n".to_vec())), None);
    }

    #[test]
    fn the_picture_is_cropped_to_the_model_and_keeps_its_alpha() {
        let picture = dot(100, 80, 50, 40);
        let cropped = crop(&picture);
        let side = 2 * CROP_MARGIN_PX + 1;
        assert_eq!((cropped.width, cropped.height), (side, side));
        let mut object = Object3d::new(&ethanol(), Look3d::default(), None).unwrap();
        object.set_picture(&picture);
        let pixels = object.pixels().unwrap();
        assert_eq!((pixels.width, pixels.height), (side, side));
        let at = |x: u32, y: u32| {
            let i = ((y * pixels.width + x) * 4) as usize;
            [
                pixels.rgba[i],
                pixels.rgba[i + 1],
                pixels.rgba[i + 2],
                pixels.rgba[i + 3],
            ]
        };
        let c = CROP_MARGIN_PX;
        assert_eq!(at(c, c), [255, 0, 0, 255], "the model");
        assert_eq!(at(0, 0)[3], 0, "the background stays transparent");
        // Its size on the page follows the pixels: 17 px at 2 px a point is 8.5 points.
        let expected = (side as f32 / 2.0 * HIMETRIC_PER_POINT).round() as i32;
        assert_eq!(object.look.extent, [expected.max(MIN_EXTENT); 2]);

        // Nothing drawn: kept whole.
        let empty = Picture {
            rgba: vec![0; 16],
            width: 2,
            height: 2,
            px_per_point: 1.0,
        };
        assert_eq!(crop(&empty), empty);
    }

    #[test]
    fn a_large_picture_is_at_most_15_cm_on_the_page() {
        assert_eq!(extent_of(960, 480, 1.0), [15_000, 7_500]);
        let [cx, cy] = extent_of(192, 96, 2.0);
        assert_eq!((cx, cy), (2540, 1270));
        assert_eq!(extent_of(1, 1, f32::NAN), [MIN_EXTENT, MIN_EXTENT]);
    }

    #[test]
    fn pixels_convert_for_alpha_blending_and_for_opaque_pictures() {
        // Two pixels on one row over one below: half-transparent blue, clear; opaque green, clear.
        let pixels = Pixels {
            width: 2,
            height: 2,
            rgba: vec![0, 0, 255, 128, 0, 0, 0, 0, 0, 255, 0, 255, 9, 9, 9, 0],
        };
        let pre = pixels.premultiplied_bgra_bottom_up();
        // Bottom row first.
        assert_eq!(&pre[0..8], &[0, 255, 0, 255, 0, 0, 0, 0]);
        assert_eq!(&pre[8..12], &[128, 0, 0, 128]);
        let white = pixels.over_white_bgra_bottom_up();
        assert_eq!(&white[4..8], &[255, 255, 255, 255], "clear is white");
        assert_eq!(&white[8..12], &[255, 127, 127, 255]);
    }

    #[test]
    fn only_the_style_and_a_moved_camera_change_the_look() {
        let camera = CameraState {
            center: [0.1, 0.2, 0.3],
            rotation: [0.6, 0.0, 0.8, 0.0],
            radius: 3.0,
        };
        let look = Look3d::of_view(Style3d::BallAndStick, Some(camera), [400.0, 300.0]);
        let mut other = look.clone();
        other.view_points = [800.0, 600.0];
        other.extent = [1, 1];
        assert!(!look.differs_visibly(&other), "the size is the window's");
        other.camera = Some(CameraState {
            rotation: [-0.6, 0.0, -0.8 - 1e-7, 0.0],
            ..camera
        });
        assert!(!look.differs_visibly(&other), "the same turn, rounded");
        other.camera = Some(CameraState {
            radius: 3.5,
            ..camera
        });
        assert!(look.differs_visibly(&other));
        other.camera = None;
        assert!(look.differs_visibly(&other));
        let mut styled = look.clone();
        styled.style = Style3d::Spheres;
        assert!(look.differs_visibly(&styled));
    }

    #[test]
    fn the_object_descriptor_names_the_3d_class_and_its_size() {
        let od = object_descriptor([4000, 3000]);
        let word = |at: usize| u32::from_le_bytes(od[at..at + 4].try_into().unwrap());
        assert_eq!(word(0) as usize, od.len());
        assert_eq!(
            &od[4..20],
            &super::super::compobj::guid_bytes(super::super::CLSID_3D)
        );
        assert_eq!(word(20), 1);
        assert_eq!((word(24), word(28)), (4000, 3000));
        let name: Vec<u16> = od[word(44) as usize..word(48) as usize]
            .chunks(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .take_while(|&u| u != 0)
            .collect();
        assert_eq!(
            String::from_utf16(&name).unwrap(),
            super::super::compobj::USER_TYPE_3D
        );
    }

    #[test]
    fn a_view_too_large_is_drawn_at_fewer_pixels_a_point() {
        let small = Look3d::of_view(Style3d::Wireframe, None, [400.0, 300.0]);
        assert_eq!(small.picture_px_per_point(), PX_PER_POINT);
        let large = Look3d::of_view(Style3d::Wireframe, None, [1600.0, 900.0]);
        assert_eq!(large.picture_px_per_point(), 1.0);
        // A size that is no size keeps the default.
        let odd = Look3d::of_view(Style3d::Wireframe, None, [f32::NAN, 0.0]);
        assert_eq!(odd.view_points, Look3d::default().view_points);
    }
}
