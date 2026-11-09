//! The paste against the **operating system's** clipboard (spec §9.10): round trips
//! that only a real clipboard can show. `#[ignore]`d, since each clobbers what the
//! developer copied and a headless box has no clipboard at all — and kept under a
//! `tests/` directory, where coverage does not count lines CI never runs.
//!
//! Run: `cargo test --bin mindfork clipboard_ -- --ignored --nocapture --test-threads=1`.

use super::*;

/// A real round trip through the **operating system's** clipboard: put a file list on
/// it — what "Copy" on a file in a file manager does — and read it back the way a
/// paste does, then split it the way a paste would.
///
/// `#[ignore]` for the reason [`clipboard_image_round_trip`] is: it clobbers what the
/// developer copied, and a headless box has no clipboard. Measured on Windows before
/// this existed: the file list came back while `get_image` failed on it, so a copied
/// picture file was refused as "no image on the clipboard"
/// (docs/research/pasted-image-names.md §3.1).
///
/// Run: `cargo test clipboard_file_list_round_trip -- --ignored --nocapture`.
#[test]
#[ignore = "uses the real system clipboard (clobbers what the user copied)"]
fn clipboard_file_list_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let picture = dir.path().join("copied-picture.png");
    let letter = dir.path().join("letter.txt");
    image::RgbImage::from_pixel(8, 8, image::Rgb([200, 30, 30]))
        .save(&picture)
        .unwrap();
    std::fs::write(&letter, "not a picture").unwrap();

    let mut slot: Option<arboard::Clipboard> = match arboard::Clipboard::new() {
        Ok(c) => Some(c),
        Err(e) => {
            eprintln!("skip: no system clipboard here ({e})");
            return;
        }
    };
    slot.as_mut()
        .unwrap()
        .set()
        .file_list(&[&picture, &letter])
        .expect("putting a file list on the clipboard");

    // What a paste checks first finds nothing: the clipboard holds paths, not pixels.
    assert!(
        clipboard_image(&mut slot).is_none(),
        "a copied file is not an image on the clipboard"
    );
    let files = clipboard_files(&mut slot);
    let canon = |p: &std::path::Path| std::fs::canonicalize(p).unwrap();
    let back: Vec<PathBuf> = files.iter().map(|p| canon(p)).collect();
    assert_eq!(back, vec![canon(&picture), canon(&letter)]);
    let (images, others) = split_copied_files(files);
    assert_eq!(images.len(), 1, "the picture is staged");
    assert_eq!(others.len(), 1, "the text file is only named");
    eprintln!("clipboard file list round trip: {images:?} staged, {others:?} named");
}

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

/// What a typed `/image paste` would say **in this session** with nothing to paste —
/// read-only, so nothing the developer copied is lost. Run in each desktop session of
/// the Linux stand (docs/research/pasted-image-names.md §3.4) to read the words a person
/// would: on GNOME with XWayland off the clipboard does not open, and the refusal must
/// say so rather than call it empty.
///
/// Run: `cargo test clipboard_refusal_in_this_session -- --ignored --nocapture`.
#[test]
#[ignore = "reads the real system clipboard, which the session decides"]
fn clipboard_refusal_in_this_session() {
    let loc = crate::shared::i18n::locale(crate::shared::i18n::Lang::En);
    let mut slot: Option<arboard::Clipboard> = None;
    let image = clipboard_image(&mut slot).map(|(w, h, _)| (w, h));
    let unreachable = clipboard_unreachable(&mut slot);
    let msg = nothing_to_paste(&[], clipboard_is_far(), unreachable.as_ref(), loc);
    if unreachable.is_some() {
        assert_ne!(msg, loc.t("ui.err.image_no_clipboard"), "not called empty");
    }
    eprintln!("clipboard here: image {image:?}, unreachable {unreachable:?}; refusal: {msg}");
}
