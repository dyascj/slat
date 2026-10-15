//! Start at login via SMAppService.mainApp.

use objc2_service_management::{SMAppService, SMAppServiceStatus};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoginStatus {
    Enabled,
    RequiresApproval,
    NotFound,
    NotRegistered,
}

pub const LOGIN_ITEMS_URL: &str = "x-apple.systempreferences:com.apple.LoginItems-Settings.extension";

pub fn status() -> LoginStatus {
    // SAFETY: plain ServiceManagement calls on the main-app service.
    match unsafe { SMAppService::mainAppService().status() } {
        SMAppServiceStatus::Enabled => LoginStatus::Enabled,
        SMAppServiceStatus::RequiresApproval => LoginStatus::RequiresApproval,
        SMAppServiceStatus::NotFound => LoginStatus::NotFound,
        _ => LoginStatus::NotRegistered,
    }
}

/// Registers or unregisters the main app as a login item.
pub fn apply(enabled: bool) -> Result<(), String> {
    // SAFETY: as above.
    let res = unsafe {
        let service = SMAppService::mainAppService();
        if !enabled {
            service.unregisterAndReturnError()
        } else if service.status() != SMAppServiceStatus::Enabled {
            service.registerAndReturnError()
        } else {
            Ok(())
        }
    };
    res.map_err(|e| {
        let msg = e.localizedDescription().to_string();
        eprintln!("Slat: launch at login change failed: {msg}");
        msg
    })
}
