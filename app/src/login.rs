//! Start at login via SMAppService.mainApp.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoginStatus {
    Enabled,
    RequiresApproval,
    NotFound,
    NotRegistered,
}

pub const LOGIN_ITEMS_URL: &str = "x-apple.systempreferences:com.apple.LoginItems-Settings.extension";

pub fn status() -> LoginStatus {
    todo!("menubar agent implements")
}

/// Registers or unregisters the main app as a login item.
pub fn apply(_enabled: bool) -> Result<(), String> {
    todo!("menubar agent implements")
}
