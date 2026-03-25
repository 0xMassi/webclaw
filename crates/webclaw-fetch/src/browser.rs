/// Browser fingerprint selection and rotation.
/// Maps our simple `BrowserProfile` enum to primp's impersonation profiles.
use primp::{Impersonate, ImpersonateOS};

/// Which browser identity to present at the TLS/HTTP layer.
#[derive(Debug, Clone, Default)]
pub enum BrowserProfile {
    #[default]
    Chrome,
    Firefox,
    /// Randomly pick from all available profiles on each request.
    Random,
}

/// A complete impersonation profile: browser + OS.
#[derive(Debug, Clone)]
pub struct ImpersonateProfile {
    pub browser: Impersonate,
    pub os: ImpersonateOS,
}

/// All Chrome profiles we ship, newest first.
pub fn chrome_profiles() -> Vec<ImpersonateProfile> {
    vec![
        ImpersonateProfile {
            browser: Impersonate::ChromeV145,
            os: ImpersonateOS::Windows,
        },
        ImpersonateProfile {
            browser: Impersonate::ChromeV145,
            os: ImpersonateOS::MacOS,
        },
        ImpersonateProfile {
            browser: Impersonate::ChromeV144,
            os: ImpersonateOS::Windows,
        },
        ImpersonateProfile {
            browser: Impersonate::ChromeV144,
            os: ImpersonateOS::Linux,
        },
    ]
}

/// All Firefox profiles we ship, newest first.
pub fn firefox_profiles() -> Vec<ImpersonateProfile> {
    vec![
        ImpersonateProfile {
            browser: Impersonate::FirefoxV146,
            os: ImpersonateOS::Windows,
        },
        ImpersonateProfile {
            browser: Impersonate::FirefoxV146,
            os: ImpersonateOS::Linux,
        },
        ImpersonateProfile {
            browser: Impersonate::FirefoxV140,
            os: ImpersonateOS::Windows,
        },
    ]
}

/// Safari + Edge + Opera profiles for maximum diversity in Random mode.
pub fn extra_profiles() -> Vec<ImpersonateProfile> {
    vec![
        ImpersonateProfile {
            browser: Impersonate::SafariV18_5,
            os: ImpersonateOS::MacOS,
        },
        ImpersonateProfile {
            browser: Impersonate::SafariV26,
            os: ImpersonateOS::MacOS,
        },
        ImpersonateProfile {
            browser: Impersonate::EdgeV145,
            os: ImpersonateOS::Windows,
        },
        ImpersonateProfile {
            browser: Impersonate::OperaV127,
            os: ImpersonateOS::Windows,
        },
    ]
}

pub fn latest_chrome() -> ImpersonateProfile {
    ImpersonateProfile {
        browser: Impersonate::SafariV26,
        os: ImpersonateOS::MacOS,
    }
}

pub fn latest_firefox() -> ImpersonateProfile {
    ImpersonateProfile {
        browser: Impersonate::FirefoxV146,
        os: ImpersonateOS::Windows,
    }
}
