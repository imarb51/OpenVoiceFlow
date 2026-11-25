use super::types::*;
use crate::injection::{ClipboardPasteInjector, TextInjector};
use enigo::{Axis, Button, Coordinate, Direction, Enigo, Key, Keyboard, Mouse, Settings};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use windows::{
    core::{Interface, BOOL, PWSTR},
    Win32::{
        Foundation::*,
        Graphics::Gdi::*,
        System::{Com::*, Threading::*},
        UI::{Accessibility::*, HiDpi::*, WindowsAndMessaging::*},
    },
};

fn hwnd(w: &WindowTarget) -> HWND {
    HWND(w.handle as *mut _)
}
pub fn foreground() -> Option<WindowTarget> {
    unsafe { target(GetForegroundWindow()).ok() }
}
unsafe fn target(h: HWND) -> Result<WindowTarget, String> {
    if !IsWindow(Some(h)).as_bool() {
        return Err("Target window no longer exists".into());
    }
    let mut pid = 0;
    GetWindowThreadProcessId(h, Some(&mut pid));
    let mut title = [0u16; 1024];
    let n = GetWindowTextW(h, &mut title);
    Ok(WindowTarget {
        handle: h.0 as i64,
        pid,
        title: String::from_utf16_lossy(&title[..n as usize]),
    })
}
fn valid(w: &WindowTarget) -> Result<(), String> {
    if unsafe { target(hwnd(w))? }.pid != w.pid {
        return Err("Window identity changed; observe again".into());
    }
    Ok(())
}
fn bounds(w: &WindowTarget) -> Result<Bounds, String> {
    valid(w)?;
    let mut r = RECT::default();
    unsafe { GetWindowRect(hwnd(w), &mut r) }.map_err(|e| e.to_string())?;
    Ok(rect(r))
}
fn rect(r: RECT) -> Bounds {
    Bounds {
        x: r.left,
        y: r.top,
        width: r.right - r.left,
        height: r.bottom - r.top,
    }
}
unsafe extern "system" fn collect_windows(h: HWND, data: LPARAM) -> BOOL {
    if IsWindowVisible(h).as_bool() {
        if let Ok(w) = target(h) {
            if !w.title.is_empty() {
                (*(data.0 as *mut Vec<WindowTarget>)).push(w);
            }
        }
    }
    BOOL(1)
}
pub fn windows() -> Vec<WindowTarget> {
    let mut v = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(collect_windows), LPARAM(&mut v as *mut _ as isize));
    }
    v
}
pub(crate) fn process_path(pid: u32) -> String {
    unsafe {
        let Ok(p) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return String::new();
        };
        let mut b = [0u16; 32768];
        let mut n = b.len() as u32;
        let ok = QueryFullProcessImageNameW(p, PROCESS_NAME_WIN32, PWSTR(b.as_mut_ptr()), &mut n)
            .is_ok();
        let _ = CloseHandle(p);
        if ok {
            String::from_utf16_lossy(&b[..n as usize])
        } else {
            String::new()
        }
    }
}

pub fn discover(custom: &[Application]) -> Vec<Application> {
    use std::os::windows::process::CommandExt;
    // Fixed inventory script only. Model arguments never become shell code.
    let script = r#"[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false); $ErrorActionPreference='SilentlyContinue'; $shell=New-Object -ComObject WScript.Shell; $items=@(); foreach($root in @([Environment]::GetFolderPath('StartMenu'),[Environment]::GetFolderPath('CommonStartMenu'))) { Get-ChildItem -LiteralPath $root -Filter *.lnk -Recurse | ForEach-Object { $l=$shell.CreateShortcut($_.FullName); if($l.TargetPath -like '*.exe' -and -not $l.Arguments) { $items+=@{id=$_.FullName;name=$_.BaseName;executable=$l.TargetPath;arguments=@()} } } }; foreach($root in @('HKCU:\Software\Microsoft\Windows\CurrentVersion\App Paths','HKLM:\Software\Microsoft\Windows\CurrentVersion\App Paths')) { Get-ChildItem $root | ForEach-Object { $p=$_.GetValue(''); if($p) {$items+=@{id=$_.Name;name=$_.PSChildName;executable=$p.Trim('"');arguments=@()}} } }; Get-StartApps | ForEach-Object {if($_.AppID -match '!') {$items+=@{id=$_.AppID;name=$_.Name;executable='explorer.exe';arguments=@(('shell:AppsFolder\'+$_.AppID))}}}; ConvertTo-Json -InputObject @($items) -Compress"#;
    let output = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .creation_flags(0x08000000)
        .output();
    let mut apps: Vec<Application> = output
        .ok()
        .and_then(|o| serde_json::from_slice(&o.stdout).ok())
        .unwrap_or_default();
    let mut seen = std::collections::HashSet::new();
    apps.retain(|a| seen.insert((a.executable.to_lowercase(), a.arguments.clone())));
    for (i, a) in apps.iter_mut().enumerate() {
        a.id = format!("installed-{i}");
    }
    apps.extend_from_slice(custom);
    apps.sort_by(|a, b| a.name.cmp(&b.name));
    apps.dedup_by(|a, b| a.id == b.id);
    apps
}

pub struct NativeDesktop {
    automation: IUIAutomation,
    controls: HashMap<String, IUIAutomationElement>,
    apps: Vec<Application>,
    sequence: u64,
    input: Enigo,
    cancelled: Arc<AtomicBool>,
    observed_at: Instant,
    _com: ComApartment,
}
struct ComApartment;
impl Drop for ComApartment {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}
impl NativeDesktop {
    pub fn new(apps: Vec<Application>, cancelled: Arc<AtomicBool>) -> Result<Self, String> {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED)
                .ok()
                .map_err(|e| e.to_string())?;
            let apartment = ComApartment;
            let _ = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            let automation: IUIAutomation =
                CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)
                    .map_err(|e| e.to_string())?;
            if let Ok(v2) = automation.cast::<IUIAutomation2>() {
                let _ = v2.SetConnectionTimeout(2000);
                let _ = v2.SetTransactionTimeout(3000);
            }
            Ok(Self {
                automation,
                controls: HashMap::new(),
                apps,
                sequence: 0,
                input: Enigo::new(&Settings::default()).map_err(|e| e.to_string())?,
                cancelled,
                observed_at: Instant::now(),
                _com: apartment,
            })
        }
    }
    fn focus(&self, w: &WindowTarget) -> Result<(), String> {
        if self.cancelled.load(Ordering::SeqCst) {
            return Err("Cancelled".into());
        }
        valid(w)?;
        unsafe {
            if IsIconic(hwnd(w)).as_bool() {
                let _ = ShowWindow(hwnd(w), SW_RESTORE);
            }
            let _ = SetForegroundWindow(hwnd(w));
            if GetForegroundWindow() != hwnd(w) {
                let current = GetCurrentThreadId();
                let foreground_thread = GetWindowThreadProcessId(GetForegroundWindow(), None);
                let attached = foreground_thread != 0
                    && foreground_thread != current
                    && AttachThreadInput(current, foreground_thread, true).as_bool();
                let _ = BringWindowToTop(hwnd(w));
                let _ = SetForegroundWindow(hwnd(w));
                if attached {
                    let _ = AttachThreadInput(current, foreground_thread, false);
                }
            }
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if self.cancelled.load(Ordering::SeqCst) {
                return Err("Cancelled".into());
            }
            if foreground().is_some_and(|f| f.handle == w.handle && f.pid == w.pid) {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(40));
        }
        Err("Windows refused foreground focus; select the application and resume".into())
    }
    fn check(&self, id: u64, o: &Observation) -> Result<(), String> {
        if self.cancelled.load(Ordering::SeqCst) {
            return Err("Cancelled".into());
        }
        if id != o.id || id != self.sequence || self.observed_at.elapsed() > Duration::from_secs(60)
        {
            return Err("Stale observation; observe again".into());
        }
        if bounds(&o.window)? != o.bounds {
            return Err("Window moved or resized; observe again".into());
        }
        self.focus(&o.window)
    }
    fn control(&self, id: &str, o: &Observation) -> Result<&IUIAutomationElement, String> {
        let c = self
            .controls
            .get(id)
            .ok_or("Control is not in current observation")?;
        let old = o
            .controls
            .iter()
            .find(|c| c.id == id)
            .ok_or("Unknown control")?;
        if unsafe { rect(c.CurrentBoundingRectangle().map_err(|e| e.to_string())?) } != old.bounds {
            return Err("Control moved; observe again".into());
        }
        if unsafe { c.CurrentIsPassword().unwrap_or(BOOL(1)).as_bool() } {
            return Err("Password fields require manual input".into());
        }
        Ok(c)
    }
}
impl Desktop for NativeDesktop {
    fn catalog(&self) -> Vec<Application> {
        self.apps.clone()
    }
    fn observe(&mut self, w: &WindowTarget, screenshot: bool) -> Result<Observation, String> {
        self.focus(w)?;
        self.sequence += 1;
        self.controls.clear();
        let b = bounds(w)?;
        if w.pid == std::process::id() {
            return Ok(Observation{id:self.sequence,window:w.clone(),bounds:b,scale:1.0,controls:vec![],accessibility_error:Some("OpenVoiceFlow's own settings are excluded to protect stored credentials. Open or focus the requested external application.".into()),screenshot:None});
        }
        let mut controls = Vec::new();
        let mut error = None;
        unsafe {
            let result = (|| -> windows::core::Result<()> {
                let root = self.automation.ElementFromHandle(hwnd(w))?;
                let elements =
                    root.FindAll(TreeScope_Subtree, &self.automation.CreateTrueCondition()?)?;
                for i in 0..elements.Length()?.min(1500) {
                    let c = elements.GetElement(i)?;
                    if c.CurrentIsPassword().unwrap_or(BOOL(1)).as_bool()
                        || c.CurrentIsOffscreen().unwrap_or(BOOL(1)).as_bool()
                    {
                        continue;
                    }
                    let r = rect(c.CurrentBoundingRectangle().unwrap_or_default());
                    if r.width <= 0 || r.height <= 0 {
                        continue;
                    }
                    let id = format!("{}:{}", self.sequence, i);
                    let value = c
                        .GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
                        .ok();
                    let name = c.CurrentName().unwrap_or_default().to_string();
                    let text = value
                        .as_ref()
                        .and_then(|p| p.CurrentValue().ok())
                        .map(|s| s.to_string())
                        .or_else(|| {
                            c.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId)
                                .ok()
                                .and_then(|p| p.DocumentRange().ok())
                                .and_then(|r| r.GetText(4000).ok())
                                .map(|s| s.to_string())
                        });
                    let role = c.CurrentControlType().unwrap_or_default().0;
                    controls.push(Control {
                        id: id.clone(),
                        name: name.chars().take(400).collect(),
                        role: format!("{role}"),
                        bounds: r,
                        editable: role == 50004
                            || role == 50030
                            || value
                                .as_ref()
                                .is_some_and(|v| v.CurrentIsReadOnly().is_ok_and(|b| !b.as_bool())),
                        value: text.map(|s| s.chars().take(4000).collect()),
                    });
                    self.controls.insert(id, c);
                }
                Ok(())
            })();
            if let Err(e) = result {
                error = Some(e.to_string());
            }
        }
        self.observed_at = Instant::now();
        Ok(Observation {
            id: self.sequence,
            window: unsafe { target(hwnd(w))? },
            bounds: b.clone(),
            scale: unsafe { GetDpiForWindow(hwnd(w)) as f64 / 96.0 },
            controls,
            accessibility_error: error,
            screenshot: if screenshot { Some(capture(&b)?) } else { None },
        })
    }
    fn execute(&mut self, a: &Action, o: &Observation) -> Result<WindowTarget, String> {
        if self.cancelled.load(Ordering::SeqCst) {
            return Err("Cancelled".into());
        }
        let map = |e: enigo::InputError| e.to_string();
        match a {
            Action::OpenApp { app_id } => {
                let app = self
                    .apps
                    .iter()
                    .find(|a| &a.id == app_id)
                    .ok_or("Application ID not in catalog")?
                    .clone();
                let matches = |w: &WindowTarget| {
                    let p = process_path(w.pid);
                    application_matches_process(&app, &p)
                };
                if let Some(w) = windows().into_iter().find(&matches) {
                    self.focus(&w)?;
                    return Ok(w);
                }
                std::process::Command::new(&app.executable)
                    .args(&app.arguments)
                    .spawn()
                    .map_err(|e| e.to_string())?;
                let end = Instant::now() + Duration::from_secs(15);
                while Instant::now() < end {
                    if self.cancelled.load(Ordering::SeqCst) {
                        return Err("Cancelled".into());
                    }
                    if let Some(w) = windows().into_iter().find(&matches) {
                        self.focus(&w)?;
                        return Ok(w);
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                return Err(
                    "Application launched but no matching window appeared; observe or ask user"
                        .into(),
                );
            }
            Action::FocusWindow { window } => {
                self.focus(window)?;
                return Ok(window.clone());
            }
            Action::FindApps { query } => {
                if !query.is_empty()
                    && query
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || ['.', '-', '_'].contains(&c))
                {
                    let name = if query.to_lowercase().ends_with(".exe") {
                        query.clone()
                    } else {
                        format!("{query}.exe")
                    };
                    let mut directories =
                        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
                            .collect::<Vec<_>>();
                    if let Some(root) = std::env::var_os("WINDIR") {
                        directories.push(std::path::PathBuf::from(root).join("System32"));
                    }
                    if let Some(path) = directories
                        .into_iter()
                        .map(|d| d.join(&name))
                        .find(|p| p.is_file())
                    {
                        let executable = path.to_string_lossy().to_string();
                        if !self
                            .apps
                            .iter()
                            .any(|a| a.executable.eq_ignore_ascii_case(&executable))
                        {
                            self.apps.push(Application {
                                id: format!("path-{}", self.apps.len()),
                                name: query.clone(),
                                executable,
                                arguments: vec![],
                            });
                        }
                    }
                }
            }
            Action::Observe { .. } | Action::FindControls { .. } => {}
            Action::TypeText {
                observation_id,
                control_id,
                text,
            } => {
                self.check(*observation_id, o)?;
                if text.len() > 32768 {
                    return Err("Text exceeds 32 KiB limit".into());
                }
                if let Some(id) = control_id {
                    if !o.controls.iter().any(|c| c.id == *id && c.editable) {
                        return Err("Target is not an editable field".into());
                    }
                    unsafe {
                        self.control(id, o)?.SetFocus().map_err(|e| e.to_string())?;
                    }
                }
                let focused = unsafe {
                    self.automation
                        .GetFocusedElement()
                        .map_err(|e| e.to_string())?
                };
                if unsafe { focused.CurrentIsPassword().unwrap_or(BOOL(1)).as_bool() } {
                    return Err("Password fields require manual input".into());
                }
                if unsafe { focused.CurrentProcessId().unwrap_or_default() } as u32 != o.window.pid
                {
                    return Err("Focused field belongs to another application".into());
                }
                ClipboardPasteInjector
                    .insert_text_checked(text, &|| {
                        use crate::injection::InjectionError;
                        if self.cancelled.load(Ordering::SeqCst)
                            || !foreground().is_some_and(|w| {
                                w.handle == o.window.handle && w.pid == o.window.pid
                            })
                        {
                            return Err(InjectionError::Input(
                                "Focus changed or task cancelled before paste".into(),
                            ));
                        }
                        let same = unsafe {
                            self.automation.GetFocusedElement().and_then(|current| {
                                self.automation.CompareElements(&focused, &current)
                            })
                        }
                        .is_ok_and(|v| v.as_bool());
                        if !same {
                            return Err(InjectionError::Input(
                                "Focused field changed before paste".into(),
                            ));
                        }
                        Ok(())
                    })
                    .map_err(|e| e.to_string())?;
                if let Ok(p) = unsafe {
                    focused.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
                } {
                    let end = Instant::now() + Duration::from_secs(2);
                    let mut matched = false;
                    while Instant::now() < end {
                        if unsafe { p.CurrentValue() }.is_ok_and(|v| v.to_string().contains(text)) {
                            matched = true;
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    if !matched {
                        return Err("Text input result uncertain; inspect before retrying to avoid duplication".into());
                    }
                }
            }
            Action::Key {
                observation_id,
                keys,
            } => {
                self.check(*observation_id, o)?;
                if unsafe {
                    self.automation
                        .GetFocusedElement()
                        .and_then(|e| e.CurrentIsPassword())
                        .is_ok_and(|v| v.as_bool())
                } && !keys
                    .iter()
                    .all(|k| ["tab", "escape"].contains(&k.to_lowercase().as_str()))
                {
                    return Err("Password fields require manual input".into());
                }
                let parsed: Result<Vec<Key>, String> = keys.iter().map(|s| key(s)).collect();
                let parsed = parsed?;
                if parsed.is_empty() || parsed.len() > 5 {
                    return Err("Use one key or a shortcut with up to five keys".into());
                }
                let result: Result<(), String> = (|| {
                    for k in &parsed {
                        self.input.key(*k, Direction::Press).map_err(map)?;
                    }
                    Ok(())
                })();
                for k in parsed.iter().rev() {
                    let _ = self.input.key(*k, Direction::Release);
                }
                result?;
            }
            Action::Pointer {
                observation_id,
                control_id,
                x,
                y,
                operation,
                amount,
            } => {
                self.check(*observation_id, o)?;
                let (px, py) = if let Some(id) = control_id {
                    let r = unsafe {
                        rect(
                            self.control(id, o)?
                                .CurrentBoundingRectangle()
                                .map_err(|e| e.to_string())?,
                        )
                    };
                    (r.x + r.width / 2, r.y + r.height / 2)
                } else {
                    if o.screenshot.is_none() {
                        return Err("Coordinates require a screenshot observation".into());
                    }
                    let (x, y) = (x.ok_or("Missing x")?, y.ok_or("Missing y")?);
                    screen_point(&o.bounds, x, y)?
                };
                if !["move", "click", "double_click", "right_click", "scroll"]
                    .contains(&operation.as_str())
                {
                    return Err("Unsupported pointer operation".into());
                }
                self.input
                    .move_mouse(px, py, Coordinate::Abs)
                    .map_err(map)?;
                match operation.as_str() {
                    "move" => {}
                    "scroll" => self
                        .input
                        .scroll(amount.unwrap_or(3).clamp(-20, 20), Axis::Vertical)
                        .map_err(map)?,
                    "right_click" => self
                        .input
                        .button(Button::Right, Direction::Click)
                        .map_err(map)?,
                    _ => {
                        self.input
                            .button(Button::Left, Direction::Click)
                            .map_err(map)?;
                        if operation == "double_click" {
                            self.input
                                .button(Button::Left, Direction::Click)
                                .map_err(map)?;
                        }
                    }
                }
            }
        }
        Ok(o.window.clone())
    }
    fn release_keys(&mut self) {
        for k in [Key::Control, Key::Shift, Key::Alt, Key::Meta] {
            let _ = self.input.key(k, Direction::Release);
        }
    }
}
pub(crate) fn screen_point(bounds: &Bounds, x: i32, y: i32) -> Result<(i32, i32), String> {
    if x < 0 || y < 0 || x >= bounds.width || y >= bounds.height {
        return Err("Coordinates outside observed window".into());
    }
    Ok((bounds.x + x, bounds.y + y))
}
pub(crate) fn application_matches_process(app: &Application, process: &str) -> bool {
    if process.is_empty() {
        return false;
    }
    if app.executable.eq_ignore_ascii_case("explorer.exe") {
        return app
            .arguments
            .first()
            .and_then(|a| a.strip_prefix("shell:AppsFolder\\"))
            .and_then(|id| id.split('_').next())
            .is_some_and(|package| {
                package.len() > 3
                    && process
                        .to_lowercase()
                        .contains(&format!("\\{}", package.to_lowercase()))
            });
    }
    process.eq_ignore_ascii_case(&app.executable)
}
impl Drop for NativeDesktop {
    fn drop(&mut self) {
        self.release_keys();
    }
}
fn key(s: &str) -> Result<Key, String> {
    Ok(match s.to_lowercase().as_str() {
        "enter" => Key::Return,
        "tab" => Key::Tab,
        "escape" => Key::Escape,
        "backspace" => Key::Backspace,
        "delete" => Key::Delete,
        "control" | "ctrl" => Key::Control,
        "alt" => Key::Alt,
        "shift" => Key::Shift,
        "meta" | "win" => Key::Meta,
        "space" => Key::Space,
        "up" => Key::UpArrow,
        "down" => Key::DownArrow,
        "left" => Key::LeftArrow,
        "right" => Key::RightArrow,
        "home" => Key::Home,
        "end" => Key::End,
        _ => {
            let mut c = s.chars();
            let first = c.next().ok_or("Empty key")?;
            if c.next().is_some() {
                return Err("Unsupported key".into());
            }
            Key::Unicode(first)
        }
    })
}

fn capture(b: &Bounds) -> Result<String, String> {
    use base64::Engine;
    use std::io::Cursor;
    if b.width <= 0 || b.height <= 0 || b.width > 10000 || b.height > 10000 {
        return Err("Invalid screenshot bounds".into());
    }
    unsafe {
        let dc = GetDC(None);
        let mem = CreateCompatibleDC(Some(dc));
        let bmp = CreateCompatibleBitmap(dc, b.width, b.height);
        let old = SelectObject(mem, HGDIOBJ(bmp.0));
        let copied = BitBlt(mem, 0, 0, b.width, b.height, Some(dc), b.x, b.y, SRCCOPY);
        SelectObject(mem, old);
        let mut info = BITMAPINFO::default();
        info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = b.width;
        info.bmiHeader.biHeight = -b.height;
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        let mut pixels = vec![0u8; (b.width * b.height * 4) as usize];
        let lines = GetDIBits(
            mem,
            bmp,
            0,
            b.height as u32,
            Some(pixels.as_mut_ptr() as *mut _),
            &mut info,
            DIB_RGB_COLORS,
        );
        let _ = DeleteObject(HGDIOBJ(bmp.0));
        let _ = DeleteDC(mem);
        ReleaseDC(None, dc);
        copied.map_err(|e| e.to_string())?;
        if lines == 0 {
            return Err("Screenshot capture failed".into());
        }
        for p in pixels.chunks_exact_mut(4) {
            p.swap(0, 2);
            p[3] = 255;
        }
        let image = image::RgbaImage::from_raw(b.width as u32, b.height as u32, pixels)
            .ok_or("Screenshot conversion failed")?;
        let mut encoded = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut encoded, image::ImageFormat::Png)
            .map_err(|e| e.to_string())?;
        Ok(base64::engine::general_purpose::STANDARD.encode(encoded.into_inner()))
    }
}
