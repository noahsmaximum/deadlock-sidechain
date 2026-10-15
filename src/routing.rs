#![allow(non_snake_case)]
//! Per-app output device override — the thing Windows' "App volume and device preferences" sets.
//!
//! Undocumented WinRT factory `Windows.Media.Internal.AudioPolicyConfig` (same one EarTrumpet uses).
//! Spotify gets pointed at a sink nobody listens to; process loopback still hears it there
//! (loopback is per-process, not per-device), and we play the processed copy on the real output.

use std::ffi::c_void;

use anyhow::{Result, anyhow};
use windows::Win32::Media::Audio::{EDataFlow, ERole, eConsole, eMultimedia, eRender};
use windows::Win32::System::WinRT::RoGetActivationFactory;
use windows::core::{HRESULT, HSTRING, IUnknown, IUnknown_Vtbl, interface};

// Windows 10 21H2+ / Windows 11 layout. Declared on IUnknown with IInspectable's three slots
// spelled out; then 19 slots we never call precede the three we do.
#[interface("ab3d4648-e242-459f-b02f-541c70306324")]
unsafe trait IAudioPolicyConfigFactory: IUnknown {
    fn _get_iids(&self) -> HRESULT;
    fn _get_runtime_class_name(&self) -> HRESULT;
    fn _get_trust_level(&self) -> HRESULT;
    fn _add_ctx_volume_change(&self) -> HRESULT;
    fn _remove_ctx_volume_changed(&self) -> HRESULT;
    fn _add_ringer_vibrate_state_changed(&self) -> HRESULT;
    fn _remove_ringer_vibrate_state_change(&self) -> HRESULT;
    fn _set_volume_group_gain_for_id(&self) -> HRESULT;
    fn _get_volume_group_gain_for_id(&self) -> HRESULT;
    fn _get_active_volume_group_for_endpoint_id(&self) -> HRESULT;
    fn _get_volume_groups_for_endpoint(&self) -> HRESULT;
    fn _get_current_volume_context(&self) -> HRESULT;
    fn _set_volume_group_mute_for_id(&self) -> HRESULT;
    fn _get_volume_group_mute_for_id(&self) -> HRESULT;
    fn _set_ringer_vibrate_state(&self) -> HRESULT;
    fn _get_ringer_vibrate_state(&self) -> HRESULT;
    fn _set_preferred_chat_application(&self) -> HRESULT;
    fn _reset_preferred_chat_application(&self) -> HRESULT;
    fn _get_preferred_chat_application(&self) -> HRESULT;
    fn _get_current_chat_applications(&self) -> HRESULT;
    fn _add_chat_context_changed(&self) -> HRESULT;
    fn _remove_chat_context_changed(&self) -> HRESULT;
    fn SetPersistedDefaultAudioEndpoint(&self, pid: u32, flow: EDataFlow, role: ERole, device: *mut c_void) -> HRESULT;
    fn GetPersistedDefaultAudioEndpoint(&self, pid: u32, flow: EDataFlow, role: ERole, device: *mut *mut c_void) -> HRESULT;
    fn ClearAllPersistedApplicationDefaultEndpoints(&self) -> HRESULT;
}

pub struct PolicyConfig(IAudioPolicyConfigFactory);

impl PolicyConfig {
    /// Needs COM/WinRT initialised on the calling thread.
    pub fn new() -> Result<Self> {
        let f: IAudioPolicyConfigFactory = unsafe { RoGetActivationFactory(&HSTRING::from("Windows.Media.Internal.AudioPolicyConfig")) }
            .map_err(|e| anyhow!("AudioPolicyConfig unavailable on this Windows build: {e}"))?;
        Ok(Self(f))
    }

    /// Point `pid`'s default render device at `mmdevice_id` (`{0.0.0.00000000}.{guid}` as from
    /// IMMDevice::GetId), or back to the system default with `None`.
    pub fn set_render_device(&self, pid: u32, mmdevice_id: Option<&str>) -> Result<()> {
        self.set_render_device_raw(pid, mmdevice_id.map(device_path).as_deref())
    }

    /// Same, taking the raw `\\?\SWD#MMDEVAPI#…` form `render_device` returns.
    pub fn set_render_device_raw(&self, pid: u32, raw_path: Option<&str>) -> Result<()> {
        let h = raw_path.map(HSTRING::from);
        let raw: *mut c_void = h.as_ref().map_or(std::ptr::null_mut(), |s| unsafe { std::mem::transmute_copy(s) });
        for role in [eMultimedia, eConsole] {
            unsafe { self.0.SetPersistedDefaultAudioEndpoint(pid, eRender, role, raw) }.ok()?;
        }
        Ok(())
    }

    /// The override currently set for `pid` (raw path form), `Ok(None)` if it follows the system
    /// default. Errors (E_INVALIDARG) for processes that have no audio session yet.
    pub fn render_device(&self, pid: u32) -> Result<Option<String>> {
        let mut raw: *mut c_void = std::ptr::null_mut();
        unsafe { self.0.GetPersistedDefaultAudioEndpoint(pid, eRender, eMultimedia, &mut raw) }.ok()?;
        if raw.is_null() {
            return Ok(None);
        }
        let h: HSTRING = unsafe { std::mem::transmute(raw) };
        let s = h.to_string_lossy();
        Ok((!s.is_empty()).then_some(s))
    }
}

/// `{0.0.0.00000000}.{guid}` -> `\\?\SWD#MMDEVAPI#{0.0.0.00000000}.{guid}#{render interface}`
pub fn device_path(mmdevice_id: &str) -> String {
    format!(r"\\?\SWD#MMDEVAPI#{mmdevice_id}#{{e6327cad-dcec-4949-ae8a-991e976a79d2}}")
}

/// Inverse of `device_path`.
pub fn mmdevice_id(path: &str) -> Option<String> {
    let start = path.find("MMDEVAPI#")? + "MMDEVAPI#".len();
    let rest = &path[start..];
    Some(rest[..rest.find("#{")?].to_string())
}
