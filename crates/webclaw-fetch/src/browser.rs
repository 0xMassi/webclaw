//! Browser fingerprint selection and rotation.
//! Maps our BrowserProfile enum to webclaw-http client builder methods.

/// Which browser identity to present at the TLS/HTTP layer.
#[derive(Debug, Clone, Default)]
pub enum BrowserProfile {
    #[default]
    Chrome,
    Firefox,
    /// Safari iOS 26 (iPhone). The one profile proven to defeat
    /// DataDome's immobiliare.it / idealista.it / target.com-class
    /// rules when paired with a country-scoped residential proxy.
    SafariIos,
    /// Randomly pick from all available profiles on each request.
    Random,
}

/// A browser variant for building webclaw-http clients.
#[derive(Debug, Clone, Copy)]
pub enum BrowserVariant {
    Chrome,
    ChromeMacos,
    Firefox,
    Safari,
    SafariIos26,
    Edge,
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
