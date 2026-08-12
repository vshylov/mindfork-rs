//! Runtime — reading/writing the system clipboard (the arboard slot). Part of the [`super`] module, split out of the
//! runtime.rs monolith (see docs/history/refactoring-god-objects.md, stage 7).

/// Reads the system clipboard's text (lazily creating the client). `None` if the clipboard
/// is unavailable/empty/non-text. Used only on Windows to restore
/// emoji in a paste (see [`reconcile_paste`]).
#[cfg(windows)]
pub(super) fn read_clipboard_text(slot: &mut Option<arboard::Clipboard>) -> Option<String> {
    if slot.is_none() {
        *slot = arboard::Clipboard::new().ok();
    }
    slot.as_mut().and_then(|c| c.get_text().ok())
}

/// Reads any text on the clipboard, on **every** platform — the fallback half of the
/// `Ctrl+V` handler: with no image on the clipboard the key must still do what the help
/// overlay has always promised it does.
///
/// Separate from [`read_clipboard_text`], which is Windows-only and exists for a
/// different job (reconstructing a paste's lost supplementary-plane characters).
pub(super) fn clipboard_text(slot: &mut Option<arboard::Clipboard>) -> Option<String> {
    if slot.is_none() {
        *slot = arboard::Clipboard::new().ok();
    }
    slot.as_mut()
        .and_then(|c| c.get_text().ok())
        .filter(|t| !t.is_empty())
}

/// Reads an image off the system clipboard as `(width, height, RGBA8 row-major)`.
///
/// `None` covers every "not an image" case alike — an unavailable clipboard, text on it,
/// nothing on it — because the caller's next step is the same in all of them and arboard
/// does not distinguish them usefully either.
///
/// The pixels come back raw: arboard normalizes whatever the platform stores (`CF_DIB` on
/// Windows, `image/png` on X11, an `NSImage` on macOS) into RGBA, so there is no container
/// format to sniff and no decoder to run here — the encode happens in
/// [`crate::features::image_prepare::prepare_rgba`].
pub(super) fn clipboard_image(
    slot: &mut Option<arboard::Clipboard>,
) -> Option<(u32, u32, Vec<u8>)> {
    if slot.is_none() {
        *slot = arboard::Clipboard::new().ok();
    }
    let data = slot.as_mut()?.get_image().ok()?;
    let (width, height) = (
        u32::try_from(data.width).ok()?,
        u32::try_from(data.height).ok()?,
    );
    Some((width, height, data.bytes.into_owned()))
}

/// Writes text into the system clipboard, creating the client lazily and reusing
/// it. Returns error text (instead of panicking) if the clipboard is unavailable — on
/// headless Linux with no X11/Wayland, the `arboard` constructor can fail.
pub(super) fn write_clipboard(
    slot: &mut Option<arboard::Clipboard>,
    text: &str,
) -> Result<(), String> {
    if slot.is_none() {
        *slot = Some(arboard::Clipboard::new().map_err(|e| e.to_string())?);
    }
    // `unwrap` is safe: we just guaranteed `Some`.
    slot.as_mut()
        .unwrap()
        .set_text(text.to_string())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real round trip through the **operating system's** clipboard: put an image on
    /// it, read it back, and encode it the way a paste would.
    ///
    /// `#[ignore]` because it touches a shared global resource — it would clobber whatever
    /// the developer had copied, and on a headless CI box there is no clipboard at all.
    /// It exists because everything else about this path is a unit test against pixels we
    /// made up: only a real clipboard can show that arboard's `image-data` feature is
    /// actually wired on this platform, that the bytes really come back RGBA in the size
    /// reported, and that the round trip survives the platform's own format conversion
    /// (`CF_DIB` on Windows, `image/png` on X11).
    ///
    /// Run: `cargo test clipboard_image_round_trip -- --ignored --nocapture`.
    #[test]
    #[ignore = "uses the real system clipboard (clobbers what the user copied)"]
    fn clipboard_image_round_trip() {
        let (w, h) = (64u32, 32u32);
        // Distinct per-pixel values, so a stride or channel-order mistake shows up as a
        // mismatch rather than as a plausible-looking picture.
        let source: Vec<u8> = (0..w as usize * h as usize)
            .flat_map(|i| [(i % 251) as u8, (i % 253) as u8, (i % 257 % 256) as u8, 255])
            .collect();

        let mut slot: Option<arboard::Clipboard> = None;
        if slot.is_none() {
            match arboard::Clipboard::new() {
                Ok(c) => slot = Some(c),
                Err(e) => {
                    eprintln!("skip: no system clipboard here ({e})");
                    return;
                }
            }
        }
        slot.as_mut()
            .unwrap()
            .set_image(arboard::ImageData {
                width: w as usize,
                height: h as usize,
                bytes: source.clone().into(),
            })
            .expect("putting an image on the clipboard");

        let (rw, rh, rgba) = clipboard_image(&mut slot).expect("an image back off the clipboard");
        assert_eq!((rw, rh), (w, h), "the size must survive the round trip");
        assert_eq!(rgba.len(), source.len(), "RGBA8, four bytes per pixel");
        // Not asserting byte equality: a platform may composite onto an opaque background
        // or reorder channels internally. What must hold is that the pixels are *ours* —
        // an all-black or all-white buffer would mean the image never made it.
        let distinct = rgba
            .chunks(4)
            .map(|p| (p[0], p[1], p[2]))
            .collect::<std::collections::HashSet<_>>();
        assert!(
            distinct.len() > 100,
            "the round trip flattened the image into {} distinct colours",
            distinct.len()
        );

        // And the encode a paste would do accepts what came back.
        let prepared =
            crate::features::image_prepare::prepare_rgba(rw, rh, &rgba, 1568).expect("encoding");
        assert_eq!(prepared.mime, "image/png");
        assert_eq!((prepared.width, prepared.height), (w, h));
        eprintln!(
            "clipboard round trip: {w}x{h} -> {} bytes png",
            prepared.bytes.len()
        );
    }
}
