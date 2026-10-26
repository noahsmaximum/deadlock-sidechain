//! Everything that makes the exe stand alone: where its files live, the console when started from
//! a terminal, the bundled DirectML / model / hook addon, one instance at a time, start with Windows.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, anyhow};
use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE};
use windows::core::{HSTRING, PCWSTR};

/// True when stdout goes somewhere a person can see (a terminal or a pipe).
pub static HAS_CONSOLE: AtomicBool = AtomicBool::new(false);

/// %APPDATA%\Sidechain: config + presets.
pub fn config_dir() -> PathBuf {
    std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("Sidechain")
}

/// %LOCALAPPDATA%\Sidechain: unpacked runtime files.
pub fn local_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("Sidechain")
}

/// The exe is built for the Windows subsystem (no console window on double-click). From a
/// terminal, borrow the parent's console so commands still print; redirected output (pipes,
/// files) already works on its own.
pub fn attach_console() {
    use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_OUTPUT_HANDLE};
    let redirected = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) }.is_ok_and(|h| !h.is_invalid() && !h.0.is_null());
    let attached = redirected || unsafe { AttachConsole(ATTACH_PARENT_PROCESS) }.is_ok();
    HAS_CONSOLE.store(attached, Ordering::Relaxed);
}

// ---------------------------------------------------------------------------- DirectML

#[cfg(bundled_dml)]
static DIRECTML: &[u8] = include_bytes!(env!("SIDECHAIN_DML_FILE"));

/// DirectML.dll is delay-loaded: make sure the right copy is found before the neural stems first
/// touch it (Windows ships an older one in System32). A copy next to the exe wins (dev builds).
pub fn prepare_directml() {
    let beside = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join("DirectML.dll")));
    if beside.is_some_and(|p| p.exists()) {
        return;
    }
    #[cfg(bundled_dml)]
    {
        use windows::Win32::System::LibraryLoader::{LoadLibraryW, SetDllDirectoryW};
        // versioned folder: the file must be called DirectML.dll for the loader to match it
        let dir = local_dir().join("runtime").join(format!("directml-{}", DIRECTML.len()));
        let path = dir.join("DirectML.dll");
        if std::fs::metadata(&path).map(|m| m.len()).ok() != Some(DIRECTML.len() as u64) {
            let tmp = dir.join("DirectML.dll.part");
            let ok = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&tmp, DIRECTML)).and_then(|_| std::fs::rename(&tmp, &path));
            if let Err(e) = ok {
                crate::log(format!("couldn't unpack DirectML: {e}"));
                return;
            }
        }
        unsafe {
            let _ = SetDllDirectoryW(&HSTRING::from(dir.as_os_str()));
            let _ = LoadLibraryW(&HSTRING::from(path.as_os_str()));
        }
    }
}

// ---------------------------------------------------------------------------- one instance

const INSTANCE: &str = "Local\\Sidechain.Instance";
const SHOW: &str = "Local\\Sidechain.Show";

/// Owns the instance mutex for the life of the process.
pub struct Instance(#[allow(dead_code)] HANDLE);

/// Ok(Some) = we're the first; Ok(None) = another Sidechain is running (it was asked to show its
/// window, so this one should just exit).
pub fn single_instance() -> Result<Option<Instance>> {
    use windows::Win32::System::Threading::{CreateMutexW, EVENT_MODIFY_STATE, OpenEventW, SetEvent};
    let m = unsafe { CreateMutexW(None, true, &HSTRING::from(INSTANCE)) }?;
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        unsafe {
            if let Ok(e) = OpenEventW(EVENT_MODIFY_STATE, false, &HSTRING::from(SHOW)) {
                let _ = SetEvent(e);
                let _ = CloseHandle(e);
            }
            let _ = CloseHandle(m);
        }
        return Ok(None);
    }
    Ok(Some(Instance(m)))
}

/// Calls `show` whenever a second launch asks this instance to come forward.
pub fn on_show_request(show: impl Fn() + Send + 'static) {
    use windows::Win32::System::Threading::{CreateEventW, INFINITE, WaitForSingleObject};
    let Ok(e) = (unsafe { CreateEventW(None, false, false, &HSTRING::from(SHOW)) }) else { return };
    let raw = e.0 as isize;
    std::thread::Builder::new()
        .name("show-request".into())
        .spawn(move || {
            let e = HANDLE(raw as *mut core::ffi::c_void);
            loop {
                if unsafe { WaitForSingleObject(e, INFINITE) }.0 != 0 {
                    return;
                }
                show();
            }
        })
        .ok();
}

// ---------------------------------------------------------------------------- start with Windows

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_NAME: &str = "Sidechain";

pub fn autostart() -> bool {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW};
    unsafe { RegGetValueW(HKEY_CURRENT_USER, &HSTRING::from(RUN_KEY), &HSTRING::from(RUN_NAME), RRF_RT_REG_SZ, None, None, None) }.is_ok()
}

/// Starts this exe hidden in the tray at sign-in (or stops doing so).
pub fn set_autostart(on: bool) -> Result<()> {
    use windows::Win32::System::Registry::{HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_SZ, RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegSetValueExW};
    let mut key = HKEY::default();
    unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, &HSTRING::from(RUN_KEY), None, KEY_SET_VALUE, &mut key) }.ok()?;
    let r = if on {
        let exe = std::env::current_exe()?;
        let cmd = format!("\"{}\" --hidden", exe.display());
        let wide: Vec<u16> = cmd.encode_utf16().chain(std::iter::once(0)).collect();
        let bytes = unsafe { std::slice::from_raw_parts(wide.as_ptr() as *const u8, wide.len() * 2) };
        unsafe { RegSetValueExW(key, &HSTRING::from(RUN_NAME), None, REG_SZ, Some(bytes)) }.ok()
    } else {
        unsafe { RegDeleteValueW(key, &HSTRING::from(RUN_NAME)) }.ok()
    };
    unsafe {
        let _ = RegCloseKey(key);
    }
    r.map_err(|e| anyhow!("{e}"))
}

// ---------------------------------------------------------------------------- hook addon

#[cfg(bundled_hook)]
pub static HOOK_VPK: Option<&[u8]> = Some(include_bytes!(env!("SIDECHAIN_HOOK_FILE")));
#[cfg(not(bundled_hook))]
pub static HOOK_VPK: Option<&[u8]> = None;

/// <Deadlock>\game: from the running game, else Steam's library folders.
pub fn deadlock_game_dir() -> Option<PathBuf> {
    if let Some(log) = crate::game::find_console_log() {
        return log.parent()?.parent().map(Path::to_path_buf);
    }
    let steam = steam_dir()?;
    let vdf = std::fs::read_to_string(steam.join("steamapps").join("libraryfolders.vdf")).ok()?;
    let mut libs = vec![steam.clone()];
    for line in vdf.lines() {
        let l = line.trim();
        if let Some(rest) = l.strip_prefix("\"path\"") {
            let p = rest.trim().trim_matches('"').replace("\\\\", "\\");
            libs.push(PathBuf::from(p));
        }
    }
    libs.into_iter().map(|l| l.join("steamapps").join("common").join("Deadlock").join("game")).find(|g| g.join("citadel").exists())
}

fn steam_dir() -> Option<PathBuf> {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW};
    let mut buf = vec![0u16; 1024];
    let mut len = (buf.len() * 2) as u32;
    unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            &HSTRING::from(r"Software\Valve\Steam"),
            &HSTRING::from("SteamPath"),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut _),
            Some(&mut len),
        )
    }
    .ok()
    .ok()?;
    let s = String::from_utf16_lossy(&buf[..(len as usize / 2).saturating_sub(1)]);
    Some(PathBuf::from(s.replace('/', "\\")))
}

#[derive(Clone, Debug, PartialEq)]
pub enum HookState {
    /// Deadlock isn't installed where we can find it.
    NoGame,
    /// No installed copy (add it in Mod Manager).
    NotInstalled,
    UpToDate(Vec<PathBuf>),
    Outdated(Vec<PathBuf>),
}

/// Installed copies of our addon: small VPKs under citadel\addons that contain our script.
pub fn hook_state() -> HookState {
    let Some(game) = deadlock_game_dir() else { return HookState::NoGame };
    let mut found = Vec::new();
    let mut stack = vec![game.join("citadel").join("addons")];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let Ok(m) = e.metadata() else { continue };
            if m.is_dir() {
                stack.push(p);
            } else if m.len() < 1 << 20 && p.extension().is_some_and(|x| x.eq_ignore_ascii_case("vpk")) {
                if std::fs::read(&p).is_ok_and(|b| b.windows(14).any(|w| w == b"sidechain_hook")) {
                    found.push(p);
                }
            }
        }
    }
    if found.is_empty() {
        return HookState::NotInstalled;
    }
    let current = HOOK_VPK.is_some_and(|v| found.iter().all(|p| std::fs::read(p).is_ok_and(|b| b == v)));
    if current { HookState::UpToDate(found) } else { HookState::Outdated(found) }
}

/// Overwrite every installed copy with the bundled addon (the game must be closed).
pub fn update_hook(copies: &[PathBuf]) -> Result<()> {
    let v = HOOK_VPK.ok_or_else(|| anyhow!("this build has no hook addon bundled"))?;
    for p in copies {
        std::fs::write(p, v).with_context(|| p.display().to_string())?;
    }
    Ok(())
}

/// Put a copy of the addon in Downloads (for adding it in Mod Manager) and show it in Explorer.
pub fn export_hook() -> Result<PathBuf> {
    let v = HOOK_VPK.ok_or_else(|| anyhow!("this build has no hook addon bundled"))?;
    let dir = std::env::var_os("USERPROFILE").map(|h| PathBuf::from(h).join("Downloads")).filter(|d| d.exists()).unwrap_or_else(config_dir);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("sidechain-hook.vpk");
    std::fs::write(&path, v)?;
    let _ = std::process::Command::new("explorer").arg(format!("/select,{}", path.display())).spawn();
    Ok(path)
}

// ---------------------------------------------------------------------------- errors / session end

/// A startup error when there's no console to print it to.
pub fn error_box(msg: &str) {
    use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
    unsafe {
        MessageBoxW(None, &HSTRING::from(msg), &HSTRING::from("Sidechain"), MB_ICONERROR | MB_OK);
    }
}

/// Without a console there's no Ctrl handler for logoff / shutdown: a hidden top-level window
/// hears WM_ENDSESSION instead, and holds Windows off until the music app has its output back.
pub fn watch_session_end() {
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, MSG, RegisterClassW, TranslateMessage, WINDOW_EX_STYLE, WM_ENDSESSION,
        WM_QUERYENDSESSION, WNDCLASSW, WS_OVERLAPPED,
    };
    unsafe extern "system" fn proc(h: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        match msg {
            WM_QUERYENDSESSION => LRESULT(1),
            WM_ENDSESSION if w.0 != 0 => {
                crate::STOP.store(true, Ordering::SeqCst);
                let t = std::time::Instant::now();
                while !crate::CLEANED.load(Ordering::SeqCst) && t.elapsed() < std::time::Duration::from_secs(4) {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                LRESULT(0)
            }
            _ => unsafe { DefWindowProcW(h, msg, w, l) },
        }
    }
    std::thread::Builder::new()
        .name("session-end".into())
        .spawn(|| unsafe {
            let inst = GetModuleHandleW(None).unwrap_or_default();
            let class = HSTRING::from("SidechainSessionEnd");
            let wc = WNDCLASSW { lpfnWndProc: Some(proc), hInstance: inst.into(), lpszClassName: PCWSTR(class.as_ptr()), ..Default::default() };
            RegisterClassW(&wc);
            // top-level (not message-only: those don't get WM_ENDSESSION), never shown
            if CreateWindowExW(WINDOW_EX_STYLE(0), &class, &class, WS_OVERLAPPED, 0, 0, 0, 0, None, None, Some(inst.into()), None).is_err() {
                return;
            }
            let mut m = MSG::default();
            while GetMessageW(&mut m, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&m);
                DispatchMessageW(&m);
            }
        })
        .ok();
}
