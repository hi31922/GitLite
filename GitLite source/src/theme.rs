use std::io;
use windows_sys::Win32::Foundation::{COLORREF, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    CreateRoundRectRgn, CreateSolidBrush, DRAW_TEXT_FORMAT, DT_CENTER, DT_END_ELLIPSIS, DT_LEFT,
    DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, DeleteObject, DrawTextW, FillRect, FillRgn, FrameRect,
    FrameRgn, HBRUSH, HDC, OPAQUE, RestoreDC, SaveDC, SelectObject, SetBkColor, SetBkMode,
    SetTextColor, TRANSPARENT,
};
use windows_sys::Win32::UI::Controls::{
    DRAWITEMSTRUCT, ODS_COMBOBOXEDIT, ODS_DISABLED, ODS_FOCUS, ODS_GRAYED, ODS_HOTLIGHT,
    ODS_NOFOCUSRECT, ODS_SELECTED,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{SendMessageW, WM_GETFONT};

const fn rgb(red: u8, green: u8, blue: u8) -> COLORREF {
    red as COLORREF | (green as COLORREF) << 8 | (blue as COLORREF) << 16
}

pub const BACKGROUND: COLORREF = rgb(0x11, 0x1C, 0x2C);
pub const PANEL: COLORREF = rgb(0x20, 0x37, 0x4E);
pub const FIELD: COLORREF = rgb(0x15, 0x24, 0x38);
pub const LINE: COLORREF = rgb(0x39, 0x52, 0x68);
pub const TEXT: COLORREF = rgb(0xEA, 0xF5, 0xF8);
pub const MUTED: COLORREF = rgb(0xA4, 0xBA, 0xC8);
pub const ACCENT: COLORREF = rgb(0x55, 0xDC, 0xA0);

const HOVER: COLORREF = rgb(0x29, 0x45, 0x5F);
const PRESSED: COLORREF = rgb(0x17, 0x2A, 0x3D);
const ACTIVE: COLORREF = rgb(0x24, 0x4A, 0x48);
const DISABLED: COLORREF = rgb(0x1C, 0x2E, 0x41);
const PRIMARY_PRESSED: COLORREF = rgb(0x3D, 0xBF, 0x85);
const PRIMARY_TEXT: COLORREF = rgb(0x10, 0x28, 0x20);
const FOCUS_ON_ACCENT: COLORREF = rgb(0xD2, 0xFB, 0xE4);

#[derive(Clone, Copy)]
pub enum Tone {
    Regular,
    Primary,
    Active,
}

struct Brush(HBRUSH);

impl Brush {
    fn new(color: COLORREF) -> io::Result<Self> {
        let handle = unsafe { CreateSolidBrush(color) };
        if handle.is_null() {
            Err(io::Error::other(
                "Windows could not allocate a theme brush.",
            ))
        } else {
            Ok(Self(handle))
        }
    }
}

impl Drop for Brush {
    fn drop(&mut self) {
        unsafe { DeleteObject(self.0) };
    }
}

pub struct Theme {
    background: Brush,
    panel: Brush,
    field: Brush,
    line: Brush,
    accent: Brush,
    hover: Brush,
    pressed: Brush,
    active: Brush,
    disabled: Brush,
    primary_pressed: Brush,
    focus_on_accent: Brush,
}

impl Theme {
    pub fn new() -> io::Result<Self> {
        Ok(Self {
            background: Brush::new(BACKGROUND)?,
            panel: Brush::new(PANEL)?,
            field: Brush::new(FIELD)?,
            line: Brush::new(LINE)?,
            accent: Brush::new(ACCENT)?,
            hover: Brush::new(HOVER)?,
            pressed: Brush::new(PRESSED)?,
            active: Brush::new(ACTIVE)?,
            disabled: Brush::new(DISABLED)?,
            primary_pressed: Brush::new(PRIMARY_PRESSED)?,
            focus_on_accent: Brush::new(FOCUS_ON_ACCENT)?,
        })
    }

    pub fn paint(&self, hdc: HDC, bounds: &RECT) {
        unsafe { FillRect(hdc, bounds, self.background.0) };
    }

    pub fn card(&self, hdc: HDC, bounds: &RECT, radius: i32) {
        rounded(hdc, bounds, radius, self.panel.0, self.line.0, 1);
    }

    pub fn control(&self, hdc: HDC, field: bool, enabled: bool, accent: bool) -> HBRUSH {
        let (background, brush) = if field {
            (FIELD, self.field.0)
        } else {
            (PANEL, self.panel.0)
        };
        unsafe {
            SetTextColor(hdc, label_color(enabled, accent));
            SetBkColor(hdc, background);
            SetBkMode(hdc, OPAQUE as i32);
        }
        brush
    }

    pub fn label(&self, hdc: HDC, on_panel: bool, enabled: bool, accent: bool) -> HBRUSH {
        let (background, brush) = self.surface(on_panel);
        unsafe {
            SetTextColor(hdc, label_color(enabled, accent));
            SetBkColor(hdc, background);
            SetBkMode(hdc, TRANSPARENT as i32);
        }
        brush
    }

    pub fn button(&self, item: &DRAWITEMSTRUCT, label: &[u16], tone: Tone, radius: i32) {
        self.button_on(item, label, tone, radius, true);
    }

    pub fn button_on(
        &self,
        item: &DRAWITEMSTRUCT,
        label: &[u16],
        tone: Tone,
        radius: i32,
        on_panel: bool,
    ) {
        let state = item.itemState;
        let enabled = state & (ODS_DISABLED | ODS_GRAYED) == 0;
        let pressed = state & ODS_SELECTED != 0;
        let hovered = state & ODS_HOTLIGHT != 0;
        let focused = enabled && show_focus(state);
        let border_width = if focused { 2 } else { 1 };
        let (_, surface) = self.surface(on_panel);

        let (fill, line, text) = if !enabled {
            (self.disabled.0, self.line.0, MUTED)
        } else {
            match tone {
                Tone::Primary => (
                    if pressed {
                        self.primary_pressed.0
                    } else {
                        self.accent.0
                    },
                    if focused {
                        self.focus_on_accent.0
                    } else {
                        self.accent.0
                    },
                    PRIMARY_TEXT,
                ),
                Tone::Active => (
                    if pressed {
                        self.pressed.0
                    } else {
                        self.active.0
                    },
                    self.accent.0,
                    ACCENT,
                ),
                Tone::Regular => (
                    if pressed {
                        self.pressed.0
                    } else if hovered {
                        self.hover.0
                    } else {
                        self.panel.0
                    },
                    if focused { self.accent.0 } else { self.line.0 },
                    TEXT,
                ),
            }
        };

        unsafe { FillRect(item.hDC, &item.rcItem, surface) };
        rounded(item.hDC, &item.rcItem, radius, fill, line, border_width);
        draw_label(item, label, text, DT_CENTER, 7);
    }

    pub fn row(&self, item: &DRAWITEMSTRUCT, label: &[u16], inset: i32) {
        let state = item.itemState;
        let enabled = state & (ODS_DISABLED | ODS_GRAYED) == 0;
        let selected = enabled && state & ODS_SELECTED != 0 && state & ODS_COMBOBOXEDIT == 0;
        let fill = if selected {
            self.active.0
        } else {
            self.field.0
        };
        let text = if selected {
            ACCENT
        } else {
            label_color(enabled, false)
        };
        unsafe {
            FillRect(item.hDC, &item.rcItem, fill);
            if enabled && show_focus(state) {
                FrameRect(item.hDC, &item.rcItem, self.accent.0);
            }
        }
        draw_label(item, label, text, DT_LEFT, inset.max(0));
    }

    fn surface(&self, on_panel: bool) -> (COLORREF, HBRUSH) {
        if on_panel {
            (PANEL, self.panel.0)
        } else {
            (BACKGROUND, self.background.0)
        }
    }
}

fn label_color(enabled: bool, accent: bool) -> COLORREF {
    if !enabled {
        MUTED
    } else if accent {
        ACCENT
    } else {
        TEXT
    }
}

fn show_focus(state: u32) -> bool {
    state & ODS_FOCUS != 0 && state & ODS_NOFOCUSRECT == 0
}

fn rounded(hdc: HDC, bounds: &RECT, radius: i32, fill: HBRUSH, line: HBRUSH, border_width: i32) {
    let width = bounds.right.saturating_sub(bounds.left);
    let height = bounds.bottom.saturating_sub(bounds.top);
    if width < 1 || height < 1 {
        return;
    }
    let diameter = radius.max(0).min(width / 2).min(height / 2) * 2;
    unsafe {
        if diameter > 0 {
            let region = CreateRoundRectRgn(
                bounds.left,
                bounds.top,
                bounds.right,
                bounds.bottom,
                diameter,
                diameter,
            );
            if !region.is_null() {
                FillRgn(hdc, region, fill);
                FrameRgn(hdc, region, line, border_width, border_width);
                DeleteObject(region);
                return;
            }
        }
        FillRect(hdc, bounds, fill);
        FrameRect(hdc, bounds, line);
    }
}

fn draw_label(
    item: &DRAWITEMSTRUCT,
    label: &[u16],
    color: COLORREF,
    alignment: DRAW_TEXT_FORMAT,
    inset: i32,
) {
    let length = label
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(label.len());
    let mut bounds = item.rcItem;
    bounds.left = bounds.left.saturating_add(inset);
    bounds.right = bounds.right.saturating_sub(inset);
    if length == 0 || bounds.left >= bounds.right {
        return;
    }
    unsafe {
        let dc_state = SaveDC(item.hDC);
        if dc_state == 0 {
            return;
        }
        let font = SendMessageW(item.hwndItem, WM_GETFONT, 0, 0) as *mut std::ffi::c_void;
        if !font.is_null() {
            SelectObject(item.hDC, font);
        }
        SetTextColor(item.hDC, color);
        SetBkMode(item.hDC, TRANSPARENT as i32);
        DrawTextW(
            item.hDC,
            label.as_ptr(),
            length.min(i32::MAX as usize) as i32,
            &mut bounds,
            alignment | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX,
        );
        RestoreDC(item.hDC, dc_state);
    }
}
