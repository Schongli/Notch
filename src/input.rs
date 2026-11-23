// A small dark box under the notch for typing a task or a Jot. A real EDIT control does the text work
// (caret, selection, IME, clipboard), so none of that is hand-written. Enter sends, Esc or clicking away cancels.
use std::cell::RefCell;

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;

/// Posted to the notch when the user presses Enter; wParam is the `Ask` as usize. Read the words with `take()`.
pub const WM_ASKED: u32 = WM_APP + 8;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Ask {
    Task,
    Jot,
}

struct Box_ {
    hwnd: HWND,
    edit: HWND,
    notch: HWND,
    kind: Ask,
    label: Vec<u16>,
    font: HFONT,
    small: HFONT,
    back: HBRUSH,
    field: HBRUSH,
    /// Where focus goes back to after the box closes
    before: HWND,
    text: String,
}

thread_local! {
    static BOX: RefCell<Option<Box_>> = const { RefCell::new(None) };
}

const W: i32 = 420;
const H: i32 = 76;
const BACK: COLORREF = COLORREF(0x001C1C1C); // 0x00BBGGRR
const FIELD: COLORREF = COLORREF(0x002A2A2A);

fn scale(v: i32, dpi: u32) -> i32 {
    v * dpi as i32 / 96
}

/// Opens the box centered at `cx`, its top at `y` (screen pixels).
pub fn ask(notch: HWND, kind: Ask, cx: i32, y: i32, dpi: u32) {
    let label = match kind {
        Ask::Task => "New task · add a time like 15:30 for a reminder",
        Ask::Jot => "Jot an idea · Enter saves it",
    };
    unsafe {
        let before = GetForegroundWindow();
        let fresh = BOX.with(|b| b.borrow().is_none());
        if fresh && create(notch, dpi).is_err() {
            return;
        }
        // Set state first, then call into windows with no borrow held: they send messages that read it.
        let Some((hwnd, edit)) = BOX.with(|b| {
            let mut b = b.borrow_mut();
            let x = b.as_mut()?;
            x.kind = kind;
            x.label = label.encode_utf16().collect();
            if before != x.hwnd {
                x.before = before;
            }
            Some((x.hwnd, x.edit))
        }) else {
            return;
        };
        let (w, h) = (scale(W, dpi), scale(H, dpi));
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), cx - w / 2, y, w, h, SWP_SHOWWINDOW);
        let _ = SetWindowTextW(edit, w!(""));
        let _ = InvalidateRect(Some(hwnd), None, true);
        let _ = SetForegroundWindow(hwnd);
        let _ = SetFocus(Some(edit));
    }
}

/// The words typed into the box, once.
pub fn take() -> String {
    BOX.with(|b| b.borrow_mut().as_mut().map(|x| std::mem::take(&mut x.text)).unwrap_or_default())
}

unsafe fn create(notch: HWND, dpi: u32) -> Result<()> {
    unsafe {
        let instance: HINSTANCE = GetModuleHandleW(None)?.into();
        let back = CreateSolidBrush(BACK);
        let class = w!("NotchAsk");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(box_proc),
            hInstance: instance,
            lpszClassName: class,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            hbrBackground: back,
            ..Default::default()
        };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            class,
            w!("notch input"),
            WS_POPUP,
            0,
            0,
            scale(W, dpi),
            scale(H, dpi),
            None,
            None,
            Some(instance),
            None,
        )?;
        // Rounded corners and a quiet border on Windows 11; ignored elsewhere.
        let round = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &round as *const _ as _, size_of_val(&round) as u32);
        let border = COLORREF(0x00383838);
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_BORDER_COLOR, &border as *const _ as _, size_of_val(&border) as u32);
        let font_of = |px: i32, weight: i32| {
            CreateFontW(
                -scale(px, dpi),
                0,
                0,
                0,
                weight,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                CLEARTYPE_QUALITY,
                0,
                w!("Segoe UI Variable Text"),
            )
        };
        let (font, small) = (font_of(17, 400), font_of(12, 400));
        let pad = scale(16, dpi);
        let edit = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("EDIT"),
            w!(""),
            WS_CHILD | WS_VISIBLE | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
            pad,
            scale(34, dpi),
            scale(W, dpi) - 2 * pad,
            scale(28, dpi),
            Some(hwnd),
            None,
            Some(instance),
            None,
        )?;
        SendMessageW(edit, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(1)));
        // Classic subclassing (no comctl32 needed): catch Enter and Esc before the EDIT beeps at them.
        EDIT_PROC.store(GetWindowLongPtrW(edit, GWLP_WNDPROC), std::sync::atomic::Ordering::Relaxed);
        SetWindowLongPtrW(edit, GWLP_WNDPROC, edit_hook as *const () as isize);
        BOX.with(|b| {
            *b.borrow_mut() = Some(Box_ {
                hwnd,
                edit,
                notch,
                kind: Ask::Task,
                label: Vec::new(),
                font,
                small,
                back,
                field: CreateSolidBrush(FIELD),
                before: HWND::default(),
                text: String::new(),
            })
        });
        Ok(())
    }
}

/// Hides the box and hands focus back; `send` posts the words to the notch.
/// Hiding sends WM_ACTIVATE right away, which comes back in here, so no borrow is held across it.
fn close(send: bool) {
    let Some((hwnd, edit, notch, kind, before)) = BOX.with(|b| b.borrow().as_ref().map(|x| (x.hwnd, x.edit, x.notch, x.kind, x.before))) else {
        return;
    };
    let mut text = String::new();
    if send {
        let mut buf = [0u16; 1024];
        let n = unsafe { GetWindowTextW(edit, &mut buf) };
        text = String::from_utf16_lossy(&buf[..n.max(0) as usize]).trim().to_string();
    }
    BOX.with(|b| {
        if let Some(x) = b.borrow_mut().as_mut() {
            x.before = HWND::default();
            if send {
                x.text = text.clone();
            }
        }
    });
    unsafe {
        let _ = ShowWindow(hwnd, SW_HIDE);
        if !before.is_invalid() {
            let _ = SetForegroundWindow(before);
        }
        if !text.is_empty() {
            let _ = PostMessageW(Some(notch), WM_ASKED, WPARAM(kind as usize), LPARAM(0));
        }
    }
}

static EDIT_PROC: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

extern "system" fn edit_hook(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let key = VIRTUAL_KEY(wp.0 as u16);
    match msg {
        WM_KEYDOWN if key == VK_RETURN => {
            close(true);
            return LRESULT(0);
        }
        WM_KEYDOWN if key == VK_ESCAPE => {
            close(false);
            return LRESULT(0);
        }
        WM_CHAR if wp.0 == 0x0D || wp.0 == 0x1B => return LRESULT(0),
        _ => {}
    }
    unsafe {
        let old: WNDPROC = std::mem::transmute(EDIT_PROC.load(std::sync::atomic::Ordering::Relaxed));
        CallWindowProcW(old, hwnd, msg, wp, lp)
    }
}

extern "system" fn box_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let dc = BeginPaint(hwnd, &mut ps);
                BOX.with(|b| {
                    if let Some(x) = b.borrow().as_ref() {
                        let dpi = GetDpiForWindow(hwnd);
                        // A soft rounded field behind the edit control, so it reads as a place to type.
                        let (old_brush, old_pen) = (SelectObject(dc, x.field.into()), SelectObject(dc, GetStockObject(NULL_PEN)));
                        let (l, t, r, b) = (scale(9, dpi), scale(29, dpi), scale(W - 9, dpi), scale(67, dpi));
                        let _ = RoundRect(dc, l, t, r, b, scale(14, dpi), scale(14, dpi));
                        SelectObject(dc, old_pen);
                        SelectObject(dc, old_brush);
                        let old = SelectObject(dc, x.small.into());
                        SetBkMode(dc, TRANSPARENT);
                        SetTextColor(dc, COLORREF(0x00A0A0A0));
                        let _ = TextOutW(dc, scale(16, dpi), scale(9, dpi), &x.label);
                        SelectObject(dc, old);
                    }
                });
                let _ = EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            WM_CTLCOLOREDIT => {
                let dc = HDC(wp.0 as *mut _);
                SetTextColor(dc, COLORREF(0x00FFFFFF));
                SetBkColor(dc, FIELD);
                LRESULT(BOX.with(|b| b.borrow().as_ref().map_or(0, |x| x.field.0 as isize)))
            }
            WM_ACTIVATE if (wp.0 & 0xFFFF) as u32 == WA_INACTIVE => {
                // Clicked somewhere else: cancel, and leave focus where the click went.
                BOX.with(|b| {
                    if let Ok(mut b) = b.try_borrow_mut()
                        && let Some(x) = b.as_mut()
                    {
                        x.before = HWND::default();
                    }
                });
                if IsWindowVisible(hwnd).as_bool() {
                    close(false);
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                BOX.with(|b| {
                    if let Some(x) = b.borrow_mut().take() {
                        let _ = DeleteObject(x.font.into());
                        let _ = DeleteObject(x.small.into());
                        let _ = DeleteObject(x.back.into());
                        let _ = DeleteObject(x.field.into());
                    }
                });
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}
