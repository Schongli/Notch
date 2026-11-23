// System pop-ups: volume, output device, charger/battery, Caps Lock. Every source is an OS callback that posts
// a message to the window; nothing polls.
use std::sync::atomic::{AtomicIsize, Ordering};

use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::*;
use windows::Win32::Media::Audio::Endpoints::*;
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Power::POWERBROADCAST_SETTING;
use windows::Win32::System::SystemServices::{GUID_ACDC_POWER_SOURCE, GUID_BATTERY_PERCENTAGE_REMAINING};
use windows::Win32::UI::Input::KeyboardAndMouse::VK_CAPITAL;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;

/// wParam: volume in per mille, plus 1 << 16 when muted.
pub const WM_VOLUME: u32 = WM_APP + 5;
pub const WM_AUDIO_DEVICE: u32 = WM_APP + 6;
pub const WM_CAPS: u32 = WM_APP + 7;

fn post(hwnd: isize, msg: u32, wp: usize) {
    unsafe {
        let _ = PostMessageW(Some(HWND(hwnd as *mut _)), msg, WPARAM(wp), LPARAM(0));
    }
}

#[implement(IAudioEndpointVolumeCallback)]
struct VolumeWatch(isize);

impl IAudioEndpointVolumeCallback_Impl for VolumeWatch_Impl {
    fn OnNotify(&self, data: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> Result<()> {
        if let Some(d) = unsafe { data.as_ref() } {
            post(self.0, WM_VOLUME, (d.fMasterVolume * 1000.0).round() as usize | (d.bMuted.as_bool() as usize) << 16);
        }
        Ok(())
    }
}

#[implement(IMMNotificationClient)]
struct DeviceWatch(isize);

impl IMMNotificationClient_Impl for DeviceWatch_Impl {
    fn OnDefaultDeviceChanged(&self, flow: EDataFlow, role: ERole, _id: &PCWSTR) -> Result<()> {
        // Fires once per role; the console role is the one the volume flyout follows.
        if flow == eRender && role == eConsole {
            post(self.0, WM_AUDIO_DEVICE, 0);
        }
        Ok(())
    }
    fn OnDeviceStateChanged(&self, _: &PCWSTR, _: DEVICE_STATE) -> Result<()> {
        Ok(())
    }
    fn OnDeviceAdded(&self, _: &PCWSTR) -> Result<()> {
        Ok(())
    }
    fn OnDeviceRemoved(&self, _: &PCWSTR) -> Result<()> {
        Ok(())
    }
    fn OnPropertyValueChanged(&self, _: &PCWSTR, _: &PROPERTYKEY) -> Result<()> {
        Ok(())
    }
}

/// The default speakers/headphones and their volume. Follows the default device when it changes.
pub struct Audio {
    devices: IMMDeviceEnumerator,
    client: IMMNotificationClient,
    watch: IAudioEndpointVolumeCallback,
    volume: Option<IAudioEndpointVolume>,
}

impl Audio {
    pub fn start(hwnd: HWND) -> Result<Self> {
        unsafe {
            let devices: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            let client: IMMNotificationClient = DeviceWatch(hwnd.0 as isize).into();
            devices.RegisterEndpointNotificationCallback(&client)?;
            let mut a = Self { devices, client, watch: VolumeWatch(hwnd.0 as isize).into(), volume: None };
            a.follow_default();
            Ok(a)
        }
    }

    /// Re-attaches to the current default output; returns its name ("Headphones (AirPods Pro)").
    pub fn follow_default(&mut self) -> Option<String> {
        unsafe {
            if let Some(v) = self.volume.take() {
                let _ = v.UnregisterControlChangeNotify(&self.watch);
            }
            let dev = self.devices.GetDefaultAudioEndpoint(eRender, eConsole).ok()?;
            let vol: IAudioEndpointVolume = dev.Activate(CLSCTX_ALL, None).ok()?;
            vol.RegisterControlChangeNotify(&self.watch).ok()?;
            self.volume = Some(vol);
            let name = dev.OpenPropertyStore(STGM_READ).and_then(|p| p.GetValue(&PKEY_Device_FriendlyName)).ok()?;
            Some(name.to_string())
        }
    }

    /// Volume 0..1 and mute.
    pub fn level(&self) -> Option<(f32, bool)> {
        let v = self.volume.as_ref()?;
        unsafe { Some((v.GetMasterVolumeLevelScalar().ok()?, v.GetMute().ok()?.as_bool())) }
    }

    /// Nudges the volume; turning it up also unmutes.
    pub fn nudge(&self, delta: f32) {
        let (Some(v), Some((level, muted))) = (&self.volume, self.level()) else { return };
        unsafe {
            let _ = v.SetMasterVolumeLevelScalar((level + delta).clamp(0.0, 1.0), std::ptr::null());
            if muted && delta > 0.0 {
                let _ = v.SetMute(false, std::ptr::null());
            }
        }
    }
}

impl Drop for Audio {
    fn drop(&mut self) {
        unsafe {
            if let Some(v) = &self.volume {
                let _ = v.UnregisterControlChangeNotify(&self.watch);
            }
            let _ = self.devices.UnregisterEndpointNotificationCallback(&self.client);
        }
    }
}

pub enum Power {
    /// true when plugged in
    Plugged(bool),
    Percent(u32),
}

/// Reads a PBT_POWERSETTINGCHANGE broadcast (the window registered for these two settings in main).
pub fn power_setting(lp: LPARAM) -> Option<Power> {
    let s = unsafe { (lp.0 as *const POWERBROADCAST_SETTING).as_ref()? };
    if s.DataLength < 4 {
        return None;
    }
    let value = u32::from_ne_bytes(unsafe { std::ptr::read_unaligned(s.Data.as_ptr() as *const [u8; 4]) });
    match s.PowerSetting {
        GUID_ACDC_POWER_SOURCE => Some(Power::Plugged(value == 0)), // 0 = AC, 1 = battery, 2 = UPS
        GUID_BATTERY_PERCENTAGE_REMAINING => Some(Power::Percent(value)),
        _ => None,
    }
}

/// Battery glyph from Segoe Fluent Icons / MDL2: Battery0..10 and BatteryCharging0..10.
pub fn battery_glyph(percent: u32, charging: bool) -> String {
    let base = if charging { 0xEBAB } else { 0xEBA0 };
    char::from_u32(base + (percent.min(100) + 5) / 10).map(String::from).unwrap_or_default()
}

/// Volume glyph by level: Mute, Volume0..3.
pub fn volume_glyph(level: f32, muted: bool) -> &'static str {
    match (muted, level) {
        (true, _) | (_, 0.0) => "\u{E74F}",
        (_, l) if l < 0.34 => "\u{E993}",
        (_, l) if l < 0.67 => "\u{E994}",
        _ => "\u{E995}",
    }
}

/// True when the battery just dropped to or below a warning step (20, 10, 5 %).
pub fn crossed_low(old: u32, new: u32) -> bool {
    [20, 10, 5].iter().any(|&t| old > t && new <= t)
}

static CAPS_HWND: AtomicIsize = AtomicIsize::new(0);

unsafe extern "system" fn keyboard(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // Only Caps Lock is looked at; every other key passes straight through, unread.
    if code == HC_ACTION as i32 && wp.0 as u32 == WM_KEYUP {
        let k = unsafe { &*(lp.0 as *const KBDLLHOOKSTRUCT) };
        if k.vkCode == VK_CAPITAL.0 as u32 {
            post(CAPS_HWND.load(Ordering::Relaxed), WM_CAPS, 0);
        }
    }
    unsafe { CallNextHookEx(None, code, wp, lp) }
}

/// A low-level keyboard hook is the only way to hear Caps Lock while another app has focus.
pub fn watch_caps_lock(hwnd: HWND, instance: HINSTANCE) {
    CAPS_HWND.store(hwnd.0 as isize, Ordering::Relaxed);
    unsafe {
        let _ = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard), Some(instance), 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn low_battery_steps() {
        assert!(crossed_low(21, 20));
        assert!(crossed_low(12, 9));
        assert!(!crossed_low(20, 19), "already warned at 20");
        assert!(!crossed_low(19, 30), "charging up");
        assert!(crossed_low(30, 4), "a big drop warns once");
    }

    #[test]
    fn glyphs() {
        assert_eq!(battery_glyph(0, false), "\u{EBA0}");
        assert_eq!(battery_glyph(84, false), "\u{EBA8}");
        assert_eq!(battery_glyph(100, true), "\u{EBB5}");
        assert_eq!(battery_glyph(250, true), "\u{EBB5}", "bad readings stay in range");
        assert_eq!(volume_glyph(0.5, true), "\u{E74F}");
        assert_eq!(volume_glyph(0.0, false), "\u{E74F}");
        assert_eq!(volume_glyph(0.9, false), "\u{E995}");
    }
}
