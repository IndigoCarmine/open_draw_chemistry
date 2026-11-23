//! The 3D object's pictures for Windows containers, made from its PNG
//! ([`super::model3d::Object3d::pixels`]).
//!
//! An embedded object is shown from the presentation the container caches: a metafile. Of the
//! pictures OLE can cache, only an enhanced metafile keeps a transparent background: the bitmap
//! is drawn into it with `AlphaBlend`, recorded as an `EMR_ALPHABLEND` with the 32-bit
//! premultiplied pixels, which Word, PowerPoint and Excel blend over the page. A Windows
//! metafile (`CF_METAFILEPICT`) and a plain `CF_DIB` have no alpha, so for the containers that
//! cache those the picture is laid over white instead. (A "PNG" would keep the alpha too, but
//! with one on the clipboard Word pastes the picture rather than the object.)

use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, AlphaBlend, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION,
    CloseEnhMetaFile, CreateCompatibleDC, CreateDIBSection, CreateEnhMetaFileW, DIB_RGB_COLORS,
    DeleteDC, DeleteEnhMetaFile, DeleteObject, GetDC, GetDeviceCaps, HALFTONE, HDC, HENHMETAFILE,
    HORZRES, HORZSIZE, ReleaseDC, SRCCOPY, SelectObject, SetStretchBltMode, StretchDIBits, VERTRES,
    VERTSIZE,
};
use windows::core::{Error, PCWSTR, Result};

use super::model3d::Pixels;

/// An enhanced metafile, deleted on drop unless [`Emf::into_raw`] hands it on.
pub struct Emf(pub HENHMETAFILE);

impl Emf {
    /// The handle, now the receiver's to delete.
    pub fn into_raw(self) -> HENHMETAFILE {
        let handle = self.0;
        std::mem::forget(self);
        handle
    }
}

impl Drop for Emf {
    fn drop(&mut self) {
        // SAFETY: a metafile this value owns, deleted once.
        unsafe {
            let _ = DeleteEnhMetaFile(Some(self.0));
        }
    }
}

/// The picture with its transparent background: an EMF `extent` (HIMETRIC) large holding the
/// pixels as one alpha-blended bitmap.
pub fn alpha_emf(pixels: &Pixels, extent: [i32; 2]) -> Result<Emf> {
    refuse_empty(pixels)?;
    let bits = pixels.premultiplied_bgra_bottom_up();
    record(pixels, extent, |dc, dest, info| {
        // SAFETY: a memory DC holding a DIB section of the picture's size, filled from `bits`
        // (exactly its size) and freed before returning.
        unsafe {
            let memory = CreateCompatibleDC(Some(dc));
            let mut target = std::ptr::null_mut();
            let bitmap =
                match CreateDIBSection(Some(memory), info, DIB_RGB_COLORS, &mut target, None, 0) {
                    Ok(bitmap) => bitmap,
                    Err(e) => {
                        let _ = DeleteDC(memory);
                        return Err(e);
                    }
                };
            std::ptr::copy_nonoverlapping(bits.as_ptr(), target.cast::<u8>(), bits.len());
            let old = SelectObject(memory, bitmap.into());
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            let (w, h) = (pixels.width as i32, pixels.height as i32);
            let ok = AlphaBlend(dc, 0, 0, dest.0, dest.1, memory, 0, 0, w, h, blend).as_bool();
            SelectObject(memory, old);
            let _ = DeleteObject(bitmap.into());
            let _ = DeleteDC(memory);
            if ok {
                Ok(())
            } else {
                Err(Error::from_thread())
            }
        }
    })
}

/// The picture laid over white, for the pictures without alpha: an EMF `extent` (HIMETRIC)
/// large holding one opaque bitmap (what `CF_METAFILEPICT` is converted from).
pub fn opaque_emf(pixels: &Pixels, extent: [i32; 2]) -> Result<Emf> {
    refuse_empty(pixels)?;
    let bits = pixels.over_white_bgra_bottom_up();
    record(pixels, extent, |dc, dest, info| {
        let (w, h) = (pixels.width as i32, pixels.height as i32);
        // SAFETY: `bits` holds exactly the bitmap `info` describes.
        let lines = unsafe {
            SetStretchBltMode(dc, HALFTONE);
            StretchDIBits(
                dc,
                0,
                0,
                dest.0,
                dest.1,
                0,
                0,
                w,
                h,
                Some(bits.as_ptr().cast()),
                info,
                DIB_RGB_COLORS,
                SRCCOPY,
            )
        };
        if lines == 0 {
            Err(Error::from_thread())
        } else {
            Ok(())
        }
    })
}

/// `CF_DIB`: a packed 32-bit DIB (header and bottom-up pixels) of the picture over white.
pub fn dib(pixels: &Pixels) -> Vec<u8> {
    let header = info_header(pixels);
    // SAFETY: a plain-old-data struct viewed as its bytes.
    let head = unsafe {
        std::slice::from_raw_parts(
            (&raw const header).cast::<u8>(),
            size_of::<BITMAPINFOHEADER>(),
        )
    };
    let mut out = head.to_vec();
    out.extend(pixels.over_white_bgra_bottom_up());
    out
}

/// An error for a picture without pixels (there is nothing to draw).
fn refuse_empty(pixels: &Pixels) -> Result<()> {
    if pixels.width == 0 || pixels.height == 0 {
        Err(Error::from_hresult(
            windows::Win32::Foundation::E_INVALIDARG,
        ))
    } else {
        Ok(())
    }
}

fn info_header(pixels: &Pixels) -> BITMAPINFOHEADER {
    BITMAPINFOHEADER {
        biSize: size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: pixels.width as i32,
        // Positive: bottom-up rows.
        biHeight: pixels.height as i32,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB.0,
        biSizeImage: pixels.width * pixels.height * 4,
        ..Default::default()
    }
}

/// Record an EMF framed `extent` (HIMETRIC) with `draw`, which is given the metafile's DC, the
/// frame's size in the reference device's pixels and the bitmap header of `pixels`.
fn record(
    pixels: &Pixels,
    extent: [i32; 2],
    draw: impl FnOnce(HDC, (i32, i32), &BITMAPINFO) -> Result<()>,
) -> Result<Emf> {
    refuse_empty(pixels)?;
    let frame = RECT {
        left: 0,
        top: 0,
        right: extent[0].max(1),
        bottom: extent[1].max(1),
    };
    let info = BITMAPINFO {
        bmiHeader: info_header(pixels),
        ..Default::default()
    };
    // SAFETY: a memory metafile against the screen (its reference device), drawn and closed
    // here; the screen DC is released.
    unsafe {
        let screen = GetDC(None);
        // The frame in the screen's pixels (its size in mm, as GDI sizes metafiles).
        let per_mm = |res, size| {
            f64::from(GetDeviceCaps(Some(screen), res))
                / f64::from(GetDeviceCaps(Some(screen), size)).max(1.0)
        };
        let dest = (
            ((f64::from(frame.right) / 100.0 * per_mm(HORZRES, HORZSIZE)).round() as i32).max(1),
            ((f64::from(frame.bottom) / 100.0 * per_mm(VERTRES, VERTSIZE)).round() as i32).max(1),
        );
        let dc = CreateEnhMetaFileW(Some(screen), PCWSTR::null(), Some(&frame), PCWSTR::null());
        ReleaseDC(None, screen);
        if dc.is_invalid() {
            return Err(Error::from_thread());
        }
        let drawn = draw(dc, dest, &info);
        let emf = Emf(CloseEnhMetaFile(dc));
        if emf.0.is_invalid() {
            std::mem::forget(emf);
            return Err(Error::from_thread());
        }
        drawn.map(|()| emf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Graphics::Gdi::{
        EMR_ALPHABLEND, EMR_STRETCHDIBITS, ENHMETAHEADER, GetEnhMetaFileBits, GetEnhMetaFileHeader,
    };

    pub(crate) fn sample() -> Pixels {
        // 4 × 2: an opaque red pixel, a half-transparent one, and clear ones.
        let mut rgba = vec![0u8; 4 * 2 * 4];
        rgba[0..4].copy_from_slice(&[255, 0, 0, 255]);
        rgba[4..8].copy_from_slice(&[0, 0, 255, 128]);
        Pixels {
            width: 4,
            height: 2,
            rgba,
        }
    }

    /// The record types in `emf`, in order.
    fn records(emf: &Emf) -> Vec<u32> {
        let size = unsafe { GetEnhMetaFileBits(emf.0, None) } as usize;
        let mut bits = vec![0u8; size];
        assert_eq!(
            unsafe { GetEnhMetaFileBits(emf.0, Some(&mut bits)) } as usize,
            size
        );
        let mut out = Vec::new();
        let mut at = 0;
        while at + 8 <= bits.len() {
            let kind = u32::from_le_bytes(bits[at..at + 4].try_into().unwrap());
            let len = u32::from_le_bytes(bits[at + 4..at + 8].try_into().unwrap()) as usize;
            out.push(kind);
            if len == 0 {
                break;
            }
            at += len;
        }
        out
    }

    fn frame(emf: &Emf) -> windows::Win32::Foundation::RECTL {
        let mut header = ENHMETAHEADER::default();
        unsafe {
            GetEnhMetaFileHeader(emf.0, size_of::<ENHMETAHEADER>() as u32, Some(&mut header));
        }
        header.rclFrame
    }

    #[test]
    fn the_transparent_picture_is_an_alpha_blended_bitmap_in_an_emf() {
        let emf = alpha_emf(&sample(), [2000, 1000]).unwrap();
        assert!(records(&emf).contains(&(EMR_ALPHABLEND.0)));
        let f = frame(&emf);
        assert_eq!((f.right - f.left, f.bottom - f.top), (2000, 1000));
    }

    #[test]
    fn the_opaque_picture_is_a_plain_bitmap() {
        let emf = opaque_emf(&sample(), [2000, 1000]).unwrap();
        let kinds = records(&emf);
        assert!(kinds.contains(&(EMR_STRETCHDIBITS.0)));
        assert!(!kinds.contains(&(EMR_ALPHABLEND.0)));
        let raw = emf.into_raw();
        unsafe {
            let _ = DeleteEnhMetaFile(Some(raw));
        }
    }

    #[test]
    fn the_dib_is_packed_and_opaque() {
        let dib = dib(&sample());
        assert_eq!(dib.len(), 40 + 4 * 2 * 4);
        let width = i32::from_le_bytes(dib[4..8].try_into().unwrap());
        let bits = u16::from_le_bytes(dib[14..16].try_into().unwrap());
        assert_eq!((width, bits), (4, 32));
        assert!(dib[40..].chunks(4).all(|px| px[3] == 255));
        // The top-left (red) pixel is the first of the last row, bottom-up.
        assert_eq!(&dib[40 + 16..40 + 20], &[0, 0, 255, 255]);
    }

    #[test]
    fn an_empty_picture_is_refused() {
        let empty = Pixels {
            width: 0,
            height: 0,
            rgba: Vec::new(),
        };
        assert!(alpha_emf(&empty, [100, 100]).is_err());
    }
}
