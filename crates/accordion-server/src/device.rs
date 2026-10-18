//! Names a browser for the device list, such as "Firefox on Windows".
//!
//! User-Agent strings are messy, and any client can send whatever it likes,
//! so this is only a label to help users recognize their own devices. Never
//! use it for security.

pub fn describe_browser(user_agent: Option<&str>) -> String {
    let ua = user_agent.unwrap_or_default();

    // Order matters: Edge and the iOS browsers also claim to be Chrome or
    // Safari, and Chrome also claims to be Safari.
    let browser = if ua.contains("Edg/") || ua.contains("EdgiOS/") {
        "Edge"
    } else if ua.contains("Firefox/") || ua.contains("FxiOS/") {
        "Firefox"
    } else if ua.contains("Chrome/") || ua.contains("CriOS/") {
        "Chrome"
    } else if ua.contains("Safari/") {
        "Safari"
    } else {
        "Web browser"
    };

    // iPhones claim to be "like Mac OS X", and Android claims to be Linux.
    let os = if ua.contains("iPhone") {
        Some("iPhone")
    } else if ua.contains("iPad") {
        Some("iPad")
    } else if ua.contains("Android") {
        Some("Android")
    } else if ua.contains("Windows") {
        Some("Windows")
    } else if ua.contains("Macintosh") {
        Some("macOS")
    } else if ua.contains("CrOS") {
        Some("ChromeOS")
    } else if ua.contains("Linux") {
        Some("Linux")
    } else {
        None
    };

    match os {
        Some(os) => format!("{browser} on {os}"),
        None => browser.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_common_browsers() {
        for (ua, expected) in [
            (
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:140.0) Gecko/20100101 Firefox/140.0",
                "Firefox on Windows",
            ),
            (
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) \
                 Chrome/140.0.0.0 Safari/537.36 Edg/140.0.0.0",
                "Edge on Windows",
            ),
            (
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 \
                 (KHTML, like Gecko) Version/26.0 Safari/605.1.15",
                "Safari on macOS",
            ),
            (
                "Mozilla/5.0 (iPhone; CPU iPhone OS 26_0 like Mac OS X) AppleWebKit/605.1.15 \
                 (KHTML, like Gecko) CriOS/140.0.0.0 Mobile/15E148 Safari/604.1",
                "Chrome on iPhone",
            ),
            (
                "Mozilla/5.0 (Linux; Android 16; Pixel 9) AppleWebKit/537.36 (KHTML, like Gecko) \
                 Chrome/140.0.0.0 Mobile Safari/537.36",
                "Chrome on Android",
            ),
            (
                "Mozilla/5.0 (X11; Linux x86_64; rv:140.0) Gecko/20100101 Firefox/140.0",
                "Firefox on Linux",
            ),
            ("curl/8.14.1", "Web browser"),
        ] {
            assert_eq!(describe_browser(Some(ua)), expected);
        }
        assert_eq!(describe_browser(None), "Web browser");
    }
}
