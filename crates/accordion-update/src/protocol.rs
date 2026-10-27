//! How the server asks the update helper to install a release, and how the
//! helper says what happened (decision 12).
//!
//! The server can't install anything itself. It downloads the release into
//! [`INBOX`] in its data folder, then writes [`REQUEST`] there last. A
//! systemd path unit starts the helper when that file appears. The helper
//! writes [`RESULT`] back when it's done. Both files are a few lines of
//! `name value`.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::Version;

/// The folder in the server's data folder that the helper reads from.
pub const INBOX: &str = "updates";
/// Written last, so the helper never sees a half-downloaded release.
pub const REQUEST: &str = "request";
/// What happened, for the server to read when it starts again.
pub const RESULT: &str = "result";

/// Where the helper finds itself, so the server can tell it's installed.
pub const HELPER: &str = "/usr/lib/accordion/accordion-update";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub version: Version,
    /// The package's file name in [`INBOX`].
    pub package: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The new version is running.
    Installed,
    /// The new version didn't work, so the old one is back, with the data
    /// from before the update.
    RolledBack,
    /// Nothing changed, such as when the release didn't check out.
    Refused,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub version: String,
    /// The version that was running before.
    pub from: String,
    pub outcome: Outcome,
    pub finished_at: SystemTime,
    /// Why it was rolled back or refused, in plain words.
    pub reason: Option<String>,
}

impl Request {
    pub fn to_text(&self) -> String {
        format!("version {}\npackage {}\n", self.version, self.package)
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let fields = Fields::parse(text);
        let version = fields
            .get("version")?
            .parse()
            .map_err(|err: crate::Error| err.to_string())?;
        let package = fields.get("package")?.to_owned();
        Ok(Self { version, package })
    }
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Installed => "installed",
            Self::RolledBack => "rolled-back",
            Self::Refused => "refused",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        [Self::Installed, Self::RolledBack, Self::Refused]
            .into_iter()
            .find(|outcome| outcome.as_str() == text)
    }
}

impl Report {
    pub fn to_text(&self) -> String {
        let millis = self
            .finished_at
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let mut text = format!(
            "version {}\nfrom {}\noutcome {}\nfinished {millis}\n",
            self.version,
            self.from,
            self.outcome.as_str()
        );
        if let Some(reason) = &self.reason {
            // One line, since each field is one.
            text.push_str(&format!("reason {}\n", one_line(reason)));
        }
        text
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let fields = Fields::parse(text);
        let outcome = fields.get("outcome")?;
        let outcome = Outcome::parse(outcome)
            .ok_or_else(|| format!("the update result says \"{outcome}\""))?;
        let millis: u64 = fields
            .get("finished")?
            .parse()
            .map_err(|_| "the update result's time isn't a number".to_owned())?;
        Ok(Self {
            version: fields.get("version")?.to_owned(),
            from: fields.get("from")?.to_owned(),
            outcome,
            finished_at: UNIX_EPOCH + Duration::from_millis(millis),
            reason: fields.get("reason").ok().map(str::to_owned),
        })
    }
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

struct Fields<'a>(Vec<(&'a str, &'a str)>);

impl<'a> Fields<'a> {
    fn parse(text: &'a str) -> Self {
        Self(
            text.lines()
                .filter_map(|line| line.split_once(' '))
                .collect(),
        )
    }

    fn get(&self, name: &str) -> Result<&'a str, String> {
        self.0
            .iter()
            .find(|(field, _)| *field == name)
            .map(|(_, value)| *value)
            .ok_or_else(|| format!("the update file has no {name}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_and_reports_read_back() {
        let request = Request {
            version: "0.3.0".parse().unwrap(),
            package: "accordion_0.3.0_arm64.deb".into(),
        };
        assert_eq!(Request::parse(&request.to_text()).unwrap(), request);

        let report = Report {
            version: "0.3.0".into(),
            from: "0.2.1".into(),
            outcome: Outcome::RolledBack,
            finished_at: UNIX_EPOCH + Duration::from_millis(1_793_035_151_937),
            reason: Some("It stopped\nwith an error.".into()),
        };
        let text = report.to_text();
        assert!(
            text.contains("reason It stopped with an error.\n"),
            "{text}"
        );
        let read = Report::parse(&text).unwrap();
        assert_eq!(read.reason.as_deref(), Some("It stopped with an error."));
        assert_eq!(read.finished_at, report.finished_at);
        assert_eq!(read.outcome, Outcome::RolledBack);

        assert!(Request::parse("version 0.3.0\n").is_err());
        assert!(Request::parse("version x\npackage p\n").is_err());
        assert!(Report::parse("outcome maybe\n").is_err());
    }
}
