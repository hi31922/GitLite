use std::ffi::{OsStr, OsString};
use std::mem::size_of;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::ptr::null;

use windows_sys::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows_sys::Win32::Graphics::Gdi::HFONT;
use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::Dialogs::{
    GetOpenFileNameW, OFN_EXPLORER, OFN_FILEMUSTEXIST, OFN_HIDEREADONLY, OFN_NOCHANGEDIR,
    OFN_PATHMUSTEXIST, OPENFILENAMEW,
};
use windows_sys::Win32::UI::Controls::EM_SETSEL;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows_sys::Win32::UI::Shell::{
    BFFM_INITIALIZED, BFFM_SETSELECTIONW, BIF_EDITBOX, BIF_NEWDIALOGSTYLE, BIF_RETURNONLYFSDIRS,
    BROWSEINFOW, SHBrowseForFolderW, SHGetPathFromIDListEx,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BS_DEFPUSHBUTTON, DS_CENTER, DS_MODALFRAME, DS_SETFONT, DialogBoxIndirectParamW,
    ES_AUTOHSCROLL, EndDialog, GWLP_USERDATA, GetDlgItem, GetWindowLongPtrW, GetWindowTextLengthW,
    GetWindowTextW, IDCANCEL, IDOK, SendMessageW, SetWindowLongPtrW, WM_CLOSE, WM_COMMAND,
    WM_INITDIALOG, WM_SETFONT, WS_CAPTION, WS_CHILD, WS_EX_CLIENTEDGE, WS_POPUP, WS_SYSMENU,
    WS_TABSTOP, WS_VISIBLE,
};
use windows_sys::w;

const PATH_CAPACITY: usize = 32_768;
const TITLE_CONTROL: u16 = 101;
const INPUT_CONTROL: u16 = 102;

pub fn pick_folder(owner: HWND, initial: Option<&Path>, title: &str) -> Option<PathBuf> {
    let mut display = [0u16; 260];
    let initial = initial.map(|path| wide(path.as_os_str()));
    let title = wide(OsStr::new(title));
    let request = BROWSEINFOW {
        hwndOwner: owner,
        pszDisplayName: display.as_mut_ptr(),
        lpszTitle: title.as_ptr(),
        ulFlags: BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE | BIF_EDITBOX,
        lpfn: Some(folder_opened),
        lParam: initial.as_ref().map_or(0, |path| path.as_ptr() as LPARAM),
        ..Default::default()
    };
    unsafe {
        let item = SHBrowseForFolderW(&request);
        if item.is_null() {
            return None;
        }
        let mut path = vec![0u16; PATH_CAPACITY];
        let resolved = SHGetPathFromIDListEx(item, path.as_mut_ptr(), path.len() as u32, 0);
        CoTaskMemFree(item.cast());
        if resolved == 0 {
            None
        } else {
            read_path(&path)
        }
    }
}

unsafe extern "system" fn folder_opened(
    window: HWND,
    message: u32,
    _: LPARAM,
    initial: LPARAM,
) -> i32 {
    if message == BFFM_INITIALIZED && initial != 0 {
        unsafe { SendMessageW(window, BFFM_SETSELECTIONW, 1, initial) };
    }
    0
}

pub fn pick_git(owner: HWND) -> Option<PathBuf> {
    let mut path = vec![0u16; PATH_CAPACITY];
    let mut request = OPENFILENAMEW {
        lStructSize: size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: owner,
        lpstrFilter: w!("Git executable (git.exe)\0git.exe\0Executables (*.exe)\0*.exe\0"),
        nFilterIndex: 1,
        lpstrFile: path.as_mut_ptr(),
        nMaxFile: path.len() as u32,
        lpstrTitle: w!("Choose Git executable"),
        Flags: OFN_EXPLORER
            | OFN_FILEMUSTEXIST
            | OFN_PATHMUSTEXIST
            | OFN_HIDEREADONLY
            | OFN_NOCHANGEDIR,
        ..Default::default()
    };
    if unsafe { GetOpenFileNameW(&mut request) } == 0 {
        None
    } else {
        read_path(&path)
    }
}

pub fn prompt(
    owner: HWND,
    font: HFONT,
    caption: &str,
    hint: &str,
    initial: &str,
    accept_label: &str,
) -> Option<String> {
    let template = prompt_template(caption, hint, initial, accept_label);
    let mut state = PromptState { font, value: None };
    let result = unsafe {
        DialogBoxIndirectParamW(
            GetModuleHandleW(null()),
            template.as_ptr().cast(),
            owner,
            Some(prompt_proc),
            (&mut state as *mut PromptState) as LPARAM,
        )
    };
    if result == IDOK as isize {
        state.value
    } else {
        None
    }
}

struct PromptState {
    font: HFONT,
    value: Option<String>,
}

unsafe extern "system" fn prompt_proc(
    dialog: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> isize {
    unsafe {
        match message {
            WM_INITDIALOG => {
                SetWindowLongPtrW(dialog, GWLP_USERDATA, lparam);
                let state = &*(lparam as *const PromptState);
                if !state.font.is_null() {
                    for control in [
                        i32::from(TITLE_CONTROL),
                        i32::from(INPUT_CONTROL),
                        IDOK,
                        IDCANCEL,
                    ] {
                        SendMessageW(
                            GetDlgItem(dialog, control),
                            WM_SETFONT,
                            state.font as WPARAM,
                            0,
                        );
                    }
                }
                let edit = GetDlgItem(dialog, i32::from(INPUT_CONTROL));
                SetFocus(edit);
                SendMessageW(edit, EM_SETSEL, 0, -1);
                0 // Focus was set explicitly.
            }
            WM_COMMAND => match (wparam & 0xffff) as i32 {
                IDOK => {
                    let state = GetWindowLongPtrW(dialog, GWLP_USERDATA) as *mut PromptState;
                    if let Some(state) = state.as_mut() {
                        let edit = GetDlgItem(dialog, i32::from(INPUT_CONTROL));
                        let mut text = vec![0u16; GetWindowTextLengthW(edit) as usize + 1];
                        let length = GetWindowTextW(edit, text.as_mut_ptr(), text.len() as i32);
                        state.value = Some(String::from_utf16_lossy(&text[..length as usize]));
                    }
                    EndDialog(dialog, IDOK as isize);
                    1
                }
                IDCANCEL => {
                    EndDialog(dialog, IDCANCEL as isize);
                    1
                }
                _ => 0,
            },
            WM_CLOSE => {
                EndDialog(dialog, IDCANCEL as isize);
                1
            }
            _ => 0,
        }
    }
}

fn prompt_template(caption: &str, hint: &str, initial: &str, accept_label: &str) -> Vec<u32> {
    const BUTTON_CLASS: u16 = 0x0080;
    const EDIT_CLASS: u16 = 0x0081;
    const STATIC_CLASS: u16 = 0x0082;
    const STATIC_NO_PREFIX: u32 = 0x0080;
    let mut words = TemplateWords(Vec::new());
    let style =
        WS_POPUP | WS_CAPTION | WS_SYSMENU | (DS_CENTER | DS_MODALFRAME | DS_SETFONT) as u32;
    words.dword(style);
    words.dword(0);
    words.0.extend([4, 0, 0, 260, 79, 0, 0]);
    words.string(caption);
    words.0.push(9);
    words.string("Segoe UI");

    words.control(
        TITLE_CONTROL,
        STATIC_CLASS,
        STATIC_NO_PREFIX,
        0,
        [10, 9, 240, 19],
        hint,
    );
    words.control(
        INPUT_CONTROL,
        EDIT_CLASS,
        WS_TABSTOP | ES_AUTOHSCROLL as u32,
        WS_EX_CLIENTEDGE,
        [10, 31, 240, 15],
        initial,
    );
    words.control(
        IDOK as u16,
        BUTTON_CLASS,
        WS_TABSTOP | BS_DEFPUSHBUTTON as u32,
        0,
        [139, 55, 53, 15],
        accept_label,
    );
    words.control(
        IDCANCEL as u16,
        BUTTON_CLASS,
        WS_TABSTOP,
        0,
        [197, 55, 53, 15],
        "Cancel",
    );
    words.align_dword();
    // Packing into u32 storage meets the dialog manager's alignment requirement.
    words
        .0
        .chunks_exact(2)
        .map(|pair| u32::from(pair[0]) | (u32::from(pair[1]) << 16))
        .collect()
}

struct TemplateWords(Vec<u16>);

impl TemplateWords {
    fn align_dword(&mut self) {
        if self.0.len() % 2 != 0 {
            self.0.push(0);
        }
    }

    fn dword(&mut self, value: u32) {
        self.0.extend([value as u16, (value >> 16) as u16]);
    }

    fn string(&mut self, text: &str) {
        self.0
            .extend(text.encode_utf16().take_while(|&unit| unit != 0));
        self.0.push(0);
    }

    fn control(&mut self, id: u16, class: u16, style: u32, ext: u32, bounds: [u16; 4], text: &str) {
        self.align_dword();
        self.dword(WS_CHILD | WS_VISIBLE | style);
        self.dword(ext);
        self.0.extend(bounds);
        self.0.extend([id, 0xffff, class]);
        self.string(text);
        self.0.push(0); // No creation data.
    }
}

fn wide(text: &OsStr) -> Vec<u16> {
    text.encode_wide()
        .take_while(|&unit| unit != 0)
        .chain([0])
        .collect()
}

fn read_path(buffer: &[u16]) -> Option<PathBuf> {
    let end = buffer
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(buffer.len());
    (end != 0).then(|| PathBuf::from(OsString::from_wide(&buffer[..end])))
}
