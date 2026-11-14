//! What the window frame xd draws for itself has to do on each desktop.
//!
//! GPUI hides the system title bar on every platform xd runs on, because the
//! window asks for a transparent one. What is left differs: Linux
//! compositors hand the whole frame to the app, macOS keeps its traffic lights
//! and its own resize border, and Windows keeps an invisible resize border
//! outside the window and asks the app, pixel by pixel, which part of the
//! frame the pointer is over.
//!
//! The platform is a parameter, not a `cfg`, so the Windows answers are tested
//! on the Linux machines that run the test suite.

/// Segoe's caption glyphs, at the code points Windows itself uses.
pub const CAPTION_MINIMIZE: &str = "\u{e921}";
pub const CAPTION_MAXIMIZE: &str = "\u{e922}";
pub const CAPTION_RESTORE: &str = "\u{e923}";
pub const CAPTION_CLOSE: &str = "\u{e8bb}";

/// Windows 11 ships the first font and Windows 10 the second, with the caption
/// glyphs at the same code points.
const CAPTION_FONTS: [&str; 2] = ["Segoe Fluent Icons", "Segoe MDL2 Assets"];

/// Width of one Windows caption button, as the system draws its own.
pub const CAPTION_BUTTON_WIDTH: f32 = 46.0;
/// The size the Segoe caption glyphs are drawn at: an icon that scales with
/// its button, like the app's other icons, so not text under the caption floor.
pub const CAPTION_GLYPH_SIZE: f32 = 10.0;

/// The red Windows puts behind a hovered close button, and behind a pressed one.
pub const CAPTION_CLOSE_HOVER: u32 = 0xc42b1c;
pub const CAPTION_CLOSE_PRESSED: u32 = 0xb3271a;

/// Whether xd draws the title bar: the drag region, and off macOS the
/// minimize, maximize and close buttons.
///
/// Windows never reports client-side decorations, yet its system title bar is
/// hidden all the same, so without this the window has nothing to drag it by
/// and no way to close it.
pub fn draws_titlebar(os: &str, client_decorations: bool) -> bool {
    client_decorations || matches!(os, "macos" | "windows")
}

/// Whether the app draws its own minimize, maximize and close buttons.
pub fn draws_window_controls(os: &str, titlebar: bool) -> bool {
    titlebar && os != "macos"
}

/// The edges of the window the app has to make resizable itself.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ResizeEdges {
    /// Every edge, by starting a resize when one is pressed.
    All {
        thickness: f32,
    },
    /// Only the top edge, by leaving it to the system.
    ///
    /// Windows resizes from a border outside the window on the other three
    /// sides, and has none along the top of a window without a title bar. A
    /// strip inside the window on those three sides would show a resize
    /// pointer over something that cannot resize.
    Top {
        thickness: f32,
    },
    None,
}

/// GPUI resizes a Windows window from its top edge only where nothing claims
/// the pointer first, and only this far down.
const WINDOWS_TOP_RESIZE: f32 = 4.0;

const CLIENT_RESIZE: f32 = 6.0;

pub fn resize_edges(os: &str, maximized: bool, fullscreen: bool) -> ResizeEdges {
    if os != "windows" {
        return ResizeEdges::All {
            thickness: CLIENT_RESIZE,
        };
    }
    // A maximized window has no edge to drag, and its buttons reach the top of
    // the screen so the pointer cannot overshoot them.
    if maximized || fullscreen {
        ResizeEdges::None
    } else {
        ResizeEdges::Top {
            thickness: WINDOWS_TOP_RESIZE,
        }
    }
}

/// A rectangle in the logical pixels GPUI places windows in.
#[cfg(any(target_os = "windows", test))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// How many physical pixels of `monitor` make one of `screen`'s logical ones.
/// `monitor` is left, top, right and bottom as Windows reports it; `screen`
/// is the same monitor as GPUI describes it.
#[cfg(any(target_os = "windows", test))]
pub fn monitor_scale(screen: Rect, monitor: [i32; 4]) -> Option<f32> {
    let scale = (monitor[2] - monitor[0]) as f32 / screen.width;
    (scale.is_finite() && scale > 0.0).then_some(scale)
}

/// The part of a screen a window's bounds may cover, in logical pixels, from
/// the monitor and its work area as Windows reports them in physical ones.
///
/// `frame` is the thickness, in physical pixels, of the invisible border
/// Windows resizes a window by. GPUI places a window as if that border were
/// as thick above the window as below it, when above there is next to none,
/// so the window lands half a border higher than its bounds say. The room
/// starts that much lower, and a window given all of it has its title bar
/// just inside the screen instead of just outside.
#[cfg(any(target_os = "windows", test))]
pub fn work_area(screen: Rect, monitor: [i32; 4], work: [i32; 4], frame: i32) -> Option<Rect> {
    let scale = monitor_scale(screen, monitor)?;
    let lift = (frame / 2 + 1) as f32 / scale;
    let area = Rect {
        x: screen.x + (work[0] - monitor[0]) as f32 / scale,
        y: screen.y + (work[1] - monitor[1]) as f32 / scale + lift,
        width: (work[2] - work[0]) as f32 / scale,
        height: (work[3] - work[1]) as f32 / scale - lift,
    };
    (area.width > 0.0 && area.height > 0.0).then_some(area)
}

#[cfg(any(target_os = "windows", test))]
/// Where a window that was `width` by `height` opens so that it is all on
/// screen, centred in `area`.
///
/// The remembered size can be larger than the screen: the default is, on a
/// laptop at 150% scaling, and so is a size saved on a bigger monitor. Centred
/// as it was, such a window opened with its title bar above the top of the
/// screen, out of reach. A window that cannot shrink enough keeps its minimum
/// size and starts at the corner, so the title bar at least is on screen.
pub fn fit_window(width: f32, height: f32, minimum: (f32, f32), area: Rect) -> Rect {
    let width = width.min(area.width).max(minimum.0);
    let height = height.min(area.height).max(minimum.1);
    Rect {
        x: area.x + ((area.width - width) / 2.0).max(0.0),
        y: area.y + ((area.height - height) / 2.0).max(0.0),
        width,
        height,
    }
}

/// Whether a window's position is where Windows parks one while it is
/// minimized: -32000 in physical pixels on both axes, on any monitor layout.
pub fn parked_while_minimized(x: f32, y: f32) -> bool {
    // Scaling divides the physical position, by at most five.
    x <= -6000.0 && y <= -6000.0
}

/// The installed font that draws the caption glyphs, if there is one.
pub fn caption_font<'a>(installed: impl IntoIterator<Item = &'a str>) -> Option<&'static str> {
    let installed = installed.into_iter().collect::<Vec<_>>();
    CAPTION_FONTS
        .into_iter()
        .find(|font| installed.contains(font))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_draws_its_title_bar_without_client_decorations() {
        // GPUI reports server decorations on Windows while hiding the title bar.
        assert!(draws_titlebar("windows", false));
        assert!(draws_titlebar("macos", false));
        assert!(draws_titlebar("linux", true));
        // A Linux compositor that draws the frame keeps its own title bar.
        assert!(!draws_titlebar("linux", false));
    }

    #[test]
    fn every_desktop_but_macos_draws_its_window_buttons() {
        assert!(draws_window_controls("windows", true));
        assert!(draws_window_controls("linux", true));
        assert!(!draws_window_controls("macos", true));
        assert!(!draws_window_controls("linux", false));
    }

    #[test]
    fn windows_leaves_three_edges_to_the_system_border() {
        assert_eq!(
            resize_edges("windows", false, false),
            ResizeEdges::Top { thickness: 4.0 }
        );
        assert_eq!(
            resize_edges("linux", false, false),
            ResizeEdges::All { thickness: 6.0 }
        );
    }

    #[test]
    fn a_maximized_windows_window_keeps_its_buttons_against_the_screen_edge() {
        assert_eq!(resize_edges("windows", true, false), ResizeEdges::None);
        assert_eq!(resize_edges("windows", false, true), ResizeEdges::None);
    }

    #[test]
    fn the_work_area_is_converted_from_physical_to_logical_pixels() {
        // A 1920x1080 laptop panel at 150%, with a 72-pixel taskbar and the
        // 12-pixel resize border Windows uses at that scaling.
        let screen = Rect {
            x: 0.0,
            y: 0.0,
            width: 1280.0,
            height: 720.0,
        };
        assert_eq!(monitor_scale(screen, [0, 0, 1920, 1080]), Some(1.5));
        // Seven physical pixels are kept clear above: half the border, and one.
        let lift = 7.0 / 1.5;
        assert_eq!(
            work_area(screen, [0, 0, 1920, 1080], [0, 0, 1920, 1008], 12),
            Some(Rect {
                x: 0.0,
                y: lift,
                width: 1280.0,
                height: 672.0 - lift,
            })
        );
        // A taskbar on the left moves the origin as well.
        assert_eq!(
            work_area(screen, [0, 0, 1920, 1080], [90, 0, 1920, 1080], 12),
            Some(Rect {
                x: 60.0,
                y: lift,
                width: 1220.0,
                height: 720.0 - lift,
            })
        );
        assert_eq!(work_area(screen, [0, 0, 0, 0], [0, 0, 0, 0], 12), None);
    }

    #[test]
    fn a_window_given_the_whole_work_area_keeps_its_frame_on_screen() {
        // Measured on Windows: at 100% the border is 8 pixels, and a window
        // whose bounds started at 0 was drawn from 4 pixels above the screen
        // to 2 short of its bounds' end.
        let screen = Rect {
            x: 0.0,
            y: 0.0,
            width: 1280.0,
            height: 720.0,
        };
        let area = work_area(screen, [0, 0, 1280, 720], [0, 0, 1280, 672], 8).unwrap();
        let window = fit_window(1180.0, 780.0, (760.0, 560.0), area);
        let drawn_from = window.y - 4.0;
        let drawn_to = window.y + window.height - 2.0;
        assert!((0.0..=2.0).contains(&drawn_from), "{drawn_from}");
        assert!(drawn_to <= 672.0, "{drawn_to}");
    }

    #[test]
    fn the_default_window_fits_a_laptop_screen_with_its_title_bar_in_reach() {
        let area = Rect {
            x: 0.0,
            y: 0.0,
            width: 1280.0,
            height: 672.0,
        };
        // 1180x780 centred on the 720-pixel screen started 30 pixels above it.
        let window = fit_window(1180.0, 780.0, (760.0, 560.0), area);
        assert_eq!(
            window,
            Rect {
                x: 50.0,
                y: 0.0,
                width: 1180.0,
                height: 672.0,
            }
        );
    }

    #[test]
    fn a_window_that_fits_is_centred_and_keeps_its_size() {
        let area = Rect {
            x: 0.0,
            y: 0.0,
            width: 1920.0,
            height: 1032.0,
        };
        assert_eq!(
            fit_window(1180.0, 780.0, (760.0, 560.0), area),
            Rect {
                x: 370.0,
                y: 126.0,
                width: 1180.0,
                height: 780.0,
            }
        );
    }

    #[test]
    fn a_screen_smaller_than_the_minimum_keeps_the_title_bar_on_it() {
        let area = Rect {
            x: 10.0,
            y: 20.0,
            width: 640.0,
            height: 440.0,
        };
        assert_eq!(
            fit_window(1180.0, 780.0, (760.0, 560.0), area),
            Rect {
                x: 10.0,
                y: 20.0,
                width: 760.0,
                height: 560.0,
            }
        );
    }

    #[test]
    fn a_minimized_window_is_told_from_one_on_a_monitor_to_the_left() {
        assert!(parked_while_minimized(-32000.0, -32000.0));
        // The same position at 250% scaling.
        assert!(parked_while_minimized(-12800.0, -12800.0));
        // A window on a second monitor left of and above the primary one.
        assert!(!parked_while_minimized(-1920.0, -1080.0));
        assert!(!parked_while_minimized(-32000.0, 100.0));
        assert!(!parked_while_minimized(120.0, 80.0));
    }

    #[test]
    fn caption_glyphs_use_the_newest_installed_segoe_font() {
        assert_eq!(
            caption_font(["Arial", "Segoe MDL2 Assets", "Segoe Fluent Icons"]),
            Some("Segoe Fluent Icons")
        );
        assert_eq!(
            caption_font(["Segoe MDL2 Assets"]),
            Some("Segoe MDL2 Assets")
        );
        assert_eq!(caption_font(["Arial"]), None);
    }
}
