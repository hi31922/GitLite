use std::cell::{Cell, RefCell};
use std::ffi::{OsStr, OsString};
use std::io;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::sync::mpsc::{self, Receiver, TryRecvError};

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Dwm::{
    DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE,
    DwmSetWindowAttribute,
};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::*;
use windows_sys::Win32::UI::HiDpi::*;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::Shell::{
    DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass, ShellExecuteW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;
use windows_sys::core::{PCWSTR, w};

use crate::dialogs;
use crate::git::{self, Change, Git, Snapshot};
use crate::github::{self, AccountStatus};
use crate::theme::{self, Theme, Tone};

const CLASS: PCWSTR = w!("GitLiteRepositories");
const TITLE: PCWSTR = w!("GitLite");
const TIMER: usize = 1;
const PATH: u16 = 101;
const OPEN: u16 = 102;
const CLONE: u16 = 103;
const CONNECT: u16 = 104;
const BRANCH: u16 = 105;
const SWITCH: u16 = 106;
const NEW_BRANCH: u16 = 107;
const FETCH: u16 = 108;
const PULL: u16 = 109;
const PUSH: u16 = 110;
const REFRESH: u16 = 111;
const CHANGES: u16 = 112;
const STAGE: u16 = 113;
const UNSTAGE: u16 = 114;
const STAGE_ALL: u16 = 115;
const WORKTREE: u16 = 116;
const STAGED: u16 = 117;
const HISTORY: u16 = 118;
const PREVIEW: u16 = 119;
const LOG: u16 = 120;
const COMMIT_MESSAGE: u16 = 121;
const COMMIT: u16 = 122;
const AUTHOR: u16 = 123;
const STATUS: u16 = 124;
const ACCOUNT: u16 = 125;
const CHANGES_TITLE: u16 = 126;
const PREVIEW_TITLE: u16 = 127;
const MESSAGE_TITLE: u16 = 128;
const PUBLISH_PATH: u16 = 129;
const PUBLISH_BROWSE: u16 = 130;
const PUBLISH_URL: u16 = 131;
const PUBLISH_FOLDER: u16 = 132;
const PUBLISH_PATH_LABEL: u16 = 133;
const PUBLISH_URL_LABEL: u16 = 134;
const SS_ENDELLIPSIS: u32 = 0x4000;
const HEADER_SUBCLASS: usize = 1;
const BRANCH_SUBCLASS: usize = 2;
const TABLE_SELECTED: COLORREF = 0x394331;
const TABLE_ALTERNATE: COLORREF = 0x3b2a1b;

#[derive(Clone, Copy)]
struct Controls {
    path: HWND,
    open: HWND,
    clone_repo: HWND,
    connect: HWND,
    publish_path_label: HWND,
    publish_path: HWND,
    publish_browse: HWND,
    publish_url_label: HWND,
    publish_url: HWND,
    publish_folder: HWND,
    branch: HWND,
    switch_branch: HWND,
    new_branch: HWND,
    fetch: HWND,
    pull: HWND,
    push: HWND,
    refresh: HWND,
    changes: HWND,
    changes_title: HWND,
    stage: HWND,
    unstage: HWND,
    stage_all: HWND,
    worktree: HWND,
    staged: HWND,
    history: HWND,
    preview_title: HWND,
    preview: HWND,
    log: HWND,
    message: HWND,
    message_title: HWND,
    commit: HWND,
    author: HWND,
    status: HWND,
    account: HWND,
}

impl Controls {
    fn create(window: HWND) -> io::Result<Self> {
        let button = |id, caption| {
            child(
                window,
                id,
                w!("BUTTON"),
                caption,
                WS_TABSTOP | BS_OWNERDRAW as u32,
                0,
            )
        };
        let label = |id, caption| child(window, id, w!("STATIC"), caption, SS_ENDELLIPSIS, 0);
        let edit = |id, style| child(window, id, w!("EDIT"), w!(""), style, 0);
        let list = |id| {
            child(
                window,
                id,
                WC_LISTVIEWW,
                w!(""),
                WS_TABSTOP
                    | LVS_REPORT
                    | LVS_SINGLESEL
                    | LVS_SHOWSELALWAYS
                    | LVS_NOSORTHEADER
                    | LVS_OWNERDRAWFIXED,
                0,
            )
        };
        let controls = Self {
            path: edit(
                PATH,
                WS_TABSTOP | ES_READONLY as u32 | ES_AUTOHSCROLL as u32,
            )?,
            open: button(OPEN, w!("Open…"))?,
            clone_repo: button(CLONE, w!("Clone…"))?,
            connect: button(CONNECT, w!("Connect GitHub"))?,
            publish_path_label: label(PUBLISH_PATH_LABEL, w!("Folder to publish"))?,
            publish_path: edit(PUBLISH_PATH, WS_TABSTOP | ES_AUTOHSCROLL as u32)?,
            publish_browse: button(PUBLISH_BROWSE, w!("Browse…"))?,
            publish_url_label: label(PUBLISH_URL_LABEL, w!("GitHub repository link"))?,
            publish_url: edit(PUBLISH_URL, WS_TABSTOP | ES_AUTOHSCROLL as u32)?,
            publish_folder: button(PUBLISH_FOLDER, w!("Publish folder"))?,
            branch: child(
                window,
                BRANCH,
                w!("COMBOBOX"),
                w!(""),
                WS_TABSTOP
                    | WS_VSCROLL
                    | (CBS_DROPDOWNLIST | CBS_OWNERDRAWFIXED | CBS_HASSTRINGS) as u32,
                0,
            )?,
            switch_branch: button(SWITCH, w!("Switch"))?,
            new_branch: button(NEW_BRANCH, w!("New branch…"))?,
            fetch: button(FETCH, w!("Fetch"))?,
            pull: button(PULL, w!("Pull"))?,
            push: button(PUSH, w!("Push"))?,
            refresh: button(REFRESH, w!("Refresh"))?,
            changes_title: label(CHANGES_TITLE, w!("Changes"))?,
            changes: list(CHANGES)?,
            stage: button(STAGE, w!("Stage"))?,
            unstage: button(UNSTAGE, w!("Unstage"))?,
            stage_all: button(STAGE_ALL, w!("Stage all"))?,
            worktree: button(WORKTREE, w!("Worktree"))?,
            staged: button(STAGED, w!("Staged"))?,
            history: button(HISTORY, w!("History"))?,
            preview_title: label(PREVIEW_TITLE, w!("Worktree diff"))?,
            preview: edit(
                PREVIEW,
                WS_TABSTOP
                    | WS_HSCROLL
                    | WS_VSCROLL
                    | ES_MULTILINE as u32
                    | ES_READONLY as u32
                    | ES_AUTOHSCROLL as u32
                    | ES_AUTOVSCROLL as u32,
            )?,
            log: list(LOG)?,
            message: edit(
                COMMIT_MESSAGE,
                WS_TABSTOP
                    | WS_VSCROLL
                    | ES_MULTILINE as u32
                    | ES_AUTOVSCROLL as u32
                    | ES_WANTRETURN as u32,
            )?,
            message_title: label(MESSAGE_TITLE, w!("Commit message"))?,
            commit: button(COMMIT, w!("Commit staged"))?,
            author: button(AUTHOR, w!("Author…"))?,
            status: label(STATUS, w!(""))?,
            account: label(ACCOUNT, w!(""))?,
        };

        unsafe {
            SendMessageW(
                controls.path,
                EM_SETCUEBANNER,
                1,
                w!("Repository folder") as isize,
            );
            SendMessageW(controls.publish_path, EM_SETLIMITTEXT, 32_767, 0);
            SendMessageW(controls.publish_url, EM_SETLIMITTEXT, 2_048, 0);
            SendMessageW(controls.message, EM_SETLIMITTEXT, 64 * 1024, 0);
            for table in [controls.changes, controls.log] {
                SetWindowTheme(table, w!("DarkMode_Explorer"), null());
                SendMessageW(table, LVM_SETBKCOLOR, 0, theme::FIELD as isize);
                SendMessageW(table, LVM_SETTEXTBKCOLOR, 0, theme::FIELD as isize);
                SendMessageW(table, LVM_SETTEXTCOLOR, 0, theme::TEXT as isize);
                SendMessageW(
                    table,
                    LVM_SETEXTENDEDLISTVIEWSTYLE,
                    0,
                    (LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER | LVS_EX_LABELTIP) as isize,
                );
            }
            for field in [
                controls.path,
                controls.publish_path,
                controls.publish_url,
                controls.preview,
                controls.message,
                controls.branch,
            ] {
                SetWindowTheme(field, w!("DarkMode_Explorer"), null());
            }
        }
        columns(controls.changes, &[w!("File"), w!("Index"), w!("Worktree")]);
        columns(
            controls.log,
            &[w!("Commit"), w!("Date"), w!("Subject"), w!("Author")],
        );
        for table in [controls.changes, controls.log] {
            let header = unsafe { SendMessageW(table, LVM_GETHEADER, 0, 0) } as HWND;
            if !header.is_null() {
                unsafe { SetWindowSubclass(header, Some(header_proc), HEADER_SUBCLASS, 0) };
            }
        }
        unsafe { SetWindowSubclass(controls.branch, Some(branch_proc), BRANCH_SUBCLASS, 0) };
        Ok(controls)
    }

    fn children(self) -> [HWND; 34] {
        [
            self.path,
            self.open,
            self.clone_repo,
            self.connect,
            self.publish_path_label,
            self.publish_path,
            self.publish_browse,
            self.publish_url_label,
            self.publish_url,
            self.publish_folder,
            self.branch,
            self.switch_branch,
            self.new_branch,
            self.fetch,
            self.pull,
            self.push,
            self.refresh,
            self.changes,
            self.changes_title,
            self.stage,
            self.unstage,
            self.stage_all,
            self.worktree,
            self.staged,
            self.history,
            self.preview_title,
            self.preview,
            self.log,
            self.message,
            self.commit,
            self.author,
            self.status,
            self.account,
            self.message_title,
        ]
    }
}

fn child(
    window: HWND,
    id: u16,
    class: PCWSTR,
    text: PCWSTR,
    styles: u32,
    ex_style: u32,
) -> io::Result<HWND> {
    let handle = unsafe {
        CreateWindowExW(
            ex_style,
            class,
            text,
            WS_CHILD | WS_VISIBLE | styles,
            0,
            0,
            0,
            0,
            window,
            id as usize as HMENU,
            GetModuleHandleW(null()),
            null(),
        )
    };
    if handle.is_null() {
        Err(io::Error::last_os_error())
    } else {
        Ok(handle)
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Page {
    Worktree,
    Staged,
    History,
    Commit,
}

struct Repository {
    root: PathBuf,
    info: Snapshot,
}

enum MainOutcome {
    Git(PathBuf),
    Open(PathBuf, Snapshot),
    PublishNeedsAuthor {
        folder: PathBuf,
        url: String,
    },
    Change {
        root: PathBuf,
        action: Result<String, String>,
        snapshot: Result<Snapshot, String>,
        clear_message: bool,
    },
}

struct MainJob {
    result: Receiver<Result<MainOutcome, String>>,
}

struct AccountJob {
    signin: bool,
    result: Receiver<Result<AccountStatus, String>>,
}

enum Inspect {
    Diff(Change, bool),
    Commit(String),
}

struct PreviewRequest {
    revision: u64,
    executable: PathBuf,
    root: PathBuf,
    operation: Inspect,
}

struct PreviewJob {
    revision: u64,
    result: Receiver<Result<String, String>>,
}

struct App {
    theme: Theme,
    window: Cell<HWND>,
    controls: Cell<Option<Controls>>,
    font: Cell<HFONT>,
    mono: Cell<HFONT>,
    init_error: RefCell<Option<io::Error>>,
    executable: RefCell<Option<PathBuf>>,
    repository: RefCell<Option<Repository>>,
    main_job: RefCell<Option<MainJob>>,
    account: RefCell<Option<Result<AccountStatus, String>>>,
    account_job: RefCell<Option<AccountJob>>,
    page: Cell<Page>,
    commit_preview: RefCell<Option<String>>,
    render_guard: Cell<bool>,
    preview_revision: Cell<u64>,
    preview_request: RefCell<Option<PreviewRequest>>,
    preview_job: RefCell<Option<PreviewJob>>,
}

impl App {
    fn new(executable: Option<PathBuf>) -> io::Result<Self> {
        Ok(Self {
            theme: Theme::new()?,
            window: Cell::new(null_mut()),
            controls: Cell::new(None),
            font: Cell::new(null_mut()),
            mono: Cell::new(null_mut()),
            init_error: RefCell::new(None),
            executable: RefCell::new(executable),
            repository: RefCell::new(None),
            main_job: RefCell::new(None),
            account: RefCell::new(None),
            account_job: RefCell::new(None),
            page: Cell::new(Page::Worktree),
            commit_preview: RefCell::new(None),
            render_guard: Cell::new(false),
            preview_revision: Cell::new(0),
            preview_request: RefCell::new(None),
            preview_job: RefCell::new(None),
        })
    }

    fn controls(&self) -> Controls {
        self.controls.get().expect("controls have been created")
    }

    fn dpi(&self) -> u32 {
        let dpi = unsafe { GetDpiForWindow(self.window.get()) };
        if dpi == 0 { 96 } else { dpi }
    }

    fn unit(&self, value: i32) -> i32 {
        (value * self.dpi() as i32 + 48) / 96
    }

    fn fonts(&self) {
        let controls = self.controls();
        for (target, size, face) in [
            (&self.font, 16, w!("Segoe UI")),
            (&self.mono, 15, w!("Consolas")),
        ] {
            let font = unsafe {
                CreateFontW(
                    -self.unit(size),
                    0,
                    0,
                    0,
                    FW_NORMAL as i32,
                    0,
                    0,
                    0,
                    DEFAULT_CHARSET as u32,
                    OUT_DEFAULT_PRECIS as u32,
                    CLIP_DEFAULT_PRECIS as u32,
                    CLEARTYPE_QUALITY as u32,
                    DEFAULT_PITCH as u32,
                    face,
                )
            };
            if font.is_null() {
                continue;
            }
            if std::ptr::eq(target, &self.font) {
                for control in controls.children() {
                    if control != controls.preview {
                        unsafe { SendMessageW(control, WM_SETFONT, font as usize, 1) };
                    }
                }
                for table in [controls.changes, controls.log] {
                    let header = unsafe { SendMessageW(table, LVM_GETHEADER, 0, 0) } as HWND;
                    if !header.is_null() {
                        unsafe { SendMessageW(header, WM_SETFONT, font as usize, 1) };
                    }
                }
            } else {
                unsafe { SendMessageW(controls.preview, WM_SETFONT, font as usize, 1) };
            }
            let previous = target.replace(font);
            if !previous.is_null() {
                unsafe { DeleteObject(previous) };
            }
        }
    }

    fn layout(&self) {
        let controls = self.controls();
        let mut client = RECT::default();
        unsafe { GetClientRect(self.window.get(), &mut client) };
        let u = |value| self.unit(value);
        let left = u(15);
        let right = client.right - u(15);
        let width = right - left;
        let lower = client.bottom - u(13);
        let split = left + (width * 43 / 100);
        let divider = u(11);
        let right_start = split + divider;
        let right_width = right - right_start;
        let list_top = u(189);
        let bottom = lower - u(28);
        let work_bottom = bottom - u(70);
        let move_to = |control, x, y, w: i32, h: i32| unsafe {
            MoveWindow(control, x, y, w.max(0), h.max(0), 1);
        };

        move_to(controls.path, left, u(17), width - u(336), u(29));
        move_to(controls.open, right - u(328), u(14), u(83), u(34));
        move_to(controls.clone_repo, right - u(237), u(14), u(82), u(34));
        move_to(controls.connect, right - u(147), u(14), u(147), u(34));

        let folder_width = (width - u(218)) / 2;
        let url_start = left + folder_width + u(95);
        let url_width = right - u(123) - url_start;
        move_to(controls.publish_path_label, left, u(53), folder_width, u(19));
        move_to(controls.publish_path, left, u(75), folder_width, u(29));
        move_to(
            controls.publish_browse,
            left + folder_width + u(8),
            u(72),
            u(79),
            u(34),
        );
        move_to(controls.publish_url_label, url_start, u(53), url_width, u(19));
        move_to(controls.publish_url, url_start, u(75), url_width, u(29));
        move_to(controls.publish_folder, right - u(115), u(72), u(115), u(34));

        move_to(controls.branch, left, u(117), u(195), u(280));
        move_to(controls.switch_branch, left + u(202), u(113), u(70), u(34));
        move_to(controls.new_branch, left + u(280), u(113), u(117), u(34));
        move_to(controls.refresh, right - u(328), u(113), u(85), u(34));
        move_to(controls.fetch, right - u(235), u(113), u(72), u(34));
        move_to(controls.pull, right - u(155), u(113), u(69), u(34));
        move_to(controls.push, right - u(78), u(113), u(78), u(34));

        move_to(controls.changes_title, left, u(163), split - left, u(23));
        move_to(controls.worktree, right_start, u(155), u(91), u(29));
        move_to(controls.staged, right_start + u(96), u(155), u(77), u(29));
        move_to(controls.history, right_start + u(178), u(155), u(83), u(29));
        move_to(
            controls.changes,
            left,
            list_top,
            split - left,
            work_bottom - list_top,
        );
        move_to(
            controls.preview_title,
            right_start,
            list_top,
            right_width,
            u(22),
        );
        let preview_top = list_top + u(22);
        move_to(
            controls.preview,
            right_start,
            preview_top,
            right_width,
            work_bottom - preview_top,
        );
        move_to(
            controls.log,
            right_start,
            preview_top,
            right_width,
            work_bottom - preview_top,
        );

        move_to(controls.stage, left, work_bottom + u(9), u(75), u(32));
        move_to(
            controls.unstage,
            left + u(83),
            work_bottom + u(9),
            u(83),
            u(32),
        );
        move_to(
            controls.stage_all,
            left + u(174),
            work_bottom + u(9),
            u(88),
            u(32),
        );

        move_to(
            controls.message_title,
            right_start,
            work_bottom + u(7),
            right_width - u(135),
            u(20),
        );
        move_to(
            controls.message,
            right_start,
            work_bottom + u(28),
            right_width - u(135),
            u(36),
        );
        move_to(
            controls.commit,
            right - u(127),
            work_bottom + u(8),
            u(127),
            u(28),
        );
        move_to(
            controls.author,
            right - u(127),
            work_bottom + u(38),
            u(127),
            u(26),
        );
        move_to(controls.status, left, bottom + u(7), width - u(250), u(23));
        move_to(
            controls.account,
            right - u(240),
            bottom + u(7),
            u(240),
            u(23),
        );

        let scrollbar = unsafe { GetSystemMetricsForDpi(SM_CXVSCROLL, self.dpi()) };
        let first = split - left - u(165) - scrollbar - u(5);
        set_column_widths(controls.changes, &[first.max(u(110)), u(58), u(107)]);
        let subject = right_width - u(261) - scrollbar - u(5);
        set_column_widths(controls.log, &[u(78), u(94), subject.max(u(110)), u(89)]);
        rounded_child(controls.path, width - u(336), u(29), u(9));
        rounded_child(controls.publish_path, folder_width, u(29), u(9));
        rounded_child(controls.publish_url, url_width, u(29), u(9));
        rounded_child(controls.changes, split - left, work_bottom - list_top, u(9));
        rounded_child(
            controls.preview,
            right_width,
            work_bottom - preview_top,
            u(9),
        );
        rounded_child(controls.log, right_width, work_bottom - preview_top, u(9));
        rounded_child(controls.message, right_width - u(135), u(36), u(9));
        unsafe {
            let padding = u(8) as u32;
            for field in [
                controls.path,
                controls.publish_path,
                controls.publish_url,
                controls.preview,
                controls.message,
            ] {
                SendMessageW(
                    field,
                    EM_SETMARGINS,
                    (EC_LEFTMARGIN | EC_RIGHTMARGIN) as usize,
                    (padding | padding << 16) as isize,
                );
            }
            SendMessageW(
                controls.branch,
                CB_SETITEMHEIGHT,
                usize::MAX,
                u(25) as isize,
            );
            SendMessageW(controls.branch, CB_SETITEMHEIGHT, 0, u(25) as isize);
            InvalidateRect(self.window.get(), null(), 0);
        }
    }

    fn paint(&self, window: HWND) {
        let mut paint = PAINTSTRUCT::default();
        let hdc = unsafe { BeginPaint(window, &mut paint) };
        if hdc.is_null() {
            return;
        }
        self.paint_contents(window, hdc);
        unsafe { EndPaint(window, &paint) };
    }

    fn paint_contents(&self, window: HWND, hdc: HDC) {
        let mut client = RECT::default();
        unsafe { GetClientRect(window, &mut client) };
        self.theme.paint(hdc, &client);
        if self.controls.get().is_some() {
            let u = |unit| self.unit(unit);
            let split = u(15) + (client.right - 2 * u(15)) * 43 / 100;
            let center = split + u(5);
            let bottom = client.bottom - u(38);
            for area in [
                RECT {
                    left: u(7),
                    top: u(7),
                    right: client.right - u(7),
                    bottom: u(152),
                },
                RECT {
                    left: u(7),
                    top: u(156),
                    right: center - u(2),
                    bottom,
                },
                RECT {
                    left: center + u(2),
                    top: u(152),
                    right: client.right - u(7),
                    bottom,
                },
            ] {
                self.theme.card(hdc, &area, u(14));
            }
        }
    }

    fn draw_item(&self, item: &DRAWITEMSTRUCT) -> bool {
        let controls = self.controls();
        match item.CtlID as u16 {
            CHANGES | LOG if item.itemID != u32::MAX => {
                draw_table(item, self.font.get(), self.unit(7));
            }
            BRANCH => {
                let caption = branch_caption(controls.branch, item.itemID as usize);
                self.theme.row(item, &caption, self.unit(8));
            }
            _ if item.CtlType == ODT_BUTTON => {
                let tone = match item.CtlID as u16 {
                    COMMIT | CONNECT | PUBLISH_FOLDER => Tone::Primary,
                    WORKTREE if self.page.get() == Page::Worktree => Tone::Active,
                    STAGED if self.page.get() == Page::Staged => Tone::Active,
                    HISTORY if self.page.get() == Page::History => Tone::Active,
                    _ => Tone::Regular,
                };
                self.theme
                    .button(item, &wide(get_text(item.hwndItem)), tone, self.unit(10));
            }
            _ => return false,
        }
        true
    }

    fn setup(&self) {
        self.refresh_preview();
        self.status(if self.executable.borrow().is_some() {
            "Ready. Open a repository or publish a folder."
        } else {
            "Git was not found. Choose Locate Git or install Git for Windows."
        });
        self.account_label();
        self.buttons();
        if self.executable.borrow().is_some() {
            self.start_account(false);
        }
    }

    fn status(&self, text: &str) {
        set_text(self.controls().status, text);
    }

    fn shown_change(&self) -> Option<Change> {
        let index = selected(self.controls().changes)?;
        self.repository
            .borrow()
            .as_ref()?
            .info
            .changes
            .get(index)
            .cloned()
    }

    fn shown_branch(&self) -> Option<String> {
        let index = unsafe { SendMessageW(self.controls().branch, CB_GETCURSEL, 0, 0) };
        let state = self.repository.borrow();
        let repo = state.as_ref()?;
        let branch = repo.info.branches.get(usize::try_from(index).ok()?)?;
        if branch.current || (!repo.info.detached && repo.info.branch == branch.name) {
            None
        } else {
            Some(branch.name.clone())
        }
    }

    fn buttons(&self) {
        let controls = self.controls();
        let installed = self.executable.borrow().is_some();
        let busy = self.main_job.borrow().is_some();
        let change = self.shown_change();
        let other_branch = self.shown_branch().is_some();
        let (opened, remotes, staged, worktree, has_commits, on_branch) = {
            let state = self.repository.borrow();
            if let Some(repo) = state.as_ref() {
                (
                    true,
                    !repo.info.remotes.is_empty(),
                    repo.info.changes.iter().any(|change| change.staged),
                    repo.info.changes.iter().any(|change| change.unstaged),
                    !repo.info.unborn,
                    !repo.info.detached,
                )
            } else {
                (false, false, false, false, false, false)
            }
        };
        let change_worktree = change.as_ref().is_some_and(|item| item.unstaged);
        let change_staged = change.as_ref().is_some_and(|item| item.staged);
        let auth_busy = self.account_job.borrow().is_some();
        let auth = self.account.borrow();
        let connected = matches!(
            auth.as_ref(),
            Some(Ok(AccountStatus::SignedIn(_) | AccountStatus::Multiple(_)))
        );
        drop(auth);
        let commit_text = !get_text(controls.message).trim().is_empty();
        let publish_fields = !get_text(controls.publish_path).trim().is_empty()
            && !get_text(controls.publish_url).trim().is_empty();
        let ready = installed && !busy;
        for (control, enabled) in [
            (controls.open, !busy),
            (controls.clone_repo, !busy),
            (controls.connect, installed && !auth_busy),
            (controls.publish_path, ready),
            (controls.publish_browse, ready),
            (controls.publish_url, ready),
            (controls.publish_folder, ready && !auth_busy && publish_fields),
            (controls.refresh, ready),
            (controls.branch, ready && opened),
            (controls.switch_branch, ready && other_branch),
            (controls.new_branch, ready && opened),
            (controls.fetch, ready && remotes),
            (controls.pull, ready && remotes && on_branch),
            (controls.push, ready && remotes && on_branch && has_commits),
            (controls.stage, ready && change_worktree),
            (controls.unstage, ready && change_staged),
            (controls.stage_all, ready && worktree),
            (
                controls.worktree,
                opened && self.page.get() != Page::Worktree,
            ),
            (controls.staged, opened && self.page.get() != Page::Staged),
            (controls.history, opened && self.page.get() != Page::History),
            (controls.message, ready && opened),
            (controls.commit, ready && staged && commit_text),
            (controls.author, ready && opened),
        ] {
            unsafe { EnableWindow(control, i32::from(enabled)) };
        }
        set_text(
            controls.open,
            if installed {
                "Open…"
            } else {
                "Locate Git…"
            },
        );
        set_text(
            controls.clone_repo,
            if installed { "Clone…" } else { "Get Git" },
        );
        set_text(
            controls.connect,
            if connected {
                "GitHub account"
            } else {
                "Connect GitHub"
            },
        );
    }

    fn account_label(&self) {
        let active = self.account_job.borrow();
        let context = self.account.borrow();
        let text = if let Some(job) = active.as_ref() {
            if job.signin {
                "GitHub · waiting for browser".to_owned()
            } else {
                "GitHub · checking accounts".to_owned()
            }
        } else {
            match context.as_ref() {
                Some(Ok(AccountStatus::SignedIn(name))) => format!("GitHub · {name}"),
                Some(Ok(AccountStatus::Multiple(names))) => {
                    format!("GitHub · {} saved accounts", names.len())
                }
                Some(Ok(AccountStatus::SignedOut)) => "GitHub · signed out".to_owned(),
                Some(Err(_)) => "GitHub · sign-in unavailable".to_owned(),
                None => "GitHub · not connected".to_owned(),
            }
        };
        drop(active);
        drop(context);
        set_text(self.controls().account, &text);
    }

    fn start_account(&self, signin: bool) {
        if self.account_job.borrow().is_some() {
            return;
        }
        let Some(git) = self.executable.borrow().clone() else {
            return;
        };
        let started = background("gitlite-github", move || {
            if signin {
                github::sign_in(&git)
            } else {
                github::account(&git)
            }
        });
        match started {
            Ok(result) => *self.account_job.borrow_mut() = Some(AccountJob { signin, result }),
            Err(error) => *self.account.borrow_mut() = Some(Err(error.to_string())),
        }
        self.account_label();
        self.buttons();
    }

    fn connect(&self) {
        let status = {
            let account = self.account.borrow();
            match account.as_ref() {
                Some(Ok(AccountStatus::SignedIn(name))) => Some(format!(
                    "Git Credential Manager has a saved sign-in for {name}.\n\nContinue to GitHub sign-in?"
                )),
                Some(Ok(AccountStatus::Multiple(names))) => Some(format!(
                    "Saved GitHub accounts:\n{}\n\nGit chooses an account for each HTTPS remote. Continue to sign-in?",
                    names.join("\n")
                )),
                _ => None,
            }
        };
        if let Some(status) = status {
            if !confirm(self.window.get(), "GitHub account", &status) {
                return;
            }
        }
        self.start_account(true);
    }

    fn poll_account(&self) {
        let ready = {
            let mut active = self.account_job.borrow_mut();
            let Some(job) = active.as_ref() else { return };
            let result = match job.result.try_recv() {
                Ok(value) => value,
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    Err("GitHub sign-in ended unexpectedly.".to_owned())
                }
            };
            let job = active.take().expect("account job exists");
            (job.signin, result)
        };
        let (signin, result) = ready;
        let failure = result.as_ref().err().cloned();
        *self.account.borrow_mut() = Some(result);
        self.account_label();
        self.buttons();
        if signin {
            if let Some(error) = failure {
                alert(self.window.get(), "GitHub sign-in", &error);
            }
        }
    }

    fn start_main(
        &self,
        title: &'static str,
        task: impl FnOnce() -> Result<MainOutcome, String> + Send + 'static,
    ) {
        if self.main_job.borrow().is_some() {
            return;
        }
        match background("gitlite-repository", task) {
            Ok(result) => {
                *self.main_job.borrow_mut() = Some(MainJob { result });
                self.status(title);
                self.buttons();
            }
            Err(error) => alert(self.window.get(), "Could not start Git", &error.to_string()),
        }
    }

    fn poll_main(&self) {
        let result = {
            let mut active = self.main_job.borrow_mut();
            let Some(job) = active.as_ref() else { return };
            let result = match job.result.try_recv() {
                Ok(result) => result,
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    Err("The Git command ended unexpectedly.".into())
                }
            };
            active.take();
            result
        };
        match result {
            Ok(MainOutcome::Git(path)) => {
                *self.executable.borrow_mut() = Some(path.clone());
                self.status("Git is ready. Open a repository or publish a folder.");
                if let Err(error) = remember_git(&path) {
                    self.status(&format!("Git is ready for this session. {error}"));
                }
                self.start_account(false);
            }
            Ok(MainOutcome::Open(path, info)) => {
                self.install_repository(path, info);
                self.status(&self.repo_status());
            }
            Ok(MainOutcome::PublishNeedsAuthor { folder, url }) => {
                self.status("Set a Git author to commit and publish this folder.");
                if let Some(author) = self.prompt_author() {
                    self.start_publish(folder, url, Some(author));
                }
            }
            Ok(MainOutcome::Change {
                root,
                action,
                snapshot,
                clear_message,
            }) => {
                if action.is_ok() && clear_message {
                    set_text(self.controls().message, "");
                }
                if let Ok(info) = snapshot {
                    self.install_repository(root, info);
                } else {
                    self.refresh_preview();
                }
                match action {
                    Ok(message) => {
                        let summary = message
                            .lines()
                            .find(|line| !line.trim().is_empty())
                            .unwrap_or_default()
                            .trim();
                        self.status(if summary.is_empty() { "Done." } else { summary });
                    }
                    Err(error) => {
                        self.status(&self.repo_status());
                        alert(self.window.get(), "Git command failed", &error);
                    }
                }
            }
            Err(error) => {
                self.status(&self.repo_status());
                alert(self.window.get(), "Git command failed", &error);
            }
        }
        self.buttons();
    }

    fn command(
        &self,
        title: &'static str,
        clear_message: bool,
        operation: impl FnOnce(&Git, &Path) -> Result<String, String> + Send + 'static,
    ) {
        let Some(executable) = self.executable.borrow().clone() else {
            return;
        };
        let Some(root) = self
            .repository
            .borrow()
            .as_ref()
            .map(|repo| repo.root.clone())
        else {
            return;
        };
        self.start_main(title, move || {
            let git = Git::new(executable);
            let action = operation(&git, &root);
            let snapshot = git.snapshot(&root);
            Ok(MainOutcome::Change {
                root,
                action,
                snapshot,
                clear_message,
            })
        });
    }

    fn open(&self) {
        if self.main_job.borrow().is_some() {
            return;
        }
        if self.executable.borrow().is_none() {
            if let Some(path) = dialogs::pick_git(self.window.get()) {
                self.start_main("Checking Git…", move || {
                    git::validate_git(&path)?;
                    Ok(MainOutcome::Git(path))
                });
            }
            return;
        }
        let starting_folder = self
            .repository
            .borrow()
            .as_ref()
            .map(|repo| repo.root.clone());
        if let Some(path) = dialogs::pick_folder(
            self.window.get(),
            starting_folder.as_deref(),
            "Choose a Git repository folder.",
        ) {
            self.open_path(path);
        }
    }

    fn open_path(&self, path: PathBuf) {
        let Some(executable) = self.executable.borrow().clone() else {
            return;
        };
        self.start_main("Opening repository…", move || {
            let git = Git::new(executable);
            let root = git.open(&path)?;
            let snapshot = git.snapshot(&root)?;
            Ok(MainOutcome::Open(root, snapshot))
        });
    }

    fn clone_repo(&self) {
        if self.main_job.borrow().is_some() {
            return;
        }
        let Some(executable) = self.executable.borrow().clone() else {
            let result = unsafe {
                ShellExecuteW(
                    self.window.get(),
                    w!("open"),
                    w!("https://git-scm.com/download/win"),
                    null(),
                    null(),
                    SW_SHOWNORMAL,
                )
            };
            if result as usize <= 32 {
                self.status("Open git-scm.com/download/win to get Git for Windows.");
            }
            return;
        };
        let window = self.window.get();
        let Some(url) = dialogs::prompt(
            window,
            self.font.get(),
            "Clone repository",
            "Repository URL or local path",
            "",
            "Next",
        ) else {
            return;
        };
        let url = url.trim().to_owned();
        if url.is_empty() {
            return;
        }
        let Some(destination) = dialogs::pick_folder(
            window,
            None,
            "Choose an empty folder for the clone. Use Make New Folder to create one.",
        ) else {
            return;
        };
        self.start_main("Cloning repository…", move || {
            let git = Git::new(executable);
            git.clone_repo(&url, &destination)?;
            let root = git.open(&destination)?;
            let snapshot = git.snapshot(&root)?;
            Ok(MainOutcome::Open(root, snapshot))
        });
    }

    fn browse_publish_folder(&self) {
        let controls = self.controls();
        let field = get_text(controls.publish_path);
        let path = Path::new(field.trim().trim_matches('"'));
        let opened = self
            .repository
            .borrow()
            .as_ref()
            .map(|repo| repo.root.clone());
        let starting_folder = if path.is_absolute() {
            Some(path)
        } else {
            opened.as_deref()
        };
        if let Some(folder) = dialogs::pick_folder(
            self.window.get(),
            starting_folder,
            "Choose the folder you want to publish.",
        ) {
            set_text(controls.publish_path, &folder);
        }
    }

    fn publish_folder(&self) {
        let controls = self.controls();
        let folder = get_text(controls.publish_path);
        let folder = PathBuf::from(folder.trim().trim_matches('"'));
        if !folder.is_absolute() {
            alert(
                self.window.get(),
                "Publish folder",
                "Enter a full folder path or use Browse.",
            );
            return;
        }
        match git::github_publish_url(&get_text(controls.publish_url)) {
            Ok(url) => self.start_publish(folder, url, None),
            Err(error) => alert(self.window.get(), "Publish folder", &error),
        }
    }

    fn start_publish(&self, folder: PathBuf, url: String, author: Option<(String, String)>) {
        let Some(executable) = self.executable.borrow().clone() else {
            return;
        };
        self.start_main("Publishing folder to GitHub…", move || {
            let git = Git::new(executable);
            if let Some((name, email)) = author {
                let root = git.publish_root(&folder)?;
                git.set_identity(&root, &name, &email)?;
            }
            match git.publish_folder(&folder, &url) {
                Ok(message) => {
                    let root = git.open(&folder).unwrap_or(folder);
                    let snapshot = git.snapshot(&root);
                    Ok(MainOutcome::Change {
                        root,
                        action: Ok(message),
                        snapshot,
                        clear_message: false,
                    })
                }
                Err(error) if error == git::PUBLISH_IDENTITY_REQUIRED => {
                    Ok(MainOutcome::PublishNeedsAuthor { folder, url })
                }
                Err(error) => Err(error),
            }
        });
    }

    fn refresh(&self) {
        if let Some(root) = self
            .repository
            .borrow()
            .as_ref()
            .map(|repo| repo.root.clone())
        {
            let Some(executable) = self.executable.borrow().clone() else {
                return;
            };
            self.start_main("Refreshing repository…", move || {
                let snapshot = Git::new(executable).snapshot(&root)?;
                Ok(MainOutcome::Open(root, snapshot))
            });
        }
        self.start_account(false);
    }

    fn repo_status(&self) -> String {
        let repo = self.repository.borrow();
        let Some(repo) = repo.as_ref() else {
            return "Open a repository or publish a folder.".into();
        };
        let info = &repo.info;
        let counts = format!(
            "{} changed · {} staged",
            info.changes.len(),
            info.changes.iter().filter(|item| item.staged).count()
        );
        if info.unborn {
            return format!("{} · No commits yet · {counts}", info.branch);
        }
        if let Some(upstream) = &info.upstream {
            return format!(
                "{} → {upstream} · {counts} · {} ahead, {} behind",
                info.branch, info.ahead, info.behind
            );
        }
        format!("{} · {counts}", info.branch)
    }

    fn install_repository(&self, root: PathBuf, info: Snapshot) {
        let same = self
            .repository
            .borrow()
            .as_ref()
            .is_some_and(|repo| repo.root == root);
        let previous = if same {
            self.shown_change().map(|change| change.path)
        } else {
            None
        };
        if !same {
            self.page.set(Page::Worktree);
            *self.commit_preview.borrow_mut() = None;
            set_text(self.controls().message, "");
        }
        set_text(self.controls().path, &root);
        let matching =
            previous.and_then(|file| info.changes.iter().position(|item| item.path == file));
        let selected = matching.or_else(|| (!info.changes.is_empty()).then_some(0));
        *self.repository.borrow_mut() = Some(Repository { root, info });
        self.render_repository(selected);
    }

    fn render_repository(&self, selection: Option<usize>) {
        let controls = self.controls();
        let (branch_labels, current, changes, history, changed_count) = {
            let state = self.repository.borrow();
            let Some(repo) = state.as_ref() else { return };
            let info = &repo.info;
            let mut current = info.branches.iter().position(|branch| branch.current);
            let mut branches: Vec<_> = info
                .branches
                .iter()
                .map(|branch| {
                    if let Some(upstream) = &branch.upstream {
                        wide(format!("{}  ({upstream})", branch.name))
                    } else {
                        wide(&branch.name)
                    }
                })
                .collect();
            if current.is_none() {
                current = Some(branches.len());
                branches.push(wide(&info.branch));
            }
            let rows = info
                .changes
                .iter()
                .map(|change| {
                    let file = if let Some(original) = &change.original_path {
                        format!("{} → {}", original.display(), change.path.display())
                    } else {
                        change.path.to_string_lossy().into_owned()
                    };
                    [
                        wide(file),
                        wide(status_name(change.index_status, true)),
                        wide(status_name(change.worktree_status, false)),
                    ]
                })
                .collect::<Vec<_>>();
            let history = info
                .history
                .iter()
                .map(|commit| {
                    [
                        wide(&commit.short_id),
                        wide(&commit.date),
                        wide(&commit.subject),
                        wide(&commit.author),
                    ]
                })
                .collect::<Vec<_>>();
            (branches, current, rows, history, info.changes.len())
        };
        self.render_guard.set(true);
        unsafe {
            SendMessageW(controls.branch, CB_RESETCONTENT, 0, 0);
            for caption in &branch_labels {
                SendMessageW(controls.branch, CB_ADDSTRING, 0, caption.as_ptr() as isize);
            }
            if let Some(index) = current {
                SendMessageW(controls.branch, CB_SETCURSEL, index, 0);
            }
        }
        display_rows(controls.changes, &changes);
        display_rows(controls.log, &history);
        if let Some(index) = selection {
            select_row(controls.changes, index);
        }
        self.render_guard.set(false);
        set_text(controls.changes_title, format!("Changes ({changed_count})"));
        self.change_selected();
    }

    fn select_page(&self, page: Page) {
        self.page.set(page);
        self.refresh_preview();
        self.buttons();
    }

    fn change_selected(&self) {
        if self.render_guard.get() {
            return;
        }
        if let Some(change) = self.shown_change() {
            match self.page.get() {
                Page::Worktree if !change.unstaged && change.staged => self.page.set(Page::Staged),
                Page::Staged if !change.staged && change.unstaged => self.page.set(Page::Worktree),
                _ => {}
            }
        }
        self.refresh_preview();
        self.buttons();
    }

    fn open_commit(&self, index: i32) {
        let Ok(index) = usize::try_from(index) else {
            return;
        };
        let id = self
            .repository
            .borrow()
            .as_ref()
            .and_then(|repo| repo.info.history.get(index))
            .map(|commit| commit.id.clone());
        if let Some(id) = id {
            *self.commit_preview.borrow_mut() = Some(id);
            self.select_page(Page::Commit);
        }
    }

    fn refresh_preview(&self) {
        let controls = self.controls();
        let page = self.page.get();
        let revision = self.preview_revision.get().wrapping_add(1);
        self.preview_revision.set(revision);
        *self.preview_request.borrow_mut() = None;
        unsafe {
            ShowWindow(
                controls.preview,
                if page == Page::History {
                    SW_HIDE
                } else {
                    SW_SHOW
                },
            );
            ShowWindow(
                controls.log,
                if page == Page::History {
                    SW_SHOW
                } else {
                    SW_HIDE
                },
            );
        }
        if page == Page::History {
            set_text(
                controls.preview_title,
                "Recent commits · double-click to inspect",
            );
            return;
        }
        let Some(root) = self
            .repository
            .borrow()
            .as_ref()
            .map(|repo| repo.root.clone())
        else {
            set_text(controls.preview_title, "Repository preview");
            set_text(controls.preview, "Open a repository or publish a folder.");
            return;
        };
        let Some(executable) = self.executable.borrow().clone() else {
            return;
        };
        let operation = if page == Page::Commit {
            let Some(id) = self.commit_preview.borrow().clone() else {
                return;
            };
            set_text(
                controls.preview_title,
                format!("Commit {}", id.chars().take(12).collect::<String>()),
            );
            Inspect::Commit(id)
        } else {
            let staged = page == Page::Staged;
            let description = if staged {
                "Staged diff"
            } else {
                "Working-copy diff"
            };
            set_text(controls.preview_title, description);
            let Some(change) = self.shown_change() else {
                let state = self.repository.borrow();
                let clean = state
                    .as_ref()
                    .is_some_and(|repo| repo.info.changes.is_empty());
                set_text(
                    controls.preview,
                    if clean {
                        "The working tree is clean."
                    } else {
                        "Select a file to see its diff."
                    },
                );
                return;
            };
            if (staged && !change.staged) || (!staged && !change.unstaged) {
                set_text(
                    controls.preview,
                    if staged {
                        "This file has no staged changes."
                    } else {
                        "This file has no unstaged changes."
                    },
                );
                return;
            }
            Inspect::Diff(change, staged)
        };
        set_text(controls.preview, "Reading Git output…");
        *self.preview_request.borrow_mut() = Some(PreviewRequest {
            revision,
            executable,
            root,
            operation,
        });
        self.start_preview();
    }

    fn start_preview(&self) {
        if self.preview_job.borrow().is_some() {
            return;
        }
        let Some(request) = self.preview_request.borrow_mut().take() else {
            return;
        };
        let revision = request.revision;
        let started = background("gitlite-diff", move || {
            let git = Git::new(request.executable);
            match request.operation {
                Inspect::Diff(change, staged) => git.diff(&request.root, &change, staged),
                Inspect::Commit(id) => git.commit_detail(&request.root, &id),
            }
        });
        match started {
            Ok(result) => *self.preview_job.borrow_mut() = Some(PreviewJob { revision, result }),
            Err(error) => set_text(self.controls().preview, error.to_string()),
        }
    }

    fn poll_preview(&self) {
        let ready = {
            let mut active = self.preview_job.borrow_mut();
            let Some(job) = active.as_ref() else { return };
            let result = match job.result.try_recv() {
                Ok(value) => value,
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => Err("The preview ended unexpectedly.".into()),
            };
            let job = active.take().expect("preview job exists");
            (job.revision, result)
        };
        if ready.0 == self.preview_revision.get() {
            let text = match ready.1 {
                Ok(content) if content.is_empty() => "No textual diff is available.".into(),
                Ok(content) => content,
                Err(message) => message,
            };
            set_multiline(self.controls().preview, &text);
        }
        self.start_preview();
    }

    fn stage(&self, staged: bool) {
        let Some(change) = self.shown_change() else {
            return;
        };
        if staged && change.unstaged {
            self.command("Staging file…", false, move |git, root| {
                git.stage(root, &change)
            });
        } else if !staged && change.staged {
            self.command("Unstaging file…", false, move |git, root| {
                git.unstage(root, &change)
            });
        }
    }

    fn commit(&self) {
        let text = get_text(self.controls().message).replace("\r\n", "\n");
        if text.trim().is_empty() {
            unsafe { SetFocus(self.controls().message) };
            return;
        }
        if !self
            .repository
            .borrow()
            .as_ref()
            .is_some_and(|repo| repo.info.changes.iter().any(|change| change.staged))
        {
            return;
        }
        self.command("Creating commit…", true, move |git, root| {
            git.commit(root, &text)
        });
    }

    fn prompt_author(&self) -> Option<(String, String)> {
        let window = self.window.get();
        let name = dialogs::prompt(
            window,
            self.font.get(),
            "Commit author",
            "Git author name for this repository",
            "",
            "Next",
        )?;
        let email = dialogs::prompt(
            window,
            self.font.get(),
            "Commit author",
            "Git author email for this repository",
            "",
            "Save",
        )?;
        Some((name, email))
    }

    fn author(&self) {
        let Some((name, email)) = self.prompt_author() else {
            return;
        };
        self.command("Saving commit author…", false, move |git, root| {
            git.set_identity(root, &name, &email)
        });
    }

    fn branch_action(&self, create: bool) {
        if create {
            let Some(name) = dialogs::prompt(
                self.window.get(),
                self.font.get(),
                "New branch",
                "Branch name",
                "",
                "Create",
            ) else {
                return;
            };
            self.command("Creating branch…", false, move |git, root| {
                git.create_branch(root, name.trim())
            });
        } else if let Some(branch) = self.shown_branch() {
            self.command("Switching branch…", false, move |git, root| {
                git.switch(root, &branch)
            });
        }
    }

    fn shortcut(&self, message: &MSG) -> bool {
        if message.message != WM_KEYDOWN {
            return false;
        }
        let control = unsafe { GetKeyState(VK_CONTROL as i32) } < 0;
        match message.wParam as u16 {
            VK_F5 => self.refresh(),
            VK_RETURN if control => self.commit(),
            key if control && key == u16::from(b'O') => self.open(),
            _ => return false,
        }
        true
    }
}

impl Drop for App {
    fn drop(&mut self) {
        for font in [self.font.get(), self.mono.get()] {
            if !font.is_null() {
                unsafe { DeleteObject(font) };
            }
        }
    }
}

fn wide(text: impl AsRef<OsStr>) -> Vec<u16> {
    text.as_ref()
        .encode_wide()
        .map(|character| if character == 0 { 0xfffd } else { character })
        .chain(Some(0))
        .collect()
}

fn set_text(window: HWND, text: impl AsRef<OsStr>) {
    unsafe { SetWindowTextW(window, wide(text).as_ptr()) };
}

fn get_text(window: HWND) -> String {
    unsafe {
        let length = GetWindowTextLengthW(window);
        let mut buffer = vec![0; length as usize + 1];
        let written = GetWindowTextW(window, buffer.as_mut_ptr(), length + 1);
        String::from_utf16_lossy(&buffer[..written.max(0) as usize])
    }
}

fn set_multiline(window: HWND, text: &str) {
    let shortened = text.len() > 460_000;
    let mut stop = text.len().min(460_000);
    while !text.is_char_boundary(stop) {
        stop -= 1
    }
    let mut text = text[..stop].replace("\r\n", "\n").replace('\n', "\r\n");
    if shortened {
        text.push_str("\r\n\r\n[Preview shortened]");
    }
    set_text(window, &text);
    unsafe { SendMessageW(window, EM_SETSEL, 0, 0) };
}

fn columns(list: HWND, labels: &[PCWSTR]) {
    for (index, label) in labels.iter().enumerate() {
        let column = LVCOLUMNW {
            mask: LVCF_TEXT | LVCF_WIDTH,
            pszText: label.cast_mut(),
            cx: 110,
            ..Default::default()
        };
        unsafe { SendMessageW(list, LVM_INSERTCOLUMNW, index, (&raw const column) as isize) };
    }
}

fn set_column_widths(list: HWND, widths: &[i32]) {
    for (index, width) in widths.iter().enumerate() {
        unsafe { SendMessageW(list, LVM_SETCOLUMNWIDTH, index, *width as isize) };
    }
}

fn display_rows<const COLUMNS: usize>(list: HWND, rows: &[[Vec<u16>; COLUMNS]]) {
    unsafe {
        SendMessageW(list, WM_SETREDRAW, 0, 0);
        SendMessageW(list, LVM_DELETEALLITEMS, 0, 0);
        for (index, cells) in rows.iter().enumerate() {
            let mut item = LVITEMW {
                mask: LVIF_TEXT,
                iItem: index as i32,
                pszText: cells[0].as_ptr().cast_mut(),
                ..Default::default()
            };
            let row = SendMessageW(list, LVM_INSERTITEMW, 0, (&raw mut item) as isize);
            if row < 0 {
                continue;
            }
            for (index, text) in cells.iter().enumerate().skip(1) {
                item.iSubItem = index as i32;
                item.pszText = text.as_ptr().cast_mut();
                SendMessageW(
                    list,
                    LVM_SETITEMTEXTW,
                    row as usize,
                    (&raw mut item) as isize,
                );
            }
        }
        SendMessageW(list, WM_SETREDRAW, 1, 0);
        InvalidateRect(list, null(), 1);
    }
}

fn selected(list: HWND) -> Option<usize> {
    let index = unsafe { SendMessageW(list, LVM_GETNEXTITEM, usize::MAX, LVNI_SELECTED as isize) };
    usize::try_from(index).ok()
}

fn select_row(list: HWND, row: usize) {
    let state = LVITEMW {
        state: LVIS_SELECTED | LVIS_FOCUSED,
        stateMask: LVIS_SELECTED | LVIS_FOCUSED,
        ..Default::default()
    };
    unsafe {
        SendMessageW(list, LVM_SETITEMSTATE, row, (&raw const state) as isize);
        SendMessageW(list, LVM_ENSUREVISIBLE, row, 0);
    }
}

fn status_name(status: char, staged: bool) -> &'static str {
    match status {
        'A' => "Added",
        'M' => "Modified",
        'D' => "Deleted",
        'R' => "Renamed",
        'C' => "Copied",
        'T' => "Type change",
        'U' => "Conflict",
        '?' if !staged => "Untracked",
        _ => "",
    }
}

fn background<T: Send + 'static>(
    name: &str,
    work: impl FnOnce() -> T + Send + 'static,
) -> io::Result<Receiver<T>> {
    let (sender, receiver) = mpsc::channel();
    std::thread::Builder::new()
        .name(name.to_owned())
        .spawn(move || {
            let _ = sender.send(work());
        })?;
    Ok(receiver)
}

fn message(window: HWND, title: &str, text: &str, style: MESSAGEBOX_STYLE) -> MESSAGEBOX_RESULT {
    unsafe { MessageBoxW(window, wide(text).as_ptr(), wide(title).as_ptr(), style) }
}

fn alert(window: HWND, title: &str, text: &str) {
    message(window, title, text, MB_OK | MB_ICONERROR);
}

fn confirm(window: HWND, title: &str, text: &str) -> bool {
    message(window, title, text, MB_OKCANCEL | MB_ICONINFORMATION) == IDOK
}

pub fn startup_error(details: &str) {
    alert(null_mut(), "GitLite could not start", details);
}

fn git_preference() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|folder| folder.join("GitLite").join("git.path"))
}

fn preferred_git() -> Option<PathBuf> {
    let bytes = std::fs::read(git_preference()?).ok()?;
    if bytes.len() % 2 != 0 || bytes.is_empty() {
        return None;
    }
    let raw: Vec<_> = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    let path = PathBuf::from(OsString::from_wide(&raw));
    (path.is_absolute() && path.is_file() && git::validate_git(&path).is_ok()).then_some(path)
}

fn remember_git(path: &Path) -> io::Result<()> {
    let Some(location) = git_preference() else {
        return Ok(());
    };
    if let Some(parent) = location.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let bytes: Vec<_> = path
        .as_os_str()
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect();
    std::fs::write(location, bytes)
}

unsafe extern "system" fn window_proc(
    window: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // The event loop in run_window keeps the App alive until the window is closed.
    unsafe {
        if msg == WM_NCCREATE {
            let create = &*(lparam as *const CREATESTRUCTW);
            let app = &*create.lpCreateParams.cast::<App>();
            app.window.set(window);
            SetWindowLongPtrW(window, GWLP_USERDATA, app as *const App as isize);
        }
        let app = (GetWindowLongPtrW(window, GWLP_USERDATA) as *const App).as_ref();
        if let Some(app) = app {
            match msg {
                WM_CREATE => match Controls::create(window) {
                    Ok(controls) => {
                        app.controls.set(Some(controls));
                        app.fonts();
                        return 0;
                    }
                    Err(error) => {
                        *app.init_error.borrow_mut() = Some(error);
                        return -1;
                    }
                },
                WM_GETMINMAXINFO => {
                    let size = &mut *(lparam as *mut MINMAXINFO);
                    size.ptMinTrackSize.x = app.unit(790);
                    size.ptMinTrackSize.y = app.unit(465);
                    return 0;
                }
                WM_SIZE if app.controls.get().is_some() && wparam != SIZE_MINIMIZED as usize => {
                    app.layout();
                    return 0;
                }
                WM_DPICHANGED if app.controls.get().is_some() => {
                    let size = &*(lparam as *const RECT);
                    SetWindowPos(
                        window,
                        null_mut(),
                        size.left,
                        size.top,
                        size.right - size.left,
                        size.bottom - size.top,
                        SWP_NOACTIVATE | SWP_NOZORDER,
                    );
                    app.fonts();
                    app.layout();
                    return 0;
                }
                WM_ERASEBKGND => return 1,
                WM_PAINT => {
                    app.paint(window);
                    return 0;
                }
                WM_PRINTCLIENT => {
                    app.paint_contents(window, wparam as HDC);
                    return 0;
                }
                WM_MEASUREITEM if lparam != 0 => {
                    let item = &mut *(lparam as *mut MEASUREITEMSTRUCT);
                    match item.CtlID as u16 {
                        BRANCH | CHANGES | LOG => {
                            item.itemHeight = app.unit(27) as u32;
                            return 1;
                        }
                        _ => {}
                    }
                }
                WM_DRAWITEM if lparam != 0 && app.controls.get().is_some() => {
                    if app.draw_item(&*(lparam as *const DRAWITEMSTRUCT)) {
                        return 1;
                    }
                }
                WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX
                    if app.controls.get().is_some() =>
                {
                    let control = lparam as HWND;
                    let hdc = wparam as HDC;
                    let controls = app.controls();
                    let enabled = IsWindowEnabled(control) != 0;
                    if msg != WM_CTLCOLORSTATIC
                        || [
                            controls.preview,
                            controls.path,
                            controls.message,
                            controls.publish_path,
                            controls.publish_url,
                        ]
                        .contains(&control)
                    {
                        return app.theme.control(hdc, true, enabled, false) as isize;
                    }
                    let in_panel = control != controls.status && control != controls.account;
                    let accent =
                        control == controls.changes_title || control == controls.preview_title;
                    return app.theme.label(hdc, in_panel, enabled, accent) as isize;
                }
                WM_COMMAND if app.controls.get().is_some() => {
                    match (wparam as u16, (wparam >> 16) as u32) {
                        (OPEN, BN_CLICKED) => app.open(),
                        (CLONE, BN_CLICKED) => app.clone_repo(),
                        (PUBLISH_BROWSE, BN_CLICKED) => app.browse_publish_folder(),
                        (PUBLISH_FOLDER, BN_CLICKED) => app.publish_folder(),
                        (CONNECT, BN_CLICKED) => app.connect(),
                        (REFRESH, BN_CLICKED) => app.refresh(),
                        (BRANCH, CBN_SELCHANGE)
                        | (COMMIT_MESSAGE | PUBLISH_PATH | PUBLISH_URL, EN_CHANGE) => {
                            app.buttons()
                        }
                        (SWITCH, BN_CLICKED) => app.branch_action(false),
                        (NEW_BRANCH, BN_CLICKED) => app.branch_action(true),
                        (STAGE, BN_CLICKED) => app.stage(true),
                        (UNSTAGE, BN_CLICKED) => app.stage(false),
                        (STAGE_ALL, BN_CLICKED) => {
                            app.command("Staging all changes…", false, Git::stage_all)
                        }
                        (WORKTREE, BN_CLICKED) => app.select_page(Page::Worktree),
                        (STAGED, BN_CLICKED) => app.select_page(Page::Staged),
                        (HISTORY, BN_CLICKED) => app.select_page(Page::History),
                        (COMMIT, BN_CLICKED) => app.commit(),
                        (AUTHOR, BN_CLICKED) => app.author(),
                        (FETCH, BN_CLICKED) => app.command("Fetching remotes…", false, Git::fetch),
                        (PULL, BN_CLICKED) => app.command("Pulling changes…", false, Git::pull),
                        (PUSH, BN_CLICKED) => app.command("Pushing commits…", false, Git::push),
                        _ => return DefWindowProcW(window, msg, wparam, lparam),
                    }
                    return 0;
                }
                WM_NOTIFY if app.controls.get().is_some() && lparam != 0 => {
                    let event = &*(lparam as *const NMHDR);
                    if event.idFrom == CHANGES as usize && event.code == LVN_ITEMCHANGED {
                        let changed = &*(lparam as *const NMLISTVIEW);
                        if (changed.uOldState ^ changed.uNewState) & LVIS_SELECTED != 0 {
                            app.change_selected();
                        }
                        return 0;
                    }
                    if event.idFrom == LOG as usize && event.code == NM_DBLCLK {
                        app.open_commit((*(lparam as *const NMITEMACTIVATE)).iItem);
                        return 0;
                    }
                }
                WM_TIMER if wparam == TIMER => {
                    app.poll_main();
                    app.poll_account();
                    app.poll_preview();
                    return 0;
                }
                WM_DESTROY => {
                    KillTimer(window, TIMER);
                    PostQuitMessage(0);
                    return 0;
                }
                _ => {}
            }
        }
        DefWindowProcW(window, msg, wparam, lparam)
    }
}

pub fn run() -> io::Result<()> {
    let executable = preferred_git().or_else(|| git::find_git().ok());
    let app = App::new(executable)?;
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let initialized = CoInitializeEx(
            null(),
            (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
        );
        if initialized < 0 {
            return Err(io::Error::other(format!(
                "Windows could not initialize the shell (0x{initialized:08X})."
            )));
        }
        let result = run_window(&app);
        CoUninitialize();
        result
    }
}

fn run_window(app: &App) -> io::Result<()> {
    unsafe {
        let info = INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_LISTVIEW_CLASSES,
        };
        if InitCommonControlsEx(&info) == 0 {
            return Err(io::Error::last_os_error());
        }
        let instance = GetModuleHandleW(null());
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            lpszClassName: CLASS,
            ..Default::default()
        };
        if RegisterClassW(&class) == 0 {
            return Err(io::Error::last_os_error());
        }
        let dpi = GetDpiForSystem().max(96) as i32;
        let window = CreateWindowExW(
            WS_EX_DLGMODALFRAME | WS_EX_CONTROLPARENT,
            CLASS,
            TITLE,
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1090 * dpi / 96,
            700 * dpi / 96,
            null_mut(),
            null_mut(),
            instance,
            (&raw const *app).cast(),
        );
        if window.is_null() {
            return Err(app
                .init_error
                .borrow_mut()
                .take()
                .unwrap_or_else(io::Error::last_os_error));
        }
        caption_theme(window);
        if SetTimer(window, TIMER, 60, None) == 0 {
            let error = io::Error::last_os_error();
            DestroyWindow(window);
            return Err(error);
        }
        app.layout();
        app.setup();
        ShowWindow(window, SW_SHOW);
        if let Some(folder) = std::env::args_os().nth(1) {
            app.open_path(PathBuf::from(folder));
        } else {
            SetFocus(app.controls().open);
        }

        let mut event = MSG::default();
        loop {
            match GetMessageW(&mut event, null_mut(), 0, 0) {
                0 => return Ok(()),
                -1 => {
                    let error = io::Error::last_os_error();
                    DestroyWindow(window);
                    return Err(error);
                }
                _ => {}
            }
            if !app.shortcut(&event) && IsDialogMessageW(window, &event) == 0 {
                TranslateMessage(&event);
                DispatchMessageW(&event);
            }
        }
    }
}

fn rounded_child(window: HWND, width: i32, height: i32, radius: i32) {
    if width <= 0 || height <= 0 {
        return;
    }
    unsafe {
        let region = CreateRoundRectRgn(0, 0, width + 1, height + 1, radius, radius);
        if !region.is_null() && SetWindowRgn(window, region, 1) == 0 {
            DeleteObject(region);
        }
    }
}

fn fill_color(hdc: HDC, area: &RECT, color: COLORREF) {
    unsafe {
        let old = SetDCBrushColor(hdc, color);
        FillRect(hdc, area, GetStockObject(DC_BRUSH) as HBRUSH);
        SetDCBrushColor(hdc, old);
    }
}

fn caption(hdc: HDC, units: &[u16], area: &mut RECT, color: COLORREF) {
    if area.right <= area.left || area.bottom <= area.top {
        return;
    }
    unsafe {
        let previous_mode = SetBkMode(hdc, TRANSPARENT as i32);
        let previous_text = SetTextColor(hdc, color);
        DrawTextW(
            hdc,
            units.as_ptr(),
            -1,
            area,
            DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX,
        );
        SetTextColor(hdc, previous_text);
        SetBkMode(hdc, previous_mode);
    }
}

fn draw_table(item: &DRAWITEMSTRUCT, font: HFONT, padding: i32) {
    let list = item.hwndItem;
    let hdc = item.hDC;
    let area = item.rcItem;
    let selected = item.itemState & ODS_SELECTED != 0;
    let fill = if selected {
        TABLE_SELECTED
    } else if item.itemID % 2 == 0 {
        theme::FIELD
    } else {
        TABLE_ALTERNATE
    };
    fill_color(hdc, &area, fill);
    let old_font = if font.is_null() {
        null_mut()
    } else {
        unsafe { SelectObject(hdc, font) }
    };
    let mut x = area.left;
    let count = if item.CtlID as u16 == LOG { 4 } else { 3 };
    for column in 0..count {
        let width = unsafe { SendMessageW(list, LVM_GETCOLUMNWIDTH, column, 0) } as i32;
        let mut buffer = [0u16; 2048];
        let mut query = LVITEMW {
            iSubItem: column as i32,
            pszText: buffer.as_mut_ptr(),
            cchTextMax: buffer.len() as i32,
            ..Default::default()
        };
        unsafe {
            SendMessageW(
                list,
                LVM_GETITEMTEXTW,
                item.itemID as usize,
                (&raw mut query) as isize,
            )
        };
        let mut cell = RECT {
            left: x + padding,
            top: area.top,
            right: x + width - padding,
            bottom: area.bottom,
        };
        let text = if column == 0 || selected {
            theme::TEXT
        } else {
            theme::MUTED
        };
        caption(hdc, &buffer, &mut cell, text);
        x += width;
    }
    if !old_font.is_null() {
        unsafe { SelectObject(hdc, old_font) };
    }
    if selected {
        let line = RECT {
            left: area.left,
            top: area.top,
            right: area.left + 2,
            bottom: area.bottom,
        };
        fill_color(hdc, &line, theme::ACCENT);
    }
    if item.itemState & ODS_FOCUS != 0 && item.itemState & ODS_NOFOCUSRECT == 0 {
        unsafe { DrawFocusRect(hdc, &area) };
    }
}

fn paint_header(window: HWND, hdc: HDC) {
    let mut area = RECT::default();
    unsafe { GetClientRect(window, &mut area) };
    fill_color(hdc, &area, theme::PANEL);
    let font = unsafe { SendMessageW(window, WM_GETFONT, 0, 0) } as HFONT;
    let current_font = if font.is_null() {
        null_mut()
    } else {
        unsafe { SelectObject(hdc, font) }
    };
    let count = unsafe { SendMessageW(window, HDM_GETITEMCOUNT, 0, 0) }.max(0) as usize;
    for column in 0..count {
        let mut rect = RECT::default();
        if unsafe { SendMessageW(window, HDM_GETITEMRECT, column, (&raw mut rect) as isize) } == 0 {
            continue;
        }
        let mut buffer = [0u16; 128];
        let mut item = HDITEMW {
            mask: HDI_TEXT,
            pszText: buffer.as_mut_ptr(),
            cchTextMax: buffer.len() as i32,
            ..Default::default()
        };
        if unsafe { SendMessageW(window, HDM_GETITEMW, column, (&raw mut item) as isize) } == 0 {
            continue;
        }
        let end = rect.right;
        rect.left += 8;
        rect.right -= 5;
        caption(hdc, &buffer, &mut rect, theme::TEXT);
        let ruler = RECT {
            left: end - 1,
            top: area.top + 4,
            right: end,
            bottom: area.bottom - 4,
        };
        fill_color(hdc, &ruler, theme::LINE);
    }
    let line = RECT {
        top: area.bottom - 1,
        ..area
    };
    fill_color(hdc, &line, theme::LINE);
    if !current_font.is_null() {
        unsafe { SelectObject(hdc, current_font) };
    }
}

unsafe extern "system" fn header_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    id: usize,
    _: usize,
) -> LRESULT {
    unsafe {
        match message {
            WM_ERASEBKGND => return 1,
            WM_PAINT => {
                let mut paint = PAINTSTRUCT::default();
                let hdc = BeginPaint(window, &mut paint);
                if !hdc.is_null() {
                    paint_header(window, hdc);
                    EndPaint(window, &paint);
                }
                return 0;
            }
            WM_PRINTCLIENT => {
                paint_header(window, wparam as HDC);
                return 0;
            }
            WM_NCDESTROY => {
                RemoveWindowSubclass(window, Some(header_proc), id);
            }
            _ => {}
        }
        DefSubclassProc(window, message, wparam, lparam)
    }
}

fn caption_theme(window: HWND) {
    let dark: i32 = 1;
    for (attribute, style) in [
        (DWMWA_USE_IMMERSIVE_DARK_MODE, dark as u32),
        (DWMWA_CAPTION_COLOR, theme::BACKGROUND),
        (DWMWA_TEXT_COLOR, theme::TEXT),
        (DWMWA_BORDER_COLOR, theme::LINE),
    ] {
        unsafe {
            DwmSetWindowAttribute(
                window,
                attribute as u32,
                (&raw const style).cast(),
                std::mem::size_of::<u32>() as u32,
            );
        }
    }
}

fn branch_caption(control: HWND, index: usize) -> Vec<u16> {
    let length = unsafe { SendMessageW(control, CB_GETLBTEXTLEN, index, 0) };
    if length < 0 {
        return wide("Branch");
    }
    let mut buffer = vec![0u16; length as usize + 1];
    unsafe { SendMessageW(control, CB_GETLBTEXT, index, buffer.as_mut_ptr() as isize) };
    buffer
}

fn paint_branch(window: HWND, hdc: HDC) {
    let mut area = RECT::default();
    unsafe {
        GetClientRect(window, &mut area);
        if area.right <= 1 || area.bottom <= 1 {
            return;
        }
        let enabled = IsWindowEnabled(window) != 0;
        let focused = GetFocus() == window;
        let pen = GetStockObject(DC_PEN);
        let brush = GetStockObject(DC_BRUSH);
        let old_pen = SelectObject(hdc, pen);
        let old_brush = SelectObject(hdc, brush);
        let bg = SetDCBrushColor(hdc, theme::FIELD);
        let color = SetDCPenColor(
            hdc,
            if focused && enabled {
                theme::ACCENT
            } else {
                theme::LINE
            },
        );
        fill_color(hdc, &area, theme::PANEL);
        let dpi = GetDpiForWindow(window).max(96) as i32;
        let scaled = |pixels| (pixels * dpi + 48) / 96;
        RoundRect(
            hdc,
            area.left,
            area.top,
            area.right,
            area.bottom,
            scaled(10),
            scaled(10),
        );

        let font = SendMessageW(window, WM_GETFONT, 0, 0) as HFONT;
        let old_font = if font.is_null() {
            null_mut()
        } else {
            SelectObject(hdc, font)
        };
        let index = SendMessageW(window, CB_GETCURSEL, 0, 0);
        let text = branch_caption(window, index as usize);
        let mut room = RECT {
            left: area.left + scaled(10),
            right: area.right - scaled(29),
            ..area
        };
        caption(
            hdc,
            &text,
            &mut room,
            if enabled { theme::TEXT } else { theme::MUTED },
        );
        if !old_font.is_null() {
            SelectObject(hdc, old_font);
        }

        let x = area.right - scaled(15);
        let y = (area.top + area.bottom) / 2;
        SetDCPenColor(hdc, if enabled { theme::ACCENT } else { theme::MUTED });
        MoveToEx(hdc, x - scaled(4), y - scaled(2), null_mut());
        LineTo(hdc, x, y + scaled(2));
        LineTo(hdc, x + scaled(5), y - scaled(3));
        SetDCBrushColor(hdc, bg);
        SetDCPenColor(hdc, color);
        SelectObject(hdc, old_brush);
        SelectObject(hdc, old_pen);
    }
}

unsafe extern "system" fn branch_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    id: usize,
    _: usize,
) -> LRESULT {
    unsafe {
        match message {
            WM_ERASEBKGND => return 1,
            WM_PAINT => {
                let mut paint = PAINTSTRUCT::default();
                let hdc = BeginPaint(window, &mut paint);
                if !hdc.is_null() {
                    paint_branch(window, hdc);
                    EndPaint(window, &paint);
                }
                return 0;
            }
            WM_PRINT | WM_PRINTCLIENT => {
                paint_branch(window, wparam as HDC);
                return 0;
            }
            WM_NCDESTROY => {
                RemoveWindowSubclass(window, Some(branch_proc), id);
            }
            CB_SETCURSEL | CB_RESETCONTENT | CB_SHOWDROPDOWN | WM_ENABLE | WM_SETFOCUS
            | WM_KILLFOCUS | WM_KEYDOWN | WM_MOUSEWHEEL | WM_LBUTTONUP => {
                let result = DefSubclassProc(window, message, wparam, lparam);
                InvalidateRect(window, null(), 0);
                return result;
            }
            _ => {}
        }
        DefSubclassProc(window, message, wparam, lparam)
    }
}
