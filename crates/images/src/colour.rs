//! Display colour of placed rasters. Pixels with an embedded ICC profile are converted to sRGB;
//! CMYK pixels go through their own profile, else the working CMYK space. Every raster decode
//! path (JPEG, PNG, TIFF, WebP, PSD) converts here, once, when the image is decoded.
//!
//! [`Profile`], [`Transform`], [`TransformOptions`], [`Intent`] and [`Builtin`] follow the names of
//! the shared craft-cms crate, so moving to it changes this module only. Underneath they are thin
//! wrappers over `moxcms`.
//!
//! Profiles are untrusted input: one that is too large, can't be parsed, doesn't match the
//! pixels or yields no transform is ignored with a log line, and the pixels are shown as before
//! (RGB and grey as sRGB, CMYK through the working space).

use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::{Arc, Mutex, PoisonError};

use designcraft_color::{Color, cms::Cms};
use image::RgbaImage;
use moxcms::{ColorProfile, DataColorSpace, Layout, RenderingIntent, Transform8BitExecutor};

pub use designcraft_color::cms::Intent;

use crate::CmykRaster;

/// Embedded profiles larger than this are ignored.
pub const MAX_ICC_BYTES: usize = 16 << 20;

/// Pixels per transform call.
const CHUNK: usize = 4096;

/// Transforms kept by [`Transform::cached`].
const CACHED_TRANSFORMS: usize = 16;

/// Why a profile or transform can't be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileError(String);

impl std::fmt::Display for ProfileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ProfileError {}

/// Profiles built into the code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Builtin {
    Srgb,
    AdobeRgb,
    DisplayP3,
    ProPhotoRgb,
}

/// The colour space a profile's device values are in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Space {
    Gray,
    Rgb,
    Cmyk,
    Other,
}

impl Space {
    fn channels(self) -> usize {
        match self {
            Space::Gray => 1,
            Space::Rgb => 3,
            Space::Cmyk | Space::Other => 4,
        }
    }
    /// moxcms reads four-channel CMYK through its RGBA layout.
    fn layout(self) -> Layout {
        match self {
            Space::Gray => Layout::Gray,
            Space::Rgb => Layout::Rgb,
            Space::Cmyk | Space::Other => Layout::Rgba,
        }
    }
}

/// An ICC profile.
#[derive(Clone, Debug)]
pub struct Profile {
    inner: ColorProfile,
    builtin: Option<Builtin>,
    /// Identifies the profile for [`Transform::cached`]; `None` when it can't be encoded.
    key: Option<u64>,
}

fn hash(bytes: &[u8]) -> u64 {
    let mut h = DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

impl Profile {
    /// Parse ICC bytes (at most [`MAX_ICC_BYTES`]).
    pub fn parse(bytes: &[u8]) -> Result<Self, ProfileError> {
        if bytes.len() > MAX_ICC_BYTES {
            return Err(ProfileError(format!("ICC profile of {} bytes is too large", bytes.len())));
        }
        let inner = ColorProfile::new_from_slice(bytes).map_err(|e| ProfileError(format!("ICC profile can't be read ({e:?})")))?;
        Ok(Self { inner, builtin: None, key: Some(hash(bytes)) })
    }

    pub fn builtin(b: Builtin) -> Self {
        let inner = match b {
            Builtin::Srgb => ColorProfile::new_srgb(),
            Builtin::AdobeRgb => ColorProfile::new_adobe_rgb(),
            Builtin::DisplayP3 => ColorProfile::new_display_p3(),
            Builtin::ProPhotoRgb => ColorProfile::new_pro_photo_rgb(),
        };
        let mut h = DefaultHasher::new();
        b.hash(&mut h);
        Self { inner, builtin: Some(b), key: Some(h.finish()) }
    }

    /// A profile the colour crate already parsed or built.
    fn from_moxcms(inner: ColorProfile) -> Self {
        let key = inner.encode().ok().map(|b| hash(&b));
        Self { inner, builtin: None, key }
    }

    pub fn space(&self) -> Space {
        match self.inner.color_space {
            DataColorSpace::Gray => Space::Gray,
            DataColorSpace::Rgb => Space::Rgb,
            DataColorSpace::Cmyk => Space::Cmyk,
            _ => Space::Other,
        }
    }
}

/// How a [`Transform`] maps colours.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct TransformOptions {
    pub intent: Intent,
    /// Scale the source's black to the destination's (relative colorimetric only).
    pub black_point_compensation: bool,
}

/// A colour transform between two profiles, for interleaved 8-bit pixels.
pub struct Transform {
    exec: Arc<Transform8BitExecutor>,
    src_channels: usize,
    dst_channels: usize,
    /// Black point compensation, applied to the sRGB output.
    bpc: Option<Box<[u8; 256]>>,
}

fn rendering_intent(i: Intent) -> RenderingIntent {
    match i {
        Intent::Perceptual => RenderingIntent::Perceptual,
        Intent::RelativeColorimetric => RenderingIntent::RelativeColorimetric,
        Intent::Saturation => RenderingIntent::Saturation,
        Intent::AbsoluteColorimetric => RenderingIntent::AbsoluteColorimetric,
    }
}

fn srgb_to_linear(v: f32) -> f32 {
    if v <= 0.040_45 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

fn linear_to_srgb(v: f32) -> f32 {
    if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
}

/// Black point compensation into sRGB as a table on the 8-bit output: linear light is rescaled
/// so the source's black (luminance `y_black`) lands on sRGB black and white stays white. sRGB
/// linear RGB is a linear map of XYZ, so this is the usual XYZ scaling for a neutral black.
fn bpc_table(src: &Profile, intent: RenderingIntent) -> Option<Box<[u8; 256]>> {
    let candidates: &[f32] = match src.space() {
        // Solid black ink, all inks, and a typical rich black: the darkest is the black point.
        Space::Cmyk => &[0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0, 1.0, 0.75, 0.68, 0.67, 0.9],
        Space::Rgb => &[0.0; 3],
        Space::Gray => &[0.0],
        Space::Other => return None,
    };
    let srgb = ColorProfile::new_srgb();
    let opts = moxcms::TransformOptions { rendering_intent: intent, ..Default::default() };
    let x = src.inner.create_transform_f32(src.space().layout(), &srgb, Layout::Rgb, opts).ok()?;
    let mut out = vec![0f32; candidates.len() / src.space().channels() * 3];
    x.transform(candidates, &mut out).ok()?;
    let y_black = out
        .as_chunks::<3>()
        .0
        .iter()
        .map(|c| {
            let [r, g, b] = c.map(|v| srgb_to_linear(v.clamp(0.0, 1.0)));
            0.2126 * r + 0.7152 * g + 0.0722 * b
        })
        .fold(f32::INFINITY, f32::min);
    if !(1e-4..0.5).contains(&y_black) {
        return None;
    }
    let mut t = Box::new([0u8; 256]);
    for (i, v) in t.iter_mut().enumerate() {
        let lin = (srgb_to_linear(i as f32 / 255.0) - y_black) / (1.0 - y_black);
        *v = (linear_to_srgb(lin.clamp(0.0, 1.0)) * 255.0).round() as u8;
    }
    Some(t)
}

impl Transform {
    pub fn new(src: &Profile, dst: &Profile, intent: Intent) -> Result<Self, ProfileError> {
        Self::with_options(src, dst, TransformOptions { intent, black_point_compensation: false })
    }

    pub fn with_options(src: &Profile, dst: &Profile, options: TransformOptions) -> Result<Self, ProfileError> {
        let intent = rendering_intent(options.intent);
        let opts = moxcms::TransformOptions { rendering_intent: intent, ..Default::default() };
        let exec = src
            .inner
            .create_transform_8bit(src.space().layout(), &dst.inner, dst.space().layout(), opts)
            .map_err(|e| ProfileError(format!("no transform between the profiles ({e:?})")))?;
        // Black point compensation is implemented for an sRGB destination (the display).
        let bpc = (options.black_point_compensation && options.intent == Intent::RelativeColorimetric && dst.builtin == Some(Builtin::Srgb))
            .then(|| bpc_table(src, intent))
            .flatten();
        Ok(Self { exec, src_channels: src.space().channels(), dst_channels: dst.space().channels(), bpc })
    }

    /// [`Transform::with_options`], reusing a transform built earlier for the same profiles and
    /// options (CMYK transforms build a large table).
    pub fn cached(src: &Profile, dst: &Profile, options: TransformOptions) -> Result<Arc<Self>, ProfileError> {
        static CACHE: Mutex<Vec<((u64, u64, TransformOptions), Arc<Transform>)>> = Mutex::new(Vec::new());
        let key = src.key.zip(dst.key).map(|(s, d)| (s, d, options));
        if let Some(key) = key
            && let Some((_, t)) = CACHE.lock().unwrap_or_else(PoisonError::into_inner).iter().find(|(k, _)| *k == key)
        {
            return Ok(t.clone());
        }
        let t = Arc::new(Self::with_options(src, dst, options)?);
        if let Some(key) = key {
            let mut c = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
            if c.len() >= CACHED_TRANSFORMS {
                c.remove(0);
            }
            c.push((key, t.clone()));
        }
        Ok(t)
    }

    /// Convert interleaved pixels: `src` in the source's channels, `dst` the same number of
    /// pixels in the destination's.
    pub fn convert_u8(&self, src: &[u8], dst: &mut [u8]) -> Result<(), ProfileError> {
        let n = src.len() / self.src_channels;
        if !src.len().is_multiple_of(self.src_channels) || Some(dst.len()) != n.checked_mul(self.dst_channels) {
            return Err(ProfileError("pixel buffers don't match the transform".into()));
        }
        self.exec.transform(src, dst).map_err(|e| ProfileError(format!("{e:?}")))?;
        if let Some(t) = &self.bpc {
            for v in dst.iter_mut() {
                *v = t[usize::from(*v)];
            }
        }
        Ok(())
    }
}

/// Decoded 8-bit pixels and what their values mean.
pub enum Pixels {
    /// RGB with straight alpha; converted in place.
    Rgb(RgbaImage),
    /// Grey, each pixel `[g, g, g, a]`; converted in place.
    Gray(RgbaImage),
    /// Ink values (0 = no ink).
    Cmyk(CmykRaster),
}

/// How images are shown: the CMYK profile for CMYK pixels without a usable profile of their own
/// (`None`: the profile-free formula, for Device CMYK), and the transform options.
#[derive(Clone, Debug)]
pub struct DisplayColour {
    working_cmyk: Option<Profile>,
    options: TransformOptions,
}

impl DisplayColour {
    pub fn new(working_cmyk: Option<Profile>, options: TransformOptions) -> Self {
        Self { working_cmyk, options }
    }

    /// The active colour settings (Edit → Color Settings): working CMYK space, intent and black
    /// point compensation.
    pub fn active() -> Self {
        Self::of(&designcraft_color::cms::active())
    }

    pub fn of(cms: &Cms) -> Self {
        let s = cms.settings();
        Self::new(cms.cmyk_icc_profile().map(Profile::from_moxcms), TransformOptions { intent: s.intent, black_point_compensation: s.bpc })
    }
}

/// Straight RGBA8 in display sRGB. `None` only when a CMYK raster's buffers don't match its size
/// or the output can't be allocated.
pub fn to_display_rgba(px: Pixels, embedded_icc: Option<&[u8]>, display: &DisplayColour) -> Option<RgbaImage> {
    let own = |space| embedded_icc.and_then(|icc| to_srgb(&parse(icc, space)?, display.options));
    match px {
        Pixels::Rgb(mut img) => {
            if let Some(x) = own(Space::Rgb)
                && !keeps_rgb(&x)
            {
                in_place(&mut img, &x, 3);
            }
            Some(img)
        }
        Pixels::Gray(mut img) => {
            if let Some(x) = own(Space::Gray)
                && !keeps_gray(&x)
            {
                in_place(&mut img, &x, 1);
            }
            Some(img)
        }
        Pixels::Cmyk(raster) => {
            let xf = own(Space::Cmyk).or_else(|| to_srgb(display.working_cmyk.as_ref()?, display.options));
            cmyk(&raster, xf.as_deref())
        }
    }
}

fn parse(icc: &[u8], pixels: Space) -> Option<Profile> {
    match Profile::parse(icc) {
        Ok(p) if p.space() == pixels => Some(p),
        Ok(p) => {
            log::warn!("embedded ICC profile is for {:?}, the pixels are {pixels:?}: ignored", p.space());
            None
        }
        Err(e) => {
            log::warn!("embedded {e}: ignored");
            None
        }
    }
}

fn to_srgb(src: &Profile, options: TransformOptions) -> Option<Arc<Transform>> {
    match Transform::cached(src, &Profile::builtin(Builtin::Srgb), options) {
        Ok(x) => Some(x),
        Err(e) => {
            log::warn!("image ICC profile to sRGB: {e}; profile ignored");
            None
        }
    }
}

/// Whether an RGB transform leaves every value within 1 of where it was (an sRGB profile): the
/// pixels are then kept exactly as decoded.
fn keeps_rgb(x: &Transform) -> bool {
    const V: [u8; 9] = [0, 32, 64, 96, 128, 160, 192, 224, 255];
    let src: Vec<u8> = V.iter().flat_map(|&r| V.iter().flat_map(move |&g| V.iter().flat_map(move |&b| [r, g, b]))).collect();
    let mut dst = vec![0u8; src.len()];
    x.convert_u8(&src, &mut dst).is_ok() && src.iter().zip(&dst).all(|(a, b)| a.abs_diff(*b) <= 1)
}

fn keeps_gray(x: &Transform) -> bool {
    let src: Vec<u8> = (0..=255).collect();
    let mut dst = vec![0u8; src.len() * 3];
    x.convert_u8(&src, &mut dst).is_ok() && src.iter().zip(dst.as_chunks::<3>().0).all(|(g, rgb)| rgb.iter().all(|v| v.abs_diff(*g) <= 1))
}

/// Convert RGBA pixels whose first `channels` (3: RGB, 1: grey) hold the source values.
fn in_place(img: &mut RgbaImage, x: &Transform, channels: usize) {
    let mut src = Vec::with_capacity(CHUNK * channels);
    let mut dst = vec![0u8; CHUNK * 3];
    for chunk in img.chunks_mut(CHUNK * 4) {
        let px = chunk.as_chunks_mut::<4>().0;
        src.clear();
        src.extend(px.iter().flat_map(|p| p.iter().take(channels)));
        let Some(out) = dst.get_mut(..px.len() * 3) else { return };
        if x.convert_u8(&src, out).is_err() {
            return;
        }
        for (p, rgb) in px.iter_mut().zip(out.as_chunks::<3>().0) {
            p[..3].copy_from_slice(rgb);
        }
    }
}

/// CMYK inks → RGBA through `x`, or the profile-free formula without one.
fn cmyk(r: &CmykRaster, x: Option<&Transform>) -> Option<RgbaImage> {
    let pixels = usize::try_from(r.width).ok()?.checked_mul(usize::try_from(r.height).ok()?)?;
    let len = pixels.checked_mul(4)?;
    if r.cmyk.len() != len || r.alpha.as_ref().is_some_and(|a| a.len() != pixels) {
        return None;
    }
    let mut rgba = Vec::new();
    rgba.try_reserve_exact(len).ok()?;
    rgba.resize(len, 255);

    let mut converted = false;
    if let Some(x) = x {
        converted = true;
        let mut dst = vec![0u8; CHUNK * 3];
        for (src, out) in r.cmyk.chunks(CHUNK * 4).zip(rgba.chunks_mut(CHUNK * 4)) {
            let rgb = dst.get_mut(..src.len() / 4 * 3)?;
            if let Err(e) = x.convert_u8(src, rgb) {
                log::warn!("CMYK image conversion failed ({e}): shown without a profile");
                converted = false;
                break;
            }
            for (p, c) in out.as_chunks_mut::<4>().0.iter_mut().zip(rgb.as_chunks::<3>().0) {
                p[..3].copy_from_slice(c);
            }
        }
    }
    if !converted {
        let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        let ink = |v: u8| f32::from(v) / 255.0;
        for (p, &[c, m, y, k]) in rgba.as_chunks_mut::<4>().0.iter_mut().zip(r.cmyk.as_chunks::<4>().0) {
            let [rr, g, b] = Color::cmyk(ink(c), ink(m), ink(y), ink(k)).to_rgb_uncalibrated();
            p[..3].copy_from_slice(&[q(rr), q(g), q(b)]);
        }
    }
    if let Some(alpha) = &r.alpha {
        for (p, &a) in rgba.as_chunks_mut::<4>().0.iter_mut().zip(alpha) {
            p[3] = a;
        }
    }
    RgbaImage::from_raw(r.width, r.height, rgba)
}

#[cfg(test)]
mod tests;
