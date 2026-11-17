//! Browser fingerprint selection and rotation.
//! Browser identities used by the wreq emulation builder in `tls`.

/// Which browser identity to present at the TLS/HTTP layer.
#[derive(Debug, Clone, Default)]
pub enum BrowserProfile {
    #[default]
    Chrome,
    /// Chrome on macOS.
    ChromeMacos,
    Firefox,
    /// Safari on macOS.
    Safari,
    Edge,
    /// Safari iOS 26 (iPhone). The one profile proven to defeat
    /// DataDome's immobiliare.it / idealista.it / target.com-class
    /// rules when paired with a country-scoped residential proxy.
    SafariIos,
    /// Chrome 149 on Windows: the ALPN Chrome sends over TCP and client
    /// hints that match its User-Agent.
    Chrome149,
    /// Chrome 149 on macOS.
    ChromeMacos149,
    /// Edge 149 on Windows.
    Edge149,
    /// Firefox 151 on Windows.
    Firefox151,
    /// Safari 26.4 on macOS.
    Safari26,
    /// Randomly pick from all available profiles on each request.
    Random,
}

/// A browser variant for building wreq clients.
#[derive(Debug, Clone, Copy)]
pub enum BrowserVariant {
    Chrome,
    ChromeMacos,
    Firefox,
    Safari,
    SafariIos26,
    Edge,
    Chrome149,
    ChromeMacos149,
    Edge149,
    Firefox151,
    Safari26,
}

/// All Chrome variants we ship.
pub fn chrome_variants() -> Vec<BrowserVariant> {
    vec![BrowserVariant::Chrome, BrowserVariant::ChromeMacos]
}

/// All Firefox variants we ship.
pub fn firefox_variants() -> Vec<BrowserVariant> {
    vec![BrowserVariant::Firefox]
}

/// All variants for maximum diversity in Random mode.
pub fn all_variants() -> Vec<BrowserVariant> {
    vec![
        BrowserVariant::Chrome,
        BrowserVariant::ChromeMacos,
        BrowserVariant::Firefox,
        BrowserVariant::Safari,
        BrowserVariant::Edge,
    ]
}

pub fn latest_chrome() -> BrowserVariant {
    BrowserVariant::Chrome
}

pub fn latest_firefox() -> BrowserVariant {
    BrowserVariant::Firefox
}
