// The shelf: files dropped on the notch, kept as paths (never copied or moved), and dragged back out from there.
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Ole::*;
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;

/// wParam: ENTER, LEAVE or DROP (take the files with `take_dropped`)
pub const WM_SHELF: u32 = WM_APP + 9;
/// wParam: index of the shelf item to drag out
pub const WM_SHELF_OUT: u32 = WM_APP + 10;
pub const ENTER: usize = 1;
pub const LEAVE: usize = 2;
pub const DROP: usize = 3;
const MAX: usize = 24;

thread_local! {
    static DROPPED: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

pub fn take_dropped() -> Vec<String> {
    DROPPED.with(|d| std::mem::take(&mut *d.borrow_mut()))
}

fn files_format() -> FORMATETC {
    FORMATETC { cfFormat: CF_HDROP.0, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: -1, tymed: TYMED_HGLOBAL.0 as u32 }
}

fn read_paths(obj: &IDataObject) -> Vec<String> {
    unsafe {
        let Ok(mut m) = obj.GetData(&files_format()) else { return Vec::new() };
        let hdrop = HDROP(m.u.hGlobal.0);
        let paths = (0..DragQueryFileW(hdrop, u32::MAX, None))
            .map(|i| {
                let mut buf = vec![0u16; DragQueryFileW(hdrop, i, None) as usize + 1];
                let n = DragQueryFileW(hdrop, i, Some(&mut buf)) as usize;
                String::from_utf16_lossy(&buf[..n])
            })
            .collect();
        ReleaseStgMedium(&mut m);
        paths
    }
}

/// OLE calls these on the window's thread while something is dragged over the notch.
#[implement(IDropTarget)]
struct Target {
    hwnd: isize,
    files: Cell<bool>,
}

impl Target_Impl {
    fn post(&self, what: usize) {
        unsafe {
            let _ = PostMessageW(Some(HWND(self.hwnd as *mut _)), WM_SHELF, WPARAM(what), LPARAM(0));
        }
    }

    /// "Link": the shelf remembers where the file is; it never takes it.
    fn effect(&self, out: *mut DROPEFFECT) {
        if let Some(e) = unsafe { out.as_mut() } {
            *e = if self.files.get() { DROPEFFECT_LINK } else { DROPEFFECT_NONE };
        }
    }
}

impl IDropTarget_Impl for Target_Impl {
    fn DragEnter(&self, obj: Ref<IDataObject>, _: MODIFIERKEYS_FLAGS, _: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        self.files.set(obj.as_ref().is_some_and(|o| unsafe { o.QueryGetData(&files_format()) } == S_OK));
        self.effect(effect);
        if self.files.get() {
            self.post(ENTER);
        }
        Ok(())
    }

    fn DragOver(&self, _: MODIFIERKEYS_FLAGS, _: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        self.effect(effect);
        Ok(())
    }

    fn DragLeave(&self) -> Result<()> {
        if self.files.get() {
            self.post(LEAVE);
        }
        Ok(())
    }

    fn Drop(&self, obj: Ref<IDataObject>, _: MODIFIERKEYS_FLAGS, _: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        let paths = obj.as_ref().map(read_paths).unwrap_or_default();
        self.files.set(!paths.is_empty());
        self.effect(effect);
        DROPPED.with(|d| d.borrow_mut().extend(paths));
        self.post(DROP);
        Ok(())
    }
}

/// Makes the notch a drop target. Needs OleInitialize on this thread first.
pub fn accept_drops(hwnd: HWND) -> Result<()> {
    let target: IDropTarget = Target { hwnd: hwnd.0 as isize, files: Cell::new(false) }.into();
    unsafe { RegisterDragDrop(hwnd, &target) }
}

/// Drags a shelf file into another app: copy or link, never move, so the original stays where it is.
pub fn drag_out(hwnd: HWND, path: &str) -> Result<()> {
    unsafe {
        let item: IShellItem = SHCreateItemFromParsingName(&HSTRING::from(path), None)?;
        let data: IDataObject = item.BindToHandler(None, &BHID_DataObject)?;
        SHDoDragDrop(Some(hwnd), &data, None, DROPEFFECT_COPY | DROPEFFECT_LINK)?;
        Ok(())
    }
}

/// Newest first, no duplicates, at most MAX.
pub fn add(shelf: &mut Vec<String>, new: Vec<String>) {
    for p in new {
        shelf.retain(|x| !x.eq_ignore_ascii_case(&p));
        shelf.insert(0, p);
    }
    shelf.truncate(MAX);
}

fn file() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("APPDATA")?).join("notch").join("shelf.json"))
}

/// The saved shelf, minus files that have since moved or been deleted.
pub fn load() -> Vec<String> {
    let list: Vec<String> = file().and_then(|f| std::fs::read_to_string(f).ok()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    list.into_iter().filter(|p| Path::new(p).exists()).collect()
}

pub fn save(shelf: &[String]) {
    let Some(f) = file() else { return };
    let _ = std::fs::create_dir_all(f.parent().unwrap());
    let tmp = f.with_extension("json.tmp");
    if let Ok(json) = serde_json::to_string_pretty(shelf)
        && std::fs::write(&tmp, json).is_ok()
    {
        let _ = std::fs::rename(tmp, f);
    }
}

pub fn name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

/// BGRA, premultiplied
pub struct Thumb {
    pub w: u32,
    pub h: u32,
    pub pixels: Vec<u8>,
}

/// Explorer's own picture for a file: a real thumbnail for photos and videos, the file-type icon otherwise.
pub fn thumbnail(path: &str, px: u32) -> Option<Thumb> {
    unsafe {
        let f: IShellItemImageFactory = SHCreateItemFromParsingName(&HSTRING::from(path), None).ok()?;
        let bmp = f.GetImage(SIZE { cx: px as i32, cy: px as i32 }, SIIGBF_RESIZETOFIT | SIIGBF_BIGGERSIZEOK).ok()?;
        let mut bm = BITMAP::default();
        GetObjectW(bmp.into(), size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as *mut _));
        let (w, h) = (bm.bmWidth.max(1), bm.bmHeight.abs().max(1));
        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut pixels = vec![0u8; (w * h * 4) as usize];
        let dc = GetDC(None);
        let rows = GetDIBits(dc, bmp, 0, h as u32, Some(pixels.as_mut_ptr().cast()), &mut bmi, DIB_RGB_COLORS);
        ReleaseDC(None, dc);
        let _ = DeleteObject(bmp.into());
        if rows == 0 {
            return None;
        }
        premultiplied(&mut pixels);
        Some(Thumb { w: w as u32, h: h as u32, pixels })
    }
}

/// Shell images come either without alpha (all zero), with straight alpha, or already premultiplied.
/// Direct2D wants premultiplied; a colour brighter than its alpha gives straight alpha away.
pub fn premultiplied(bgra: &mut [u8]) {
    let (px, _) = bgra.as_chunks_mut::<4>();
    if px.iter().all(|p| p[3] == 0) {
        px.iter_mut().for_each(|p| p[3] = 255);
    } else if px.iter().any(|p| p[0].max(p[1]).max(p[2]) > p[3]) {
        for p in px {
            let a = p[3] as u32;
            for c in &mut p[..3] {
                *c = (*c as u32 * a / 255) as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newest_first_without_duplicates() {
        let mut s = vec!["C:\\a.txt".to_string(), "C:\\b.txt".to_string()];
        add(&mut s, vec!["C:\\c.txt".into(), "c:\\A.TXT".into()]);
        assert_eq!(s, ["c:\\A.TXT", "C:\\c.txt", "C:\\b.txt"]);
        add(&mut s, (0..30).map(|i| format!("C:\\{i}")).collect());
        assert_eq!(s.len(), MAX);
        assert_eq!(s[0], "C:\\29");
    }

    /// Talks to the Windows shell: `cargo test -- --ignored`
    #[test]
    #[ignore]
    fn shell_live() {
        unsafe {
            let _ = OleInitialize(None);
            let path = "C:\\Windows\\notepad.exe";
            let t = thumbnail(path, 80).expect("the shell has a picture for notepad");
            assert!(t.w > 0 && t.h > 0 && t.pixels.len() == (t.w * t.h * 4) as usize);
            assert!(t.pixels.as_chunks::<4>().0.iter().any(|p| p[3] > 0), "not blank");
            // What a drag out would carry: a file drop list.
            let item: IShellItem = SHCreateItemFromParsingName(&HSTRING::from(path), None).unwrap();
            let data: IDataObject = item.BindToHandler(None, &BHID_DataObject).unwrap();
            assert_eq!(data.QueryGetData(&files_format()), S_OK);
            assert_eq!(read_paths(&data), [path]);
        }
    }

    #[test]
    fn names() {
        assert_eq!(name("C:\\Users\\me\\beach.jpg"), "beach.jpg");
        assert_eq!(name("beach.jpg"), "beach.jpg");
    }

    #[test]
    fn alpha_forms() {
        let mut none = vec![10, 20, 30, 0, 40, 50, 60, 0];
        premultiplied(&mut none);
        assert_eq!(none, [10, 20, 30, 255, 40, 50, 60, 255], "no alpha means opaque");
        let mut straight = vec![200, 100, 50, 128];
        premultiplied(&mut straight);
        assert_eq!(straight, [100, 50, 25, 128]);
        let mut already = vec![100, 50, 25, 128];
        premultiplied(&mut already);
        assert_eq!(already, [100, 50, 25, 128], "left alone");
    }
}
