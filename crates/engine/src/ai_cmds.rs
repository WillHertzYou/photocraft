//! AI image commands: Generative Fill, and the AI path of Remove Background (`cutout_cmds`).
//!
//! The engine prepares the pixels and applies the result; the model runs elsewhere. The platform
//! injects a blocking [`AiTransport`] (the desktop app talks HTTP to a local AI server such as
//! IOPaint, or to a cloud service), and each command calls it from a background job, so the
//! engine itself does no network I/O and still builds for the web, where the commands are off.

use std::sync::Arc;

use photocraft_codecs::{ChannelLayout, EncodeOptions, Format, Image};
use photocraft_color::{ColorMode, PixelFormat};
use photocraft_doc::{Layer, LayerId, LayerMask};
use photocraft_geom::Rect;
use photocraft_raster::{Surface, from_rgba_into};
use serde_json::{Value, json};

use crate::commands::CommandSpec;
use crate::{EngineError, Result, Session};

/// What a transport is asked to do. Images are PNG bytes in the document's colours (sRGB-ish
/// 8-bit RGB); the result is PNG bytes too.
#[derive(Clone, Debug)]
pub enum AiTask {
    /// Fill the white part of `mask` in `image`. An empty `prompt` asks for the surroundings
    /// to be continued (object removal); otherwise the prompt describes what to put there.
    Inpaint { image: Vec<u8>, mask: Vec<u8>, prompt: String, negative_prompt: String, seed: Option<u64> },
    /// Cut the subject out of `image`: the result's alpha is the subject.
    RemoveBackground { image: Vec<u8> },
}

impl AiTask {
    pub fn label(&self) -> &'static str {
        match self {
            AiTask::Inpaint { .. } => "Generative Fill",
            AiTask::RemoveBackground { .. } => "Remove Background",
        }
    }
}

/// Runs one task to completion on the calling (worker) thread: PNG bytes, or an error the user
/// can act on ("the AI server at … is not running").
pub type AiTransport = Arc<dyn Fn(&AiTask) -> std::result::Result<Vec<u8>, String> + Send + Sync>;

#[derive(Default, Clone)]
pub struct AiService {
    pub transport: Option<AiTransport>,
    /// Where the transport sends tasks, for messages and `ai.status`.
    pub endpoint: String,
}

/// Largest side of the area sent to the model; beyond it the request is refused rather than
/// uploading a huge crop (the local server resizes to the model's own size anyway).
const MAX_SIDE: u32 = 4096;

fn transport(s: &Session) -> std::result::Result<AiTransport, String> {
    s.ai.transport.clone().ok_or_else(|| "AI features need the AI server: none is set up for this build of PhotoCraft".to_string())
}

fn rgb_document(s: &Session) -> std::result::Result<(), String> {
    let d = s.active().ok_or("no document open")?;
    if d.doc.mode != ColorMode::Rgb {
        return Err("AI features work on RGB documents (Image › Mode › RGB Color)".into());
    }
    Ok(())
}

fn can_fill(s: &Session) -> std::result::Result<(), String> {
    transport(s)?;
    rgb_document(s)?;
    let d = s.active().ok_or("no document open")?;
    let sel = d.doc.selection.as_ref().ok_or("make a selection first: Generative Fill fills the selected area")?;
    if sel.content_bounds().intersect(&d.doc.bounds()).is_empty() {
        return Err("the selection is empty".into());
    }
    Ok(())
}

/// The area sent for a fill: the selection's bounds and some surroundings for context (a
/// quarter of the selection's size, at least 32 px), kept on the canvas.
pub fn fill_context(sel: Rect, canvas: Rect) -> Rect {
    let pad = i32::try_from((sel.width().max(sel.height()) / 4).max(32)).unwrap_or(i32::MAX);
    sel.inflate(pad).intersect(&canvas)
}

fn png_rgb8(w: u32, h: u32, rgb: Vec<u8>) -> Result<Vec<u8>> {
    let img = Image::from_u8(w, h, ChannelLayout::Rgb, rgb).map_err(|e| EngineError::Other(format!("could not prepare the image: {e}")))?;
    photocraft_codecs::encode(&img, Format::Png, &EncodeOptions::default()).map_err(|e| EngineError::Other(format!("could not encode the image: {e}")))
}

fn png_gray8(w: u32, h: u32, gray: Vec<u8>) -> Result<Vec<u8>> {
    let img = Image::from_u8(w, h, ChannelLayout::Gray, gray).map_err(|e| EngineError::Other(format!("could not prepare the mask: {e}")))?;
    photocraft_codecs::encode(&img, Format::Png, &EncodeOptions::default()).map_err(|e| EngineError::Other(format!("could not encode the mask: {e}")))
}

fn to_u8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// The AI's PNG as straight RGBA floats, checked to be `w`×`h`.
fn decode_result(bytes: &[u8], w: u32, h: u32, what: &str) -> Result<Vec<[f32; 4]>> {
    let img = photocraft_codecs::decode(bytes).map_err(|e| EngineError::Other(format!("{what}: the AI server sent an image PhotoCraft can't read ({e})")))?;
    if img.width() != w || img.height() != h {
        return Err(EngineError::Other(format!("{what}: the AI server sent a {}×{} image for a {w}×{h} area", img.width(), img.height())));
    }
    Ok(img.to_rgba_f32().chunks_exact(4).map(|c| [c[0], c[1], c[2], c[3]]).collect())
}

fn ai_error(what: &str, endpoint: &str, e: String) -> EngineError {
    let at = if endpoint.is_empty() { String::new() } else { format!(" ({endpoint})") };
    EngineError::Other(format!("{what} failed: {e}{at}"))
}

/// The visible image over `rect`, as 8-bit RGB with transparency flattened onto white.
fn composite_rgb8(doc: &photocraft_doc::Document, rect: Rect) -> Vec<u8> {
    let buf = photocraft_compose::render(doc, rect);
    let mut out = Vec::with_capacity(buf.px.len() * 3);
    for p in &buf.px {
        let a = p[3].clamp(0.0, 1.0);
        for c in &p[..3] {
            out.push(to_u8(c * a + (1.0 - a)));
        }
    }
    out
}

fn generative_fill(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "edit.generativeFill";
    can_fill(s).map_err(EngineError::Other)?;
    let ai = transport(s).map_err(EngineError::Other)?;
    let endpoint = s.ai.endpoint.clone();
    let prompt = p.get("prompt").and_then(Value::as_str).unwrap_or("").trim().to_string();
    if prompt.chars().count() > 1000 {
        return Err(EngineError::Other(format!("{CMD}: the prompt is longer than 1000 characters")));
    }
    let negative_prompt = p.get("negativePrompt").and_then(Value::as_str).unwrap_or("").trim().to_string();
    let seed = p.get("seed").and_then(Value::as_u64);
    let st = s.active().ok_or(EngineError::NoDocument)?;
    let doc = st.doc.clone();
    let anchor = st.active_layer;
    let canvas = doc.bounds();
    let sel = doc.selection.clone().ok_or_else(|| EngineError::Other("make a selection first".into()))?;
    let rect = fill_context(sel.content_bounds().intersect(&canvas), canvas);
    if rect.is_empty() {
        return Err(EngineError::Other("the selection is empty".into()));
    }
    if rect.width() > MAX_SIDE || rect.height() > MAX_SIDE {
        return Err(EngineError::Other(format!(
            "Generative Fill works on areas up to {MAX_SIDE} px across; this one is {}×{} px. Select a smaller area.",
            rect.width(),
            rect.height()
        )));
    }
    let fmt = doc.pixel_format();
    let name = if prompt.is_empty() { "Generative Fill".to_string() } else { prompt.chars().take(60).collect() };
    crate::jobs::run(
        s,
        "Generative Fill",
        true,
        move |ctx| {
            ctx.progress(0.0, "Preparing the area");
            let (w, h) = (rect.width(), rect.height());
            let image = png_rgb8(w, h, composite_rgb8(&doc, rect))?;
            let cancel = || ctx.cancelled();
            let cover = crate::fill_cmds::read_window_mask(&sel, rect, &cancel).ok_or(EngineError::Cancelled)?;
            let mask = png_gray8(w, h, cover.iter().map(|c| if *c > 0.0 { 255 } else { 0 }).collect())?;
            ctx.check()?;
            ctx.progress(0.1, "Generating (this can take a minute)");
            let task = AiTask::Inpaint { image, mask, prompt, negative_prompt, seed };
            let bytes = ai(&task).map_err(|e| ai_error("Generative Fill", &endpoint, e))?;
            ctx.check()?;
            ctx.progress(0.9, "Adding the result");
            let px = decode_result(&bytes, w, h, "Generative Fill")?;
            Ok((px, cover))
        },
        move |s, (px, cover)| {
            let target = PixelFormat::new(fmt.mode, fmt.sample, true);
            let n = target.channels();
            let mut data = vec![0.0f32; px.len() * n];
            let mut enc = [0.0f32; 8];
            for (i, rgba) in px.iter().enumerate() {
                from_rgba_into(&target, [rgba[0], rgba[1], rgba[2], 1.0], &mut enc);
                data[i * n..(i + 1) * n].copy_from_slice(&enc[..n]);
            }
            // Photoshop's generative layer: the generated pixels over the whole context area,
            // shown through a mask of the selection.
            let mut mask_surface = Surface::with_default(PixelFormat::GRAY8, &[0.0]);
            mask_surface.write_region(rect, &cover);
            mask_surface.prune();
            let layer = s.edit("Generative Fill", move |doc, active| {
                let mut l = Layer::raster(name, target);
                let surf = crate::pixels_mut(&mut l)?;
                surf.write_region(rect, &data);
                surf.prune();
                l.mask = Some(LayerMask { surface: mask_surface, ..LayerMask::reveal_all() });
                let id = doc.insert_above(anchor.or(*active), l);
                *active = Some(id);
                Ok(id)
            })?;
            Ok(json!({"layer": layer.0, "area": [rect.x0, rect.y0, rect.width(), rect.height()]}))
        },
    )
}

/// The subject of `area` as a layer-mask surface, found by the AI model: Remove Background's AI
/// path (`cutout_cmds`). Reads the layer's own pixels, or with `sample_all` the visible image.
pub fn subject_mask(
    ai: &AiTransport,
    endpoint: &str,
    doc: &photocraft_doc::Document,
    id: LayerId,
    area: Rect,
    sample_all: bool,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> std::result::Result<Surface, String> {
    let area = area.intersect(&doc.bounds());
    if area.is_empty() {
        return Err("the layer is empty".into());
    }
    if area.width() > 2 * MAX_SIDE || area.height() > 2 * MAX_SIDE {
        return Err(format!("the AI cut-out works on areas up to {} px across", 2 * MAX_SIDE));
    }
    let (w, h) = (area.width(), area.height());
    let rgb = if sample_all {
        composite_rgb8(doc, area)
    } else {
        let surf = doc.layer(id).and_then(Layer::surface).ok_or("the layer has no pixels")?;
        let fmt = surf.format();
        let n = fmt.channels().max(1);
        let px = crate::fill_cmds::read_window(surf, area, cancelled).ok_or("cancelled")?;
        let mut rgb = Vec::with_capacity(px.len() / n * 3);
        for c in px.chunks_exact(n) {
            let [r, g, b, a] = photocraft_raster::to_rgba(&fmt, c);
            let a = a.clamp(0.0, 1.0);
            // Transparent pixels as white, so the model sees what Photoshop's would.
            for v in [r, g, b] {
                rgb.push(to_u8(v * a + (1.0 - a)));
            }
        }
        rgb
    };
    let image = png_rgb8(w, h, rgb).map_err(|e| e.to_string())?;
    let bytes = ai(&AiTask::RemoveBackground { image }).map_err(|e| ai_error("Remove Background", endpoint, e).to_string())?;
    let out = decode_result(&bytes, w, h, "Remove Background").map_err(|e| e.to_string())?;
    let alpha: Vec<f32> = out.iter().map(|p| p[3]).collect();
    if !alpha.iter().any(|a| *a > 0.5) {
        return Err("no subject found".into());
    }
    let mut mask = Surface::with_default(PixelFormat::GRAY8, &[0.0]);
    mask.write_region(area, &alpha);
    mask.prune();
    Ok(mask)
}

fn status(s: &mut Session, _: &Value) -> Result<Value> {
    Ok(json!({"available": s.ai.transport.is_some(), "endpoint": s.ai.endpoint}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "edit.generativeFill",
            label: "Generative Fill…",
            menu: &["Edit"],
            shortcut: None,
            // An empty prompt fills from the surroundings (object removal). `seed` (u64) repeats a
            // result; without it every fill differs, as in Photoshop.
            params: r##"{"prompt":text,"negativePrompt":text,"seed":json}"##,
            enabled: can_fill,
            run: generative_fill,
            journal: true,
        },
        CommandSpec {
            id: "ai.status",
            label: "AI Status",
            menu: &[],
            shortcut: None,
            params: "{}",
            enabled: crate::commands::always,
            run: status,
            journal: false,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use photocraft_geom::Size;

    fn session_with_selection(transport: Option<AiTransport>) -> Session {
        let mut s = Session::new();
        let mut doc = photocraft_doc::Document::new("t", Size::new(200, 120), ColorMode::Rgb, photocraft_color::SampleType::U8);
        let mut bg = Layer::raster("Background", doc.pixel_format());
        if let Some(surf) = bg.surface_mut() {
            let n = surf.format().channels();
            surf.write_region(Rect::new(0, 0, 200, 120), &vec![1.0f32; 200 * 120 * n]);
        }
        doc.layers.push(bg);
        let mut sel = Surface::with_default(PixelFormat::GRAY8, &[0.0]);
        sel.write_region(Rect::new(50, 40, 90, 70), &vec![1.0f32; 40 * 30]);
        doc.selection = Some(sel);
        s.add_document(doc, None);
        if let Some(st) = s.active_mut() {
            st.active_layer = st.doc.layers.first().map(|l| l.id);
        }
        s.ai = AiService { transport, endpoint: "test".into() };
        s
    }

    /// A stand-in model: paints the whole area red (inpaint) or keeps the left half (cut-out).
    fn fake() -> AiTransport {
        Arc::new(|t: &AiTask| {
            let src = match t {
                AiTask::Inpaint { image, .. } | AiTask::RemoveBackground { image } => photocraft_codecs::decode(image).map_err(|e| e.to_string())?,
            };
            let (w, h) = (src.width(), src.height());
            let mut out = Vec::with_capacity((w * h * 4) as usize);
            for y in 0..h {
                for x in 0..w {
                    let _ = y;
                    match t {
                        AiTask::Inpaint { .. } => out.extend_from_slice(&[255, 0, 0, 255]),
                        AiTask::RemoveBackground { .. } => out.extend_from_slice(&[0, 0, 0, if x < w / 2 { 255 } else { 0 }]),
                    }
                }
            }
            let img = Image::from_u8(w, h, ChannelLayout::Rgba, out).map_err(|e| e.to_string())?;
            photocraft_codecs::encode(&img, Format::Png, &EncodeOptions::default()).map_err(|e| e.to_string())
        })
    }

    #[test]
    fn context_pads_the_selection_and_stays_on_the_canvas() {
        let canvas = Rect::new(0, 0, 200, 120);
        assert_eq!(fill_context(Rect::new(50, 40, 90, 70), canvas), Rect::new(18, 8, 122, 102));
        assert_eq!(fill_context(Rect::new(0, 0, 10, 10), canvas), Rect::new(0, 0, 42, 42));
    }

    #[test]
    fn generative_fill_is_off_without_a_transport() {
        let s = session_with_selection(None);
        assert!(!s.is_enabled("edit.generativeFill"));
    }

    #[test]
    fn generative_fill_adds_a_masked_layer() {
        let mut s = session_with_selection(Some(fake()));
        assert!(s.is_enabled("edit.generativeFill"));
        let r = s.execute("edit.generativeFill", json!({"prompt": "a red box"})).unwrap();
        let d = s.active().unwrap();
        assert_eq!(d.doc.layers.len(), 2, "{r}");
        let l = d.doc.layer(LayerId(r["layer"].as_u64().unwrap())).unwrap();
        assert_eq!(l.name, "a red box");
        let mask = &l.mask.as_ref().unwrap().surface;
        assert_eq!(mask.content_bounds(), Rect::new(50, 40, 90, 70), "the mask is the selection");
        let px = l.surface().unwrap();
        assert_eq!(px.content_bounds(), Rect::new(18, 8, 122, 102), "pixels cover the context area");
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(s.active().unwrap().doc.layers.len(), 1, "one undo step");
    }

    #[test]
    fn a_failing_server_is_an_error_not_a_panic() {
        let fail: AiTransport = Arc::new(|_| Err("the AI server at http://127.0.0.1:8080 is not running".into()));
        let mut s = session_with_selection(Some(fail));
        let e = s.execute("edit.generativeFill", json!({})).unwrap_err().to_string();
        assert!(e.contains("not running"), "{e}");
        assert_eq!(s.active().unwrap().doc.layers.len(), 1);
    }

    #[test]
    fn a_wrong_size_result_is_refused() {
        let tiny: AiTransport = Arc::new(|_| {
            let img = Image::from_u8(2, 2, ChannelLayout::Rgb, vec![0; 12]).map_err(|e| e.to_string())?;
            photocraft_codecs::encode(&img, Format::Png, &EncodeOptions::default()).map_err(|e| e.to_string())
        });
        let mut s = session_with_selection(Some(tiny));
        let e = s.execute("edit.generativeFill", json!({})).unwrap_err().to_string();
        assert!(e.contains("2×2"), "{e}");
    }

    #[test]
    fn remove_background_uses_the_model_when_there_is_one() {
        let mut s = session_with_selection(Some(fake()));
        let r = s.execute("layer.removeBackground", json!({})).unwrap();
        assert_eq!(r["method"], "ai", "{r}");
        let d = s.active().unwrap();
        let l = d.doc.layers.first().unwrap();
        let mask = &l.mask.as_ref().unwrap().surface;
        assert_eq!(mask.content_bounds(), Rect::new(0, 0, 100, 120), "left half kept");
    }

    #[test]
    fn remove_background_falls_back_when_the_server_is_off() {
        let fail: AiTransport = Arc::new(|_| Err("the AI server at http://127.0.0.1:8080 is not running".into()));
        let mut s = session_with_selection(Some(fail));
        // A flat grey layer has no subject for the classical method either: its error, with the
        // AI's reason, is what the user sees; either way nothing panics.
        match s.execute("layer.removeBackground", json!({})) {
            Ok(r) => assert_eq!(r["method"], "classic", "{r}"),
            Err(e) => assert!(e.to_string().contains("subject"), "{e}"),
        }
    }
}
