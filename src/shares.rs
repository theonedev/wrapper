//! JSW 3.5.51 Windows share settings. Mappings belong to the supervisor, not a JVM.
use crate::config::{Config, unquote};
use anyhow::{Result, bail, ensure};
use std::{collections::BTreeMap, time::Duration};

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::Network;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FailurePolicy {
    Continue,
    Shutdown,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ResourceType {
    Any,
    Disk,
    Printer,
}

// Do not derive Debug: this structure holds credentials.
pub struct Share {
    pub index: u32,
    pub location: String,
    pub target: String,
    pub resource_type: ResourceType,
    pub account: Option<String>,
    pub password: Option<String>,
    failure: FailurePolicy,
    premapped: FailurePolicy,
    attempts: u64,
    interval: Duration,
    unmap: bool,
    force: bool,
}

pub fn parse(config: &Config) -> Result<Vec<Share>> {
    let mut entries = BTreeMap::<u32, BTreeMap<&str, &str>>::new();
    for (key, value) in &config.values {
        let Some(rest) = key.strip_prefix("wrapper.share.") else {
            continue;
        };
        let Some((number, property)) = rest.split_once('.') else {
            bail!("Expected wrapper.share.<number>.<property>");
        };
        let index: u32 = number
            .parse()
            .ok()
            .filter(|n| *n > 0)
            .ok_or_else(|| anyhow::anyhow!("Share number must be a positive integer"))?;
        ensure!(
            number == index.to_string(),
            "Share numbers must not have leading zeros"
        );
        ensure!(
            matches!(
                property,
                "location"
                    | "target"
                    | "type"
                    | "account"
                    | "password"
                    | "startup.failure"
                    | "startup.premapped"
                    | "startup.max_retries"
                    | "startup.retry_interval"
                    | "shutdown.unmap"
                    | "shutdown.unmap.force"
            ),
            "Unsupported share property: {key}"
        );
        ensure!(!value.contains('\0'), "NUL character in {key}");
        entries.entry(index).or_default().insert(property, value);
    }
    let mut shares = Vec::<Share>::new();
    for (index, values) in entries {
        let get = |name, default| unquote(values.get(name).copied().unwrap_or(default));
        let policy = |name| -> Result<FailurePolicy> {
            match get(name, "CONTINUE").to_ascii_uppercase().as_str() {
                "CONTINUE" => Ok(FailurePolicy::Continue),
                "SHUTDOWN" => Ok(FailurePolicy::Shutdown),
                _ => bail!("wrapper.share.{index}.{name} must be CONTINUE or SHUTDOWN"),
            }
        };
        let boolean = |name| -> Result<bool> {
            match get(name, "FALSE").to_ascii_uppercase().as_str() {
                "TRUE" => Ok(true),
                "FALSE" => Ok(false),
                _ => bail!("wrapper.share.{index}.{name} must be TRUE or FALSE"),
            }
        };
        let number = |name, default| -> Result<u64> {
            get(name, default).parse().map_err(|_| {
                anyhow::anyhow!("wrapper.share.{index}.{name} must be a nonnegative integer")
            })
        };
        let location = get("location", "");
        let mut parts = location.strip_prefix(r"\\").unwrap_or("").split('\\');
        ensure!(
            parts.next().is_some_and(|s| !s.is_empty())
                && parts.next().is_some_and(|s| !s.is_empty())
                && !location.ends_with('\\'),
            "wrapper.share.{index}.location must be a UNC share path without a trailing backslash"
        );
        let mut target = get("target", "").to_ascii_uppercase();
        let disk = target.len() == 2
            && target.as_bytes()[0].is_ascii_alphabetic()
            && target.ends_with(':');
        let printer = target
            .strip_prefix("LPT")
            .map(|s| s.strip_suffix(':').unwrap_or(s))
            .is_some_and(|s| s.len() == 1 && matches!(s.as_bytes()[0], b'1'..=b'9'));
        ensure!(
            disk || printer,
            "wrapper.share.{index}.target must be a drive (Z:) or printer port (LPT1)"
        );
        if printer {
            target = target.trim_end_matches(':').to_owned();
        }
        ensure!(
            !shares.iter().any(|s| s.target == target),
            "Duplicate share target in wrapper.share.{index}.target"
        );
        let resource_type = match get("type", "ANY").to_ascii_uppercase().as_str() {
            "ANY" => ResourceType::Any,
            "DISK" if disk => ResourceType::Disk,
            "PRINTER" if printer => ResourceType::Printer,
            _ => bail!(
                "wrapper.share.{index}.type must be ANY, DISK or PRINTER and match the target"
            ),
        };
        shares.push(Share {
            index,
            location,
            target,
            resource_type,
            account: values
                .get("account")
                .map(|v| unquote(v))
                .filter(|v| !v.is_empty()),
            password: values.get("password").map(|v| unquote(v)),
            failure: policy("startup.failure")?,
            premapped: policy("startup.premapped")?,
            // JSW documents this as the total number of mapping attempts.
            attempts: number("startup.max_retries", "5")?.max(1),
            interval: Duration::from_secs(number("startup.retry_interval", "10")?),
            unmap: boolean("shutdown.unmap")?,
            force: boolean("shutdown.unmap.force")?,
        });
    }
    Ok(shares)
}

#[derive(Debug)]
pub enum MappingError {
    Premapped,
    Temporary(u32),
    Permanent(u32),
}

pub trait Backend {
    // Must not replace existing mappings, even if they point to the same share.
    fn map(&mut self, share: &Share) -> std::result::Result<(), MappingError>;
    // Must not remove a mapping that now points somewhere else.
    fn unmap(&mut self, share: &Share) -> std::result::Result<(), u32>;
}

pub struct Mappings<B: Backend, F: Fn(&str)> {
    backend: B,
    owned: Vec<Share>,
    report: F,
}

impl<B: Backend, F: Fn(&str)> Mappings<B, F> {
    pub fn new(backend: B, report: F) -> Self {
        Self {
            backend,
            owned: Vec::new(),
            report,
        }
    }

    pub fn set_reporter(&mut self, report: F) {
        self.report = report;
    }

    // Returning false means startup was cancelled. The caller must then drop us.
    // The wait callback also services stop/status requests, including before mapping.
    pub fn connect(
        &mut self,
        shares: Vec<Share>,
        mut cancelled: impl FnMut(Duration) -> Result<bool>,
    ) -> Result<bool> {
        for share in shares {
            let mut attempt = 0;
            loop {
                if cancelled(Duration::ZERO)? {
                    return Ok(false);
                }
                attempt += 1;
                match self.backend.map(&share) {
                    Ok(()) => {
                        (self.report)(&format!(
                            "Mapped wrapper.share.{} to {}",
                            share.index, share.target
                        ));
                        self.owned.push(share);
                        break;
                    }
                    Err(MappingError::Temporary(code)) if attempt < share.attempts => {
                        (self.report)(&format!(
                            "Mapping wrapper.share.{} failed (Windows error {code}); retrying",
                            share.index
                        ));
                        if cancelled(share.interval)? {
                            return Ok(false);
                        }
                    }
                    Err(error) => {
                        let (policy, reason) = match error {
                            MappingError::Premapped => {
                                (share.premapped, "target already exists".into())
                            }
                            MappingError::Temporary(code) | MappingError::Permanent(code) => {
                                (share.failure, format!("Windows error {code}"))
                            }
                        };
                        let message = format!("Cannot map wrapper.share.{}: {reason}", share.index);
                        if policy == FailurePolicy::Shutdown {
                            bail!(message);
                        }
                        (self.report)(&format!(
                            "{message}; continuing without changing the target"
                        ));
                        break;
                    }
                }
            }
        }
        Ok(true)
    }
}

impl<B: Backend, F: Fn(&str)> Drop for Mappings<B, F> {
    fn drop(&mut self) {
        for share in self.owned.iter().rev().filter(|s| s.unmap) {
            if let Err(code) = self.backend.unmap(share) {
                (self.report)(&format!(
                    "Cannot unmap wrapper.share.{} (Windows error {code})",
                    share.index
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, collections::VecDeque, rc::Rc};

    fn config(properties: &str) -> Config {
        Config {
            file: "wrapper.conf".into(),
            boot: "boot".into(),
            environment: Default::default(),
            values: properties
                .lines()
                .filter_map(|s| s.split_once('='))
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        }
    }
    const BASE: &str = "wrapper.share.1.location=\\\\server\\data\nwrapper.share.1.target=Z:\n";
    fn shares(extra: &str) -> Vec<Share> {
        parse(&config(&format!("{BASE}{extra}"))).unwrap()
    }

    #[derive(Default)]
    struct Calls {
        results: VecDeque<std::result::Result<(), MappingError>>,
        mapped: Vec<u32>,
        unmapped: Vec<(u32, bool)>,
        unmap_error: Option<u32>,
    }
    struct Fake(Rc<RefCell<Calls>>);
    impl Backend for Fake {
        fn map(&mut self, share: &Share) -> std::result::Result<(), MappingError> {
            let mut calls = self.0.borrow_mut();
            calls.mapped.push(share.index);
            calls.results.pop_front().unwrap_or(Ok(()))
        }
        fn unmap(&mut self, share: &Share) -> std::result::Result<(), u32> {
            let mut calls = self.0.borrow_mut();
            calls.unmapped.push((share.index, share.force));
            calls.unmap_error.map_or(Ok(()), Err)
        }
    }

    #[test]
    fn parses_old_defaults_and_numbered_gaps_without_disclosing_passwords() {
        let shares = shares(
            "wrapper.share.10.location=\\\\server\\printer\nwrapper.share.10.target=lpt1:\nwrapper.share.10.type=PRINTER\nwrapper.share.10.account=DOMAIN\\user\nwrapper.share.10.password=secret\n",
        );
        assert_eq!(shares.iter().map(|s| s.index).collect::<Vec<_>>(), [1, 10]);
        let first = &shares[0];
        assert_eq!(first.location, r"\\server\data");
        assert_eq!(first.resource_type, ResourceType::Any);
        assert_eq!(first.failure, FailurePolicy::Continue);
        assert_eq!(first.premapped, FailurePolicy::Continue);
        assert_eq!(first.attempts, 5);
        assert_eq!(first.interval, Duration::from_secs(10));
        assert!(!first.unmap && !first.force);
        assert!(first.account.is_none() && first.password.is_none());
        assert_eq!(shares[1].target, "LPT1");
        assert_eq!(shares[1].resource_type, ResourceType::Printer);
        assert_eq!(shares[1].account.as_deref(), Some(r"DOMAIN\user"));
        assert_eq!(shares[1].password.as_deref(), Some("secret"));
        assert_eq!(
            super::parse(&config(&format!("{BASE}wrapper.share.1.type=DISK\n"))).unwrap()[0]
                .resource_type,
            ResourceType::Disk
        );
    }

    #[test]
    fn rejects_incomplete_invalid_and_unsupported_settings() {
        for extra in [
            "wrapper.share.1.location=not-a-unc-path",
            "wrapper.share.1.location=\\\\server\\",
            "wrapper.share.1.target=",
            "wrapper.share.1.target=LPT1::",
            "wrapper.share.1.type=PRINTER",
            "wrapper.share.1.startup.failure=BACKGROUND",
            "wrapper.share.1.background.retry_interval=60",
            "wrapper.share.1.shutdown.unmap=secret",
            "wrapper.share.1.startup.max_retries=-1",
            "wrapper.share.1.password=secret\0suffix",
            "wrapper.share.01.location=\\\\server\\data",
            "wrapper.share.0.location=\\\\server\\data",
            "wrapper.share.2.location=\\\\server\\other\nwrapper.share.2.target=z:",
            "wrapper.share.2.password=secret",
        ] {
            let error = parse(&config(&format!("{BASE}{extra}")))
                .err()
                .expect(extra)
                .to_string();
            assert!(!error.contains("secret"), "{error}");
        }
    }

    #[test]
    fn retries_network_errors_but_not_credentials_and_honors_failure_policy() {
        let calls = Rc::new(RefCell::new(Calls::default()));
        calls.borrow_mut().results.extend([
            Err(MappingError::Temporary(53)),
            Err(MappingError::Temporary(53)),
            Ok(()),
        ]);
        let mut mappings = Mappings::new(Fake(calls.clone()), |_| {});
        let mut waits = Vec::new();
        assert!(mappings.connect(shares("wrapper.share.1.startup.max_retries=3\nwrapper.share.1.startup.retry_interval=2\n"), |d| { if !d.is_zero() { waits.push(d); } Ok(false) }).unwrap());
        assert_eq!(waits, [Duration::from_secs(2); 2]);
        assert_eq!(calls.borrow().mapped.len(), 3);
        drop(mappings);
        assert!(calls.borrow().unmapped.is_empty()); // JSW's default leaves mappings intact.

        for policy in ["CONTINUE", "SHUTDOWN"] {
            let calls = Rc::new(RefCell::new(Calls::default()));
            calls
                .borrow_mut()
                .results
                .push_back(Err(MappingError::Permanent(1326)));
            let mut mappings = Mappings::new(Fake(calls.clone()), |_| {});
            let result = mappings.connect(
                shares(&format!("wrapper.share.1.startup.failure={policy}\n")),
                |d| {
                    assert!(d.is_zero());
                    Ok(false)
                },
            );
            assert_eq!(result.is_err(), policy == "SHUTDOWN");
            assert_eq!(calls.borrow().mapped.len(), 1);
            drop(mappings);
            assert!(calls.borrow().unmapped.is_empty());
        }
    }

    #[test]
    fn exhaustion_applies_failure_policy_and_zero_still_attempts_once() {
        for (attempts, expected) in [(0, 1), (2, 2)] {
            let calls = Rc::new(RefCell::new(Calls::default()));
            calls
                .borrow_mut()
                .results
                .extend((0..expected).map(|_| Err(MappingError::Temporary(53))));
            let mut mappings = Mappings::new(Fake(calls.clone()), |_| {});
            assert!(mappings.connect(shares(&format!("wrapper.share.1.startup.max_retries={attempts}\nwrapper.share.1.startup.failure=SHUTDOWN\n")), |_| Ok(false)).is_err());
            assert_eq!(calls.borrow().mapped.len(), expected);
        }
    }

    #[test]
    fn premapped_policy_is_independent_and_never_owns_existing_drives() {
        for (premapped, failure) in [("CONTINUE", "SHUTDOWN"), ("SHUTDOWN", "CONTINUE")] {
            let calls = Rc::new(RefCell::new(Calls::default()));
            calls
                .borrow_mut()
                .results
                .push_back(Err(MappingError::Premapped));
            let mut mappings = Mappings::new(Fake(calls.clone()), |_| {});
            let result = mappings.connect(shares(&format!("wrapper.share.1.startup.premapped={premapped}\nwrapper.share.1.startup.failure={failure}\nwrapper.share.1.shutdown.unmap=TRUE\nwrapper.share.1.shutdown.unmap.force=TRUE\n")), |_| Ok(false));
            assert_eq!(result.is_err(), premapped == "SHUTDOWN");
            drop(mappings);
            assert!(calls.borrow().unmapped.is_empty());
        }
    }

    #[test]
    fn cancellation_and_partial_failure_clean_up_owned_mappings() {
        for cancel in [false, true] {
            let calls = Rc::new(RefCell::new(Calls::default()));
            calls
                .borrow_mut()
                .results
                .extend([Ok(()), Err(MappingError::Temporary(53))]);
            let mut mappings = Mappings::new(Fake(calls.clone()), |_| {});
            let result = mappings.connect(shares("wrapper.share.1.shutdown.unmap=TRUE\nwrapper.share.2.location=\\\\server\\other\nwrapper.share.2.target=Y:\nwrapper.share.2.startup.max_retries=1\nwrapper.share.2.startup.failure=SHUTDOWN\n"), |_| Ok(cancel && calls.borrow().mapped.len() == 1));
            if cancel {
                assert!(!result.unwrap());
            } else {
                assert!(result.is_err());
            }
            drop(mappings);
            assert_eq!(calls.borrow().unmapped, [(1, false)]);
        }
        let calls = Rc::new(RefCell::new(Calls::default()));
        calls
            .borrow_mut()
            .results
            .push_back(Err(MappingError::Temporary(53)));
        let mut mappings = Mappings::new(Fake(calls.clone()), |_| {});
        assert!(
            !mappings
                .connect(shares(""), |delay| Ok(!delay.is_zero()))
                .unwrap()
        );
        assert_eq!(calls.borrow().mapped.len(), 1);
    }

    #[test]
    fn shutdown_unmaps_in_reverse_order_and_reports_all_errors() {
        let calls = Rc::new(RefCell::new(Calls::default()));
        calls.borrow_mut().unmap_error = Some(2401);
        let logs = Rc::new(RefCell::new(Vec::new()));
        let reporter = || {
            let logs = logs.clone();
            move |s: &str| logs.borrow_mut().push(s.to_owned())
        };
        let mut mappings = Mappings::new(Fake(calls.clone()), reporter());
        assert!(mappings.connect(shares("wrapper.share.1.shutdown.unmap=TRUE\nwrapper.share.1.shutdown.unmap.force=TRUE\nwrapper.share.2.location=\\\\server\\other\nwrapper.share.2.target=Y:\nwrapper.share.2.shutdown.unmap=TRUE\n"), |_| Ok(false)).unwrap());
        assert!(calls.borrow().unmapped.is_empty()); // Retained until the supervisor exits.
        mappings.set_reporter(reporter());
        drop(mappings);
        assert_eq!(calls.borrow().unmapped, [(2, false), (1, true)]);
        assert_eq!(
            logs.borrow().iter().filter(|s| s.contains("2401")).count(),
            2
        );
    }
}
