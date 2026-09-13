use std::cell::{Cell, RefCell};
use std::path::Path;
use std::process::Command;
use std::rc::Rc;

use objc2::ClassType;
use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{NSButton, NSColorSpace, NSImage, NSView};
use objc2_foundation::{NSData, NSDate, NSError, NSObjectProtocol, NSRunLoop, NSString};
use objc2_web_kit::WKWebView;

const CALLBACK_TIMEOUT_SECONDS: f64 = 5.0;
const FRAME_COUNT: usize = 12;
const FRAME_INTERVAL_SECONDS: f64 = 0.25;
const BUTTON_INTERIOR_INSET: f64 = 4.0;
const BUTTON_SAMPLE_INTERVAL: usize = 1;
const MIN_INTERIOR_LUMINANCE_CONTRAST: f64 = 0.2;
const CONTROL_BUTTON_COUNT: usize = 10;

type Authorizer<'a> = dyn Fn() -> Result<(), String> + 'a;

pub(super) fn pump(seconds: f64) {
    objc2::rc::autoreleasepool(|_| {
        let deadline = NSDate::dateWithTimeIntervalSinceNow(seconds.max(0.0));
        NSRunLoop::currentRunLoop().runUntilDate(&deadline);
    });
}

pub(super) fn evaluate(view: &WKWebView, expression: &str) -> Result<String, String> {
    objc2::rc::autoreleasepool(|_| evaluate_inner(view, expression))
}

fn evaluate_inner(view: &WKWebView, expression: &str) -> Result<String, String> {
    let result = Rc::new(RefCell::new(None));
    let callback_result = Rc::clone(&result);
    let callback = block2::RcBlock::new(move |value: *mut AnyObject, error: *mut NSError| {
        let outcome = if error.is_null() {
            stringify_object(value)
        } else {
            Err(stringify_object(error.cast::<AnyObject>())
                .unwrap_or_else(|_| "JavaScript error".into()))
        };
        *callback_result.borrow_mut() = Some(outcome);
    });
    unsafe {
        view.evaluateJavaScript_completionHandler(&NSString::from_str(expression), Some(&callback))
    };
    wait_for(&result).ok_or_else(|| "JavaScript callback timed out".to_string())?
}

pub(super) fn snapshot_guarded(
    view: &WKWebView,
    path: &Path,
    authorize: &Authorizer<'_>,
) -> Result<(), String> {
    authorize()?;
    let data = snapshot_data(view)?;
    authorize()?;
    write_tiff(data, path)
}

fn snapshot_data(view: &WKWebView) -> Result<Retained<NSData>, String> {
    objc2::rc::autoreleasepool(|_| snapshot_data_inner(view))
}

fn snapshot_data_inner(view: &WKWebView) -> Result<Retained<NSData>, String> {
    let result = Rc::new(RefCell::new(None));
    let callback_result = Rc::clone(&result);
    let is_active = Rc::new(Cell::new(true));
    let callback_active = Rc::clone(&is_active);
    let callback = block2::RcBlock::new(move |image: *mut NSImage, error: *mut NSError| {
        if !callback_active.get() {
            return;
        }
        let outcome = if !error.is_null() {
            Err(stringify_object(error.cast::<AnyObject>())
                .unwrap_or_else(|_| "snapshot error".into()))
        } else if image.is_null() {
            Err("snapshot returned a null image".into())
        } else {
            // SAFETY: WebKit owns this nonnull image for the completion callback duration.
            let image = unsafe { &*image };
            image
                .TIFFRepresentation()
                .ok_or_else(|| "capture returned no TIFF data".into())
        };
        *callback_result.borrow_mut() = Some(outcome);
    });
    unsafe { view.takeSnapshotWithConfiguration_completionHandler(None, &callback) };
    let outcome = wait_for(&result);
    is_active.set(false);
    outcome.ok_or_else(|| "snapshot callback timed out".to_string())?
}

pub(super) fn record_guarded(
    view: &WKWebView,
    directory: &Path,
    authorize: &Authorizer<'_>,
) -> Result<(), String> {
    authorize()?;
    std::fs::create_dir_all(directory)
        .map_err(|error| format!("create recording directory: {error}"))?;
    let visibility = evaluate(view, "String(document.visibilityState)")?;
    authorize()?;
    std::fs::write(directory.join("visibility.txt"), visibility)
        .map_err(|error| error.to_string())?;
    let mut counters = Vec::with_capacity(FRAME_COUNT);
    for index in 0..FRAME_COUNT {
        let frame = directory.join(format!("frame-{index:03}.tiff"));
        snapshot_guarded(view, &frame, authorize)?;
        let counter = evaluate(
            view,
            "String(document.querySelector('#frame')?.textContent ?? '')",
        )?;
        authorize()?;
        counters.push(counter);
        if index + 1 < FRAME_COUNT {
            pump(FRAME_INTERVAL_SECONDS);
        }
    }
    authorize()?;
    std::fs::write(
        directory.join("frame-counters.txt"),
        counters.join("\n") + "\n",
    )
    .map_err(|error| format!("write frame counters: {error}"))?;
    if counters.windows(2).all(|pair| pair[0] == pair[1]) {
        return Err("frame counter did not change during recording".into());
    }
    authorize()?;
    let video = directory.join("recording.mp4");
    run_ffmpeg(
        directory,
        "ffmpeg-encode.log",
        [
            "-y",
            "-threads",
            "2",
            "-filter_threads",
            "1",
            "-framerate",
            "4",
            "-start_number",
            "0",
            "-i",
            "frame-%03d.tiff",
            "-frames:v",
            "12",
            "-threads",
            "2",
            "-pix_fmt",
            "yuv420p",
            "recording.mp4",
        ],
    )?;
    if !video.is_file()
        || std::fs::metadata(&video)
            .map_err(|error| error.to_string())?
            .len()
            == 0
    {
        return Err("ffmpeg did not create recording.mp4".into());
    }
    run_ffmpeg(
        directory,
        "ffmpeg-playback.log",
        [
            "-v",
            "error",
            "-threads",
            "2",
            "-i",
            "recording.mp4",
            "-f",
            "null",
            "-",
        ],
    )?;
    run_ffmpeg(
        directory,
        "ffmpeg-decode.log",
        [
            "-y",
            "-v",
            "error",
            "-threads",
            "2",
            "-filter_threads",
            "1",
            "-i",
            "recording.mp4",
            "-frames:v",
            "2",
            "decoded-%02d.png",
        ],
    )?;
    let first = std::fs::read(directory.join("decoded-01.png"))
        .map_err(|error| format!("read decoded frame 1: {error}"))?;
    let second = std::fs::read(directory.join("decoded-02.png"))
        .map_err(|error| format!("read decoded frame 2: {error}"))?;
    if first.is_empty() || first == second {
        return Err("decoded recording frames were blank or static".into());
    }
    Ok(())
}

pub(super) fn snapshot_controls(view: &NSView, path: &Path) -> Result<(), String> {
    if is_web_view_descendant(view) {
        return Err("control snapshot rejected: view contains a WKWebView".into());
    }
    let parent = path
        .parent()
        .ok_or_else(|| "control snapshot path has no parent".to_string())?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("create control snapshot directory: {error}"))?;
    let rect = view.bounds();
    view.display();
    let rep = view
        .bitmapImageRepForCachingDisplayInRect(rect)
        .ok_or_else(|| "AppKit could not allocate a bitmap representation".to_string())?;
    view.cacheDisplayInRect_toBitmapImageRep(rect, &rep);
    assert_button_interior_visible_contrast(view, &rep)?;
    write_tiff(
        rep.TIFFRepresentation()
            .ok_or_else(|| "capture returned no TIFF data".to_string())?,
        path,
    )
}

fn assert_button_interior_visible_contrast(
    view: &NSView,
    rep: &objc2_app_kit::NSBitmapImageRep,
) -> Result<(), String> {
    objc2::rc::autoreleasepool(|_| {
        let bounds = view.bounds();
        let x_scale = rep.pixelsWide() as f64 / bounds.size.width;
        let y_scale = rep.pixelsHigh() as f64 / bounds.size.height;
        let device_rgb = NSColorSpace::deviceRGBColorSpace();
        let buttons: Vec<_> = view
            .subviews()
            .iter()
            .filter(|child| child.isKindOfClass(NSButton::class()))
            .collect();
        if buttons.len() != CONTROL_BUTTON_COUNT {
            return Err(format!(
                "control snapshot expected {CONTROL_BUTTON_COUNT} buttons, found {}",
                buttons.len()
            ));
        }
        for (index, button) in buttons.into_iter().enumerate() {
            let frame = button.frame();
            let start_x = ((frame.origin.x + BUTTON_INTERIOR_INSET - bounds.origin.x) * x_scale)
                .ceil() as isize;
            let end_x =
                ((frame.origin.x + frame.size.width - BUTTON_INTERIOR_INSET - bounds.origin.x)
                    * x_scale)
                    .floor() as isize;
            let start_y = rep.pixelsHigh()
                - ((frame.origin.y + frame.size.height - BUTTON_INTERIOR_INSET - bounds.origin.y)
                    * y_scale)
                    .floor() as isize;
            let end_y = rep.pixelsHigh()
                - ((frame.origin.y + BUTTON_INTERIOR_INSET - bounds.origin.y) * y_scale).ceil()
                    as isize;
            let samples: Vec<_> = (start_x..end_x)
                .step_by(BUTTON_SAMPLE_INTERVAL)
                .flat_map(|x| {
                    let device_rgb = device_rgb.clone();
                    (start_y..end_y)
                        .step_by(BUTTON_SAMPLE_INTERVAL)
                        .filter_map(move |y| {
                            rep.colorAtX_y(x, y)
                                .and_then(|color| color.colorUsingColorSpace(&device_rgb))
                                .map(|color| {
                                    (
                                        color.redComponent(),
                                        color.greenComponent(),
                                        color.blueComponent(),
                                        color.alphaComponent(),
                                    )
                                })
                        })
                })
                .collect();
            if !is_button_interior_opaque(samples.iter().copied()) {
                return Err(format!(
                    "control snapshot rejected: button {index} interior lacks an opaque backdrop"
                ));
            }
            if !is_button_interior_contrast_visible(samples) {
                return Err(format!(
                    "control snapshot rejected: button {index} interior lacks visible contrast"
                ));
            }
        }
        Ok(())
    })
}

fn is_button_interior_opaque(samples: impl IntoIterator<Item = (f64, f64, f64, f64)>) -> bool {
    samples.into_iter().all(|(_, _, _, alpha)| alpha >= 1.0)
}

fn is_button_interior_contrast_visible(
    samples: impl IntoIterator<Item = (f64, f64, f64, f64)>,
) -> bool {
    let mut darkest_on_black: f64 = 1.0;
    let mut brightest_on_black: f64 = 0.0;
    let mut darkest_on_white: f64 = 1.0;
    let mut brightest_on_white: f64 = 0.0;
    let mut sample_count = 0;
    for (red, green, blue, alpha) in samples {
        let luminance = 0.2126 * red + 0.7152 * green + 0.0722 * blue;
        let luminance_on_black = alpha * luminance;
        let luminance_on_white = luminance_on_black + 1.0 - alpha;
        darkest_on_black = darkest_on_black.min(luminance_on_black);
        brightest_on_black = brightest_on_black.max(luminance_on_black);
        darkest_on_white = darkest_on_white.min(luminance_on_white);
        brightest_on_white = brightest_on_white.max(luminance_on_white);
        sample_count += 1;
    }
    sample_count > 0
        && (brightest_on_black - darkest_on_black >= MIN_INTERIOR_LUMINANCE_CONTRAST
            || brightest_on_white - darkest_on_white >= MIN_INTERIOR_LUMINANCE_CONTRAST)
}

pub(super) fn is_web_view_descendant(view: &NSView) -> bool {
    objc2::rc::autoreleasepool(|_| {
        view.isKindOfClass(WKWebView::class())
            || view
                .subviews()
                .iter()
                .any(|child| is_web_view_descendant(&child))
    })
}

fn wait_for<T>(result: &Rc<RefCell<Option<Result<T, String>>>>) -> Option<Result<T, String>> {
    let deadline = NSDate::dateWithTimeIntervalSinceNow(CALLBACK_TIMEOUT_SECONDS);
    while NSDate::date().timeIntervalSinceDate(&deadline) < 0.0 {
        if let Some(result) = result.borrow_mut().take() {
            return Some(result);
        }
        pump(0.01);
    }
    result.borrow_mut().take()
}

fn write_tiff(data: Retained<NSData>, path: &Path) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "snapshot path has no parent".to_string())?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("create snapshot directory: {error}"))?;
    if data.writeToFile_atomically(&NSString::from_str(&path.to_string_lossy()), true) {
        Ok(())
    } else {
        Err(format!("could not write {}", path.display()))
    }
}

fn stringify_object(object: *mut AnyObject) -> Result<String, String> {
    if object.is_null() {
        return Err("callback returned null".into());
    }
    // SAFETY: The callback owns the nonnull Objective-C object for this synchronous conversion.
    let description: Retained<NSString> = unsafe { msg_send![object, description] };
    Ok(description.to_string())
}

fn run_ffmpeg<const N: usize>(
    directory: &Path,
    log: &str,
    arguments: [&str; N],
) -> Result<(), String> {
    let output = Command::new("ffmpeg")
        .current_dir(directory)
        .args(arguments)
        .output()
        .map_err(|error| format!("spawn ffmpeg: {error}"))?;
    std::fs::write(
        directory.join(log),
        [&output.stdout[..], &output.stderr[..]].concat(),
    )
    .map_err(|error| format!("write ffmpeg log: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!("ffmpeg exited with {}", output.status))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_uniform_or_transparent_button_interiors() {
        for samples in [
            vec![(1.0, 1.0, 1.0, 1.0); 4],
            vec![(0.0, 0.0, 0.0, 1.0); 4],
            vec![(1.0, 0.0, 1.0, 0.0); 4],
        ] {
            assert!(!is_button_interior_contrast_visible(samples));
        }
    }

    #[test]
    fn accepts_opaque_interior_contrast() {
        assert!(is_button_interior_contrast_visible([
            (0.0, 0.0, 0.0, 1.0),
            (1.0, 1.0, 1.0, 1.0),
        ]));
    }

    #[test]
    fn rejects_non_opaque_button_interior_samples() {
        assert!(!is_button_interior_opaque([
            (0.0, 0.0, 0.0, 1.0),
            (1.0, 1.0, 1.0, 0.0),
        ]));
    }

    #[test]
    fn accepts_text_on_transparent_backdrops() {
        for foreground in [(1.0, 1.0, 1.0, 1.0), (0.0, 0.0, 0.0, 1.0)] {
            assert!(is_button_interior_contrast_visible([
                (0.0, 0.0, 0.0, 0.0),
                foreground,
            ]));
        }
    }

    #[test]
    fn rejects_invisible_color_variation() {
        assert!(!is_button_interior_contrast_visible([
            (1.0, 0.0, 0.0, 0.0),
            (0.0, 1.0, 0.0, 0.0),
            (0.0, 0.0, 1.0, 0.0),
        ]));
    }
}
