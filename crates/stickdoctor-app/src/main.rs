#![windows_subsystem = "windows"]

use native_windows_gui as nwg;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use stickdoctor_core::{
    enumerate_disks, execute_repair_with_progress, format_progress, new_logger, DiskCandidate,
    FsChoice, RepairOptions, SharedLog,
};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const APP_TITLE: &str = "StickDoctor";

fn main() {
    if !is_elevated() && !relaunch_elevated() {
        return;
    }

    nwg::init().expect("Failed to init Native Windows GUI");
    nwg::Font::set_global_family("Segoe UI").expect("Failed to set font");

    build_ui();
}

// ---------------------------------------------------------------- settings

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Settings {
    fs: u8,
    label: String,
    quick: bool,
    zero: bool,
    rebuild_only: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            fs: 0,
            label: "STICK".to_string(),
            quick: true,
            zero: false,
            rebuild_only: false,
        }
    }
}

fn settings_path() -> Option<PathBuf> {
    std::env::var("APPDATA")
        .ok()
        .map(|p| PathBuf::from(p).join("StickDoctor").join("config.json"))
}

fn load_settings() -> Settings {
    let path = match settings_path() {
        Some(p) => p,
        None => return Settings::default(),
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
        Err(_) => Settings::default(),
    }
}

fn save_settings(settings: &Settings) {
    if let Some(path) = settings_path() {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(text) = serde_json::to_string_pretty(settings) {
            let _ = std::fs::write(path, text);
        }
    }
}

// ---------------------------------------------------------------- elevation

fn is_elevated() -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::Security::{
        GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut ret_len: u32 = 0;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            &mut elevation as *mut _ as *mut std::ffi::c_void,
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut ret_len,
        );
        CloseHandle(token);
        ok != 0 && elevation.TokenIsElevated != 0
    }
}

fn relaunch_elevated() -> bool {
    use std::env;
    use std::ptr;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOW;

    let exe_path = match env::current_exe() {
        Ok(p) => p,
        Err(_) => {
            warn_box("Cannot determine the executable path.");
            return false;
        }
    };
    let exe_w: Vec<u16> = exe_path
        .to_string_lossy()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let verb: Vec<u16> = "runas".encode_utf16().chain(std::iter::once(0)).collect();
    let result = unsafe {
        ShellExecuteW(
            ptr::null_mut(),
            verb.as_ptr(),
            exe_w.as_ptr(),
            ptr::null(),
            ptr::null(),
            SW_SHOW,
        )
    };
    if (result as isize) <= 32 {
        warn_box(
            "Administrator rights required.\n\n\
             StickDoctor needs Administrator rights to read and repair raw disks.\n\
             Right-click StickDoctor.exe and choose 'Run as administrator'.",
        );
        return false;
    }
    true
}

fn warn_box(text: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
    let t: Vec<u16> = APP_TITLE.encode_utf16().chain(std::iter::once(0)).collect();
    let m: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            m.as_ptr(),
            t.as_ptr(),
            MB_OK | MB_ICONERROR,
        )
    };
}

// ------------------------------------------------------------------- state

struct State {
    candidates: RefCell<Vec<DiskCandidate>>,
    log_slot: Arc<Mutex<Option<SharedLog>>>,
    stage_slot: Arc<Mutex<String>>,
    base_pct: Arc<AtomicU8>,
    busy: Arc<AtomicBool>,
    last_signature: RefCell<String>,
}

#[allow(deprecated)] // nwg::Timer is deprecated in favour of AnimationTimer but still works
fn build_ui() {
    let settings = load_settings();

    let mut window: nwg::Window = Default::default();
    let mut title: nwg::Label = Default::default();
    let mut subtitle: nwg::Label = Default::default();
    let mut devices_label: nwg::Label = Default::default();
    let mut table: nwg::ListView = Default::default();
    let mut details_label: nwg::Label = Default::default();
    let mut details: nwg::TextBox = Default::default();
    let mut fs_label: nwg::Label = Default::default();
    let mut fs_combo: nwg::ComboBox<String> = Default::default();
    let mut name_label: nwg::Label = Default::default();
    let mut label_input: nwg::TextInput = Default::default();
    let mut quick_check: nwg::CheckBox = Default::default();
    let mut zero_check: nwg::CheckBox = Default::default();
    let mut rebuild_check: nwg::CheckBox = Default::default();
    let mut refresh_btn: nwg::Button = Default::default();
    let mut repair_btn: nwg::Button = Default::default();
    let mut save_btn: nwg::Button = Default::default();
    let mut copy_btn: nwg::Button = Default::default();
    let mut progress: nwg::ProgressBar = Default::default();
    let mut status: nwg::Label = Default::default();
    let mut log_label: nwg::Label = Default::default();
    let mut log_text: nwg::TextBox = Default::default();
    let mut notice: nwg::Notice = Default::default();
    let mut timer: nwg::Timer = Default::default();

    nwg::Window::builder()
        .size((1000, 800))
        .position((160, 20))
        .title(&format!("{} {} - USB stick recovery", APP_TITLE, VERSION))
        .build(&mut window)
        .expect("window");

    nwg::Label::builder()
        .text(&format!("{} {}", APP_TITLE, VERSION))
        .parent(&window)
        .build(&mut title)
        .expect("title");
    nwg::Label::builder()
        .text("Recover a USB stick that Windows can no longer format or see after writing a raw image.")
        .parent(&window)
        .build(&mut subtitle)
        .expect("subtitle");
    nwg::Label::builder()
        .text("1. Select the stick you want to repair")
        .parent(&window)
        .build(&mut devices_label)
        .expect("devices label");

    nwg::ListView::builder()
        .list_style(nwg::ListViewStyle::Detailed)
        .parent(&window)
        .build(&mut table)
        .expect("table");

    nwg::Label::builder()
        .text("2. Review what is on it")
        .parent(&window)
        .build(&mut details_label)
        .expect("details label");
    nwg::TextBox::builder()
        .parent(&window)
        .build(&mut details)
        .expect("details");
    details.set_readonly(true);

    nwg::Label::builder()
        .text("Filesystem")
        .parent(&window)
        .build(&mut fs_label)
        .expect("fs label");
    nwg::ComboBox::builder()
        .parent(&window)
        .build(&mut fs_combo)
        .expect("fs combo");
    nwg::Label::builder()
        .text("Label")
        .parent(&window)
        .build(&mut name_label)
        .expect("name label");
    nwg::TextInput::builder()
        .text(&settings.label)
        .parent(&window)
        .build(&mut label_input)
        .expect("label input");
    nwg::CheckBox::builder()
        .text("Quick format")
        .parent(&window)
        .build(&mut quick_check)
        .expect("quick");
    nwg::CheckBox::builder()
        .text("Zero first/last 1 MiB")
        .parent(&window)
        .build(&mut zero_check)
        .expect("zero");
    nwg::CheckBox::builder()
        .text("Only rebuild partition table")
        .parent(&window)
        .build(&mut rebuild_check)
        .expect("rebuild");

    nwg::Button::builder()
        .text("Refresh")
        .parent(&window)
        .build(&mut refresh_btn)
        .expect("refresh");
    nwg::Button::builder()
        .text("REPAIR STICK")
        .parent(&window)
        .build(&mut repair_btn)
        .expect("repair");
    nwg::Button::builder()
        .text("Save log")
        .parent(&window)
        .build(&mut save_btn)
        .expect("save");
    nwg::Button::builder()
        .text("Copy log")
        .parent(&window)
        .build(&mut copy_btn)
        .expect("copy");

    nwg::ProgressBar::builder()
        .parent(&window)
        .range(0..100)
        .build(&mut progress)
        .expect("progress");

    nwg::Label::builder()
        .text("Ready.")
        .parent(&window)
        .build(&mut status)
        .expect("status");
    nwg::Label::builder()
        .text("Log")
        .parent(&window)
        .build(&mut log_label)
        .expect("log label");
    nwg::TextBox::builder()
        .parent(&window)
        .build(&mut log_text)
        .expect("log text");
    log_text.set_readonly(true);

    nwg::Notice::builder()
        .parent(&window)
        .build(&mut notice)
        .expect("notice");
    nwg::Timer::builder()
        .parent(&window)
        .interval(2000)
        .build(&mut timer)
        .expect("timer");

    let window_icon = nwg::Icon::from_bin(include_bytes!("../assets/stickdoctor.ico")).ok();
    if let Some(ref icon) = window_icon {
        window.set_icon(Some(icon));
    }

    // ---- layout ----
    title.set_position(18, 12);
    title.set_size(964, 26);
    subtitle.set_position(18, 40);
    subtitle.set_size(964, 20);
    devices_label.set_position(18, 70);
    devices_label.set_size(964, 18);
    table.set_position(18, 90);
    table.set_size(964, 150);
    details_label.set_position(18, 248);
    details_label.set_size(964, 18);
    details.set_position(18, 268);
    details.set_size(964, 110);

    fs_label.set_position(18, 390);
    fs_label.set_size(80, 20);
    fs_combo.set_position(102, 386);
    fs_combo.set_size(130, 180);
    name_label.set_position(248, 390);
    name_label.set_size(42, 20);
    label_input.set_position(294, 386);
    label_input.set_size(150, 24);
    quick_check.set_position(454, 386);
    quick_check.set_size(120, 24);
    zero_check.set_position(582, 386);
    zero_check.set_size(180, 24);
    rebuild_check.set_position(770, 386);
    rebuild_check.set_size(212, 24);

    refresh_btn.set_position(18, 424);
    refresh_btn.set_size(110, 32);
    repair_btn.set_position(138, 424);
    repair_btn.set_size(190, 32);
    save_btn.set_position(338, 424);
    save_btn.set_size(110, 32);
    copy_btn.set_position(458, 424);
    copy_btn.set_size(110, 32);
    progress.set_position(580, 430);
    progress.set_size(402, 18);
    status.set_position(18, 464);
    status.set_size(964, 20);

    log_label.set_position(18, 488);
    log_label.set_size(964, 18);
    log_text.set_position(18, 508);
    log_text.set_size(964, 276);

    for (i, (text, width)) in [
        ("Disk", 60),
        ("Bus", 70),
        ("Size", 90),
        ("Model", 300),
        ("Partition table", 200),
        ("Safety", 200),
    ]
    .iter()
    .enumerate()
    {
        table.insert_column(nwg::InsertListViewColumn {
            index: Some(i as i32),
            fmt: None,
            width: Some(*width),
            text: Some(text.to_string()),
        });
    }

    fs_combo.push("FAT32".to_string());
    fs_combo.push("exFAT".to_string());
    fs_combo.push("NTFS".to_string());
    fs_combo.set_selection(Some(settings.fs.min(2) as usize));
    quick_check.set_check_state(if settings.quick {
        nwg::CheckBoxState::Checked
    } else {
        nwg::CheckBoxState::Unchecked
    });
    zero_check.set_check_state(if settings.zero {
        nwg::CheckBoxState::Checked
    } else {
        nwg::CheckBoxState::Unchecked
    });
    rebuild_check.set_check_state(if settings.rebuild_only {
        nwg::CheckBoxState::Checked
    } else {
        nwg::CheckBoxState::Unchecked
    });

    let state = Rc::new(State {
        candidates: RefCell::new(Vec::new()),
        log_slot: Arc::new(Mutex::new(None)),
        stage_slot: Arc::new(Mutex::new(String::new())),
        base_pct: Arc::new(AtomicU8::new(0)),
        busy: Arc::new(AtomicBool::new(false)),
        last_signature: RefCell::new(String::new()),
    });

    refresh_ui(&table, &details, &status, &state);

    let window = Rc::new(window);
    let window_h = window.clone();
    let st = state.clone();
    let sender = notice.sender();
    let stage_slot = state.stage_slot.clone();
    let base_pct = state.base_pct.clone();
    let log_slot = state.log_slot.clone();

    let handler = nwg::full_bind_event_handler(&window.handle, move |evt, _data, handle| {
        use nwg::Event as E;
        match evt {
            E::OnButtonClick => {
                if handle == refresh_btn {
                    refresh_ui(&table, &details, &status, &st);
                } else if handle == repair_btn {
                    on_repair(
                        &window_h,
                        &table,
                        &fs_combo,
                        &label_input,
                        &quick_check,
                        &zero_check,
                        &rebuild_check,
                        &log_text,
                        &status,
                        &progress,
                        &repair_btn,
                        &st,
                        sender,
                    );
                } else if handle == save_btn {
                    save_log(window_h.as_ref(), &log_text, &status);
                } else if handle == copy_btn {
                    copy_log(window_h.as_ref(), &log_text, &status);
                }
            }
            E::OnListViewClick | E::OnListViewDoubleClick => {
                if handle == table {
                    update_details(&table, &details, &st);
                }
            }
            E::OnNotice => {
                if let Some(log) = log_slot.lock().unwrap().as_ref() {
                    if let Ok(l) = log.lock() {
                        log_text.set_text(&l.render());
                        log_text.scroll_lastline();
                    }
                }
                let stage = stage_slot.lock().map(|s| s.clone()).unwrap_or_default();
                if !stage.is_empty() {
                    status.set_text(&stage);
                }
                let base = base_pct.load(Ordering::Relaxed) as u32;
                let pct = if (52..88).contains(&base) {
                    let p = format_progress() as u32;
                    52 + p * 36 / 100
                } else {
                    base
                };
                progress.set_pos(pct);
                if !st.busy.load(Ordering::Relaxed) {
                    repair_btn.set_enabled(true);
                }
            }
            E::OnTimerTick => {
                // Hotplug detection: refresh only when the set of disks changes.
                if !st.busy.load(Ordering::Relaxed) {
                    let signature = disk_signature();
                    if signature != *st.last_signature.borrow() {
                        *st.last_signature.borrow_mut() = signature;
                        refresh_ui(&table, &details, &status, &st);
                    }
                }
            }
            E::OnWindowClose => {
                save_settings(&current_settings(
                    &fs_combo,
                    &label_input,
                    &quick_check,
                    &zero_check,
                    &rebuild_check,
                ));
                nwg::stop_thread_dispatch();
            }
            _ => {}
        }
    });

    *state.last_signature.borrow_mut() = disk_signature();

    nwg::dispatch_thread_events();
    nwg::unbind_event_handler(&handler);
}

fn disk_signature() -> String {
    enumerate_disks()
        .unwrap_or_default()
        .iter()
        .filter_map(|d| d.drive_index)
        .map(|i| i.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

fn current_settings(
    fs_combo: &nwg::ComboBox<String>,
    label_input: &nwg::TextInput,
    quick: &nwg::CheckBox,
    zero: &nwg::CheckBox,
    rebuild: &nwg::CheckBox,
) -> Settings {
    Settings {
        fs: fs_combo.selection().unwrap_or(0) as u8,
        label: label_input.text(),
        quick: quick.check_state() == nwg::CheckBoxState::Checked,
        zero: zero.check_state() == nwg::CheckBoxState::Checked,
        rebuild_only: rebuild.check_state() == nwg::CheckBoxState::Checked,
    }
}

fn refresh_ui(table: &nwg::ListView, details: &nwg::TextBox, status: &nwg::Label, state: &State) {
    table.clear();
    let devices = enumerate_disks().unwrap_or_default();
    let mut candidates = Vec::new();
    for dev in &devices {
        candidates.push(DiskCandidate::probe(dev));
    }

    for (row, c) in candidates.iter().enumerate() {
        let cells = [
            c.device
                .drive_index
                .map(|n| n.to_string())
                .unwrap_or_else(|| "?".into()),
            c.properties.bus_type.to_string(),
            format!("{:.1} GiB", c.properties.size_gib()),
            c.properties
                .model
                .clone()
                .unwrap_or_else(|| "Unknown".into()),
            c.sector_zero
                .as_ref()
                .map(|s| s.style.to_string())
                .unwrap_or_else(|| "(unreadable)".into()),
            c.safety.label().to_string(),
        ];
        for (col, text) in cells.iter().enumerate() {
            table.insert_item(nwg::InsertListViewItem {
                index: Some(row as i32),
                column_index: col as i32,
                text: Some(text.clone()),
                image: None,
            });
        }
    }

    let count = candidates.len();
    *state.candidates.borrow_mut() = candidates;

    if count == 0 {
        details.set_text("No disks detected. Run StickDoctor as Administrator.");
        status.set_text("No disks detected.");
    } else {
        status.set_text(&format!(
            "{} disk(s) detected. Select your USB stick.",
            count
        ));
        table.select_item(0, true);
        update_details(table, details, state);
    }
}

fn update_details(table: &nwg::ListView, details: &nwg::TextBox, state: &State) {
    let candidates = state.candidates.borrow();
    match table.selected_item().and_then(|i| candidates.get(i)) {
        Some(c) => {
            let mut text = c.details_text();
            text.push_str(&format!("\nSafety: {}", c.safety.label()));
            if !c.can_repair() {
                text.push_str("\n\nThis disk will NOT be repaired (safety rule).");
            }
            details.set_text(&text);
        }
        None => details.set_text(""),
    }
}

#[allow(clippy::too_many_arguments)]
fn on_repair(
    window: &nwg::Window,
    table: &nwg::ListView,
    fs_combo: &nwg::ComboBox<String>,
    label_input: &nwg::TextInput,
    quick_check: &nwg::CheckBox,
    zero_check: &nwg::CheckBox,
    rebuild_check: &nwg::CheckBox,
    log_text: &nwg::TextBox,
    status: &nwg::Label,
    progress: &nwg::ProgressBar,
    repair_btn: &nwg::Button,
    state: &State,
    sender: nwg::NoticeSender,
) {
    if state.busy.load(Ordering::Relaxed) {
        status.set_text("A repair is already running.");
        return;
    }

    let candidates = state.candidates.borrow();
    let candidate = match table.selected_item().and_then(|i| candidates.get(i)) {
        Some(c) => c,
        None => {
            status.set_text("Select a disk first.");
            return;
        }
    };

    if !candidate.can_repair() {
        drop(candidates);
        nwg::modal_error_message(
            window,
            "Repair refused",
            "Only USB devices that are not the Windows system disk can be repaired.",
        );
        status.set_text("Repair refused (safety).");
        return;
    }

    let index = candidate.device.drive_index.unwrap_or(0);
    let model = candidate
        .properties
        .model
        .clone()
        .unwrap_or_else(|| "Unknown".into());
    let size = candidate.properties.size_gib();
    let drive = candidate.properties.bus_type;

    let fs_sel = fs_combo.selection().unwrap_or(0);
    let mut fs = match fs_sel {
        1 => FsChoice::ExFat,
        2 => FsChoice::Ntfs,
        _ => FsChoice::Fat32,
    };
    drop(candidates);

    let rebuild_only = rebuild_check.check_state() == nwg::CheckBoxState::Checked;

    if !rebuild_only && fs == FsChoice::Fat32 && size > 32.0 {
        let choice = nwg::modal_message(
            window,
            &nwg::MessageParams {
                title: "FAT32 size limit",
                content: &format!(
                    "This stick is {:.1} GiB. Windows cannot format FAT32 above 32 GiB.\n\nFormat as exFAT instead?\n\nYes = exFAT, No = cancel",
                    size
                ),
                buttons: nwg::MessageButtons::YesNo,
                icons: nwg::MessageIcons::Warning,
            },
        );
        if choice == nwg::MessageChoice::Yes {
            fs = FsChoice::ExFat;
        } else {
            status.set_text("Cancelled (FAT32 too large).");
            return;
        }
    }

    let action = if rebuild_only {
        "rebuild the partition table (no format)".to_string()
    } else {
        format!("format as {}", fs)
    };

    let confirm = nwg::modal_message(
        window,
        &nwg::MessageParams {
            title: "Confirm destructive operation",
            content: &format!(
                "ALL DATA on this disk will be destroyed.\n\nDisk: PhysicalDrive{}\nModel: {}\nBus: {}\nSize: {:.1} GiB\nAction: {}\n\nContinue?",
                index, model, drive, size, action
            ),
            buttons: nwg::MessageButtons::YesNo,
            icons: nwg::MessageIcons::Warning,
        },
    );
    if confirm != nwg::MessageChoice::Yes {
        status.set_text("Cancelled.");
        return;
    }

    let opts = RepairOptions {
        fs,
        label: label_input.text(),
        quick: quick_check.check_state() == nwg::CheckBoxState::Checked,
        zero_sectors: zero_check.check_state() == nwg::CheckBoxState::Checked,
        rebuild_only,
    };

    let log = new_logger();
    *state.log_slot.lock().unwrap() = Some(log.clone());
    *state.stage_slot.lock().unwrap() = "Starting...".to_string();
    state.base_pct.store(0, Ordering::Relaxed);
    state.busy.store(true, Ordering::Relaxed);
    repair_btn.set_enabled(false);
    log_text.set_text("");
    status.set_text("Starting...");
    progress.set_pos(0);

    let log_thread = log.clone();
    let stage_slot = state.stage_slot.clone();
    let base_pct = state.base_pct.clone();
    let busy = state.busy.clone();

    std::thread::spawn(move || {
        let poll_sender = sender;
        let done = Arc::new(AtomicBool::new(false));
        let done_poll = done.clone();
        let poller = std::thread::spawn(move || {
            while !done_poll.load(Ordering::Relaxed) {
                poll_sender.notice();
                std::thread::sleep(std::time::Duration::from_millis(150));
            }
            poll_sender.notice();
        });

        let stage_cb = {
            let slot = stage_slot.clone();
            let pct_slot = base_pct.clone();
            move |s: &str, pct: u8| {
                if let Ok(mut g) = slot.lock() {
                    *g = s.to_string();
                }
                pct_slot.store(pct, Ordering::Relaxed);
            }
        };

        let result = execute_repair_with_progress(index, &opts, &log_thread, &stage_cb);
        match &result {
            Ok(()) => {
                if let Ok(mut g) = stage_slot.lock() {
                    *g = if rebuild_only {
                        "Partition table rebuilt.".to_string()
                    } else {
                        "Repair complete - the stick is ready.".to_string()
                    };
                }
                base_pct.store(100, Ordering::Relaxed);
            }
            Err(e) => {
                if let Ok(mut g) = stage_slot.lock() {
                    *g = format!("Repair failed: {}", e);
                }
                if let Ok(mut l) = log_thread.lock() {
                    l.error(format!("Repair failed: {}", e));
                }
            }
        }
        busy.store(false, Ordering::Relaxed);
        done.store(true, Ordering::Relaxed);
        let _ = poller.join();
    });
}

fn save_log(window: &nwg::Window, log_text: &nwg::TextBox, status: &nwg::Label) {
    let mut dialog: nwg::FileDialog = Default::default();
    let default_folder = std::env::temp_dir().to_string_lossy().to_string();
    if nwg::FileDialog::builder()
        .title("Save log")
        .action(nwg::FileDialogAction::Save)
        .filters("Log (*.log)|*.log|Text (*.txt)|*.txt")
        .default_folder(default_folder)
        .build(&mut dialog)
        .is_err()
    {
        status.set_text("Could not open the save dialog.");
        return;
    }
    if !dialog.run(Some(window)) {
        status.set_text("Save cancelled.");
        return;
    }
    match dialog.get_selected_item() {
        Ok(path) => match std::fs::write(&path, log_text.text()) {
            Ok(()) => status.set_text(&format!("Log saved to {}", path.to_string_lossy())),
            Err(e) => status.set_text(&format!("Could not save log: {e}")),
        },
        Err(_) => status.set_text("Could not read the selected path."),
    }
}

fn copy_log(window: &nwg::Window, log_text: &nwg::TextBox, status: &nwg::Label) {
    let text = log_text.text();
    if text.is_empty() {
        status.set_text("Nothing to copy.");
        return;
    }
    nwg::Clipboard::set_data_text(window, &text);
    status.set_text("Log copied to the clipboard.");
}
