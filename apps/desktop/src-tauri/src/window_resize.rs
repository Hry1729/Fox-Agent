use windows::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
    UI::{
        HiDpi::GetDpiForWindow,
        Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
        WindowsAndMessaging::{
            GetCursorPos, GetWindowRect, SendMessageW, HTBOTTOM, HTBOTTOMLEFT, HTBOTTOMRIGHT,
            HTLEFT, HTRIGHT, HTTOP, HTTOPLEFT, HTTOPRIGHT, MINMAXINFO, WMSZ_BOTTOMLEFT,
            WMSZ_BOTTOMRIGHT, WMSZ_TOPLEFT, WMSZ_TOPRIGHT, WM_ENTERSIZEMOVE, WM_EXITSIZEMOVE,
            WM_GETMINMAXINFO, WM_NCDESTROY, WM_NCHITTEST, WM_NCLBUTTONDOWN, WM_SIZING,
        },
    },
};

const SUBCLASS_ID: usize = 0x464f5852;

#[derive(Default)]
struct ResizeState {
    start: Option<RECT>,
    limits: MINMAXINFO,
}

pub fn install(window: &tauri::WebviewWindow) -> Result<(), Box<dyn std::error::Error>> {
    let hwnd = HWND(window.hwnd()?.0 as _);
    install_on_window(hwnd)
}

fn install_on_window(hwnd: HWND) -> Result<(), Box<dyn std::error::Error>> {
    let state = Box::into_raw(Box::<ResizeState>::default());
    // Tauri setup runs on the window thread; subclass state stays on that thread.
    if !unsafe { SetWindowSubclass(hwnd, Some(resize_proc), SUBCLASS_ID, state as usize) }.as_bool()
    {
        unsafe { drop(Box::from_raw(state)) };
        return Err("Could not install Fox corner resize handler".into());
    }
    Ok(())
}

unsafe extern "system" fn resize_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    id: usize,
    data: usize,
) -> LRESULT {
    let state = data as *mut ResizeState;
    match message {
        WM_NCHITTEST => {
            let hit = DefSubclassProc(hwnd, message, wparam, lparam);
            return LRESULT(corner_hit(hwnd, hit.0 as u32) as i32 as isize);
        }
        WM_NCLBUTTONDOWN => {
            // Tauri's shadow resize child forwards top corners as HTTOP.
            // Resolve the physical pointer again before Windows begins its sizing loop.
            return DefSubclassProc(
                hwnd,
                message,
                WPARAM(corner_hit(hwnd, wparam.0 as u32) as usize),
                lparam,
            );
        }
        WM_ENTERSIZEMOVE => {
            let mut rect = RECT::default();
            let mut limits = MINMAXINFO::default();
            // Ask the existing window procedure for DPI-aware tracking limits.
            SendMessageW(
                hwnd,
                WM_GETMINMAXINFO,
                None,
                Some(LPARAM(&mut limits as *mut _ as isize)),
            );
            (*state).start = GetWindowRect(hwnd, &mut rect).ok().map(|_| rect);
            (*state).limits = limits;
        }
        WM_SIZING if lparam.0 != 0 => {
            if let Some(start) = (*state).start {
                let rect = &mut *(lparam.0 as *mut RECT);
                if constrain_corner(rect, start, wparam.0 as u32, &(*state).limits) {
                    return LRESULT(1);
                }
            }
        }
        WM_EXITSIZEMOVE => (*state).start = None,
        WM_NCDESTROY => {
            let _ = RemoveWindowSubclass(hwnd, Some(resize_proc), id);
            drop(Box::from_raw(state));
        }
        _ => {}
    }
    DefSubclassProc(hwnd, message, wparam, lparam)
}

unsafe fn corner_hit(hwnd: HWND, hit: u32) -> u32 {
    if ![HTTOP, HTBOTTOM, HTLEFT, HTRIGHT].contains(&hit) {
        return hit;
    }
    let mut point = POINT::default();
    let mut rect = RECT::default();
    if GetCursorPos(&mut point).is_err() || GetWindowRect(hwnd, &mut rect).is_err() {
        return hit;
    }
    let reach = (24 * GetDpiForWindow(hwnd).max(96) / 96) as i32;
    corner_at_point(rect, point, reach, hit)
}

fn corner_at_point(rect: RECT, point: POINT, reach: i32, hit: u32) -> u32 {
    let left = point.x < rect.left + reach;
    let right = point.x >= rect.right - reach;
    let top = point.y < rect.top + reach;
    let bottom = point.y >= rect.bottom - reach;
    match (left, right, top, bottom) {
        (true, _, true, _) => HTTOPLEFT,
        (_, true, true, _) => HTTOPRIGHT,
        (true, _, _, true) => HTBOTTOMLEFT,
        (_, true, _, true) => HTBOTTOMRIGHT,
        _ => hit,
    }
}

fn constrain_corner(rect: &mut RECT, start: RECT, edge: u32, limits: &MINMAXINFO) -> bool {
    let (left, top) = match edge {
        WMSZ_TOPLEFT => (true, true),
        WMSZ_TOPRIGHT => (false, true),
        WMSZ_BOTTOMLEFT => (true, false),
        WMSZ_BOTTOMRIGHT => (false, false),
        _ => return false,
    };
    let width = f64::from(start.right - start.left);
    let height = f64::from(start.bottom - start.top);
    if width <= 0.0 || height <= 0.0 {
        return false;
    }
    let proposed_width = f64::from(if left {
        start.right - rect.left
    } else {
        rect.right - start.left
    });
    let proposed_height = f64::from(if top {
        start.bottom - rect.top
    } else {
        rect.bottom - start.top
    });
    // Project the dragged corner onto the original aspect-ratio diagonal.
    let scale =
        (proposed_width * width + proposed_height * height) / (width * width + height * height);
    let min_scale = (f64::from(limits.ptMinTrackSize.x.max(1)) / width)
        .max(f64::from(limits.ptMinTrackSize.y.max(1)) / height);
    let max_width = if limits.ptMaxTrackSize.x > 0 {
        f64::from(limits.ptMaxTrackSize.x) / width
    } else {
        f64::INFINITY
    };
    let max_height = if limits.ptMaxTrackSize.y > 0 {
        f64::from(limits.ptMaxTrackSize.y) / height
    } else {
        f64::INFINITY
    };
    let scale = scale
        .max(min_scale)
        .min(max_width.min(max_height).max(min_scale));
    let width = (width * scale).round() as i32;
    let height = (height * scale).round() as i32;
    if left {
        rect.right = start.right;
        rect.left = start.right - width;
    } else {
        rect.left = start.left;
        rect.right = start.left + width;
    }
    if top {
        rect.bottom = start.bottom;
        rect.top = start.bottom - height;
    } else {
        rect.top = start.top;
        rect.bottom = start.top + height;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shadow_border_corners_are_distinct_from_edge_centres() {
        let rect = RECT {
            left: 100,
            top: 100,
            right: 1220,
            bottom: 900,
        };
        for (x, y, hit, expected) in [
            (103, 103, HTTOP, HTTOPLEFT),
            (1217, 103, HTTOP, HTTOPRIGHT),
            (103, 897, HTLEFT, HTBOTTOMLEFT),
            (1217, 897, HTRIGHT, HTBOTTOMRIGHT),
            (660, 103, HTTOP, HTTOP),
            (660, 897, HTBOTTOM, HTBOTTOM),
            (103, 500, HTLEFT, HTLEFT),
            (1217, 500, HTRIGHT, HTRIGHT),
        ] {
            assert_eq!(corner_at_point(rect, POINT { x, y }, 24, hit), expected);
        }
    }

    #[test]
    fn native_messages_capture_each_gesture_and_release_state() {
        use windows::{
            core::w,
            Win32::UI::WindowsAndMessaging::{
                CreateWindowExW, DestroyWindow, SetWindowPos, SWP_NOACTIVATE, SWP_NOZORDER,
                WINDOW_EX_STYLE, WS_OVERLAPPEDWINDOW,
            },
        };
        unsafe {
            // A hidden native window exercises the installed subclass without touching Fox.
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("STATIC"),
                w!("Fox resize test"),
                WS_OVERLAPPEDWINDOW,
                100,
                100,
                1120,
                800,
                None,
                None,
                None,
                None,
            )
            .unwrap();
            install_on_window(hwnd).unwrap();
            for (width, height) in [(1120, 800), (960, 800)] {
                SetWindowPos(
                    hwnd,
                    None,
                    100,
                    100,
                    width,
                    height,
                    SWP_NOACTIVATE | SWP_NOZORDER,
                )
                .unwrap();
                // Windows clamps a window to the screen's tracking limits (the CI runner
                // has a 1024x768 desktop), so the requested size is not necessarily the
                // size the window got. The gesture keeps the ratio of the window as it
                // actually is when the size-move loop starts.
                let mut actual = RECT::default();
                GetWindowRect(hwnd, &mut actual).unwrap();
                let (width, height) = (actual.right - actual.left, actual.bottom - actual.top);
                assert!(width > 0 && height > 0);
                SendMessageW(hwnd, WM_ENTERSIZEMOVE, None, None);
                let mut rect = RECT {
                    left: 100,
                    top: 100,
                    right: 1500,
                    bottom: 1000,
                };
                SendMessageW(
                    hwnd,
                    WM_SIZING,
                    Some(WPARAM(WMSZ_BOTTOMRIGHT as usize)),
                    Some(LPARAM(&mut rect as *mut _ as isize)),
                );
                let expected = width as f64 / height as f64;
                let resized_height = (rect.bottom - rect.top) as f64;
                let ratio = (rect.right - rect.left) as f64 / resized_height;
                // Both sides are rounded to whole pixels; allow that rounding, with the
                // original 0.002 as the floor.
                let tolerance = 0.002f64.max((1.0 + expected) / resized_height);
                assert!(
                    (ratio - expected).abs() < tolerance,
                    "ratio {ratio} vs window {width}x{height} ({expected}), rect {:?}",
                    (rect.left, rect.top, rect.right, rect.bottom)
                );
                SendMessageW(hwnd, WM_EXITSIZEMOVE, None, None);
                let mut unchanged = RECT {
                    left: 0,
                    top: 0,
                    right: 777,
                    bottom: 555,
                };
                SendMessageW(
                    hwnd,
                    WM_SIZING,
                    Some(WPARAM(WMSZ_BOTTOMRIGHT as usize)),
                    Some(LPARAM(&mut unchanged as *mut _ as isize)),
                );
                assert_eq!((unchanged.right, unchanged.bottom), (777, 555));
            }
            DestroyWindow(hwnd).unwrap();
        }
    }

    #[test]
    fn corners_preserve_ratio_and_opposite_corner() {
        let start = RECT {
            left: -400,
            top: 80,
            right: 720,
            bottom: 880,
        };
        for edge in [
            WMSZ_TOPLEFT,
            WMSZ_TOPRIGHT,
            WMSZ_BOTTOMLEFT,
            WMSZ_BOTTOMRIGHT,
        ] {
            let mut rect = RECT {
                left: -520,
                top: 30,
                right: 830,
                bottom: 970,
            };
            assert!(constrain_corner(
                &mut rect,
                start,
                edge,
                &MINMAXINFO::default()
            ));
            assert!(
                ((rect.right - rect.left) as f64 / (rect.bottom - rect.top) as f64 - 1.4).abs()
                    < 0.002
            );
            assert_eq!(
                if edge == WMSZ_TOPLEFT || edge == WMSZ_BOTTOMLEFT {
                    rect.right
                } else {
                    rect.left
                },
                if edge == WMSZ_TOPLEFT || edge == WMSZ_BOTTOMLEFT {
                    start.right
                } else {
                    start.left
                }
            );
            assert_eq!(
                if edge == WMSZ_TOPLEFT || edge == WMSZ_TOPRIGHT {
                    rect.bottom
                } else {
                    rect.top
                },
                if edge == WMSZ_TOPLEFT || edge == WMSZ_TOPRIGHT {
                    start.bottom
                } else {
                    start.top
                }
            );
        }
    }

    #[test]
    fn edges_are_unchanged() {
        for edge in [1, 2, 3, 6] {
            let mut rect = RECT {
                left: 10,
                top: 20,
                right: 800,
                bottom: 600,
            };
            let before = rect;
            assert!(!constrain_corner(
                &mut rect,
                before,
                edge,
                &MINMAXINFO::default()
            ));
            assert_eq!(
                (rect.left, rect.top, rect.right, rect.bottom),
                (10, 20, 800, 600)
            );
        }
    }

    #[test]
    fn respects_tracking_limits_and_new_start_ratio() {
        let start = RECT {
            left: 0,
            top: 0,
            right: 1000,
            bottom: 1000,
        };
        let mut limits = MINMAXINFO::default();
        limits.ptMinTrackSize.x = 600;
        limits.ptMinTrackSize.y = 560;
        limits.ptMaxTrackSize.x = 1600;
        limits.ptMaxTrackSize.y = 1200;
        for (proposed, expected) in [(100, 600), (4000, 1200), (900, 900)] {
            let mut rect = RECT {
                right: proposed,
                bottom: proposed,
                ..start
            };
            constrain_corner(&mut rect, start, WMSZ_BOTTOMRIGHT, &limits);
            assert_eq!((rect.right, rect.bottom), (expected, expected));
        }
    }
}
