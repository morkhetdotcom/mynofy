//! CLI bootstrap only. Call once from synchronous main, before any threads or I/O.
use mynofy_protocol::BoxError;
use std::ffi::OsString;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Identity {
    uid: u32,
    gid: u32,
}

fn numeric_id(value: &std::ffi::OsStr) -> Result<u32, BoxError> {
    let text = value.to_str().ok_or("UID/GID must be numeric")?;
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("UID/GID must contain decimal digits only".into());
    }
    let id = text.parse::<u32>()?;
    if id == 0 || id == u32::MAX {
        return Err("UID/GID must be nonzero and below 4294967295".into());
    }
    Ok(id)
}

fn arguments(args: Vec<OsString>) -> Result<(Option<Identity>, Vec<OsString>), BoxError> {
    let mut args = args.into_iter();
    let mut remaining = Vec::new();
    let (mut uid, mut gid) = (None, None);
    while let Some(arg) = args.next() {
        let destination = if arg == "--run-as-uid" {
            Some(&mut uid)
        } else if arg == "--run-as-gid" {
            Some(&mut gid)
        } else {
            None
        };
        if let Some(destination) = destination {
            if destination.is_some() {
                return Err("Duplicate privilege argument".into());
            }
            *destination = Some(numeric_id(&args.next().ok_or("Missing UID/GID")?)?);
        } else {
            // Preserve command option values verbatim, including values starting with --.
            let takes_value = arg
                .to_str()
                .is_some_and(|arg| arg.starts_with("--") && arg != "--help");
            remaining.push(arg);
            if takes_value && let Some(value) = args.next() {
                remaining.push(value);
            }
        }
    }
    let identity = match (uid, gid) {
        (Some(uid), Some(gid)) => Some(Identity { uid, gid }),
        (None, None) => None,
        _ => return Err("Supply both --run-as-uid and --run-as-gid".into()),
    };
    Ok((identity, remaining))
}

pub fn bootstrap(args: Vec<OsString>) -> Result<Vec<String>, BoxError> {
    let (target, args) = arguments(args)?;
    platform::drop_privileges(target)?;
    // No config, key files, socket binding, logging setup or runtime before this point.
    args.into_iter()
        .map(|arg| {
            arg.into_string()
                .map_err(|_| "Arguments must be UTF-8".into())
        })
        .collect()
}

#[cfg(target_os = "linux")]
mod platform {
    use super::{BoxError, Identity};
    use rustix::{process as p, thread as t};
    pub(super) fn drop_privileges(target: Option<Identity>) -> Result<(), BoxError> {
        let current = Identity {
            uid: p::getuid().as_raw(),
            gid: p::getgid().as_raw(),
        };
        let target = target
            .or_else(|| {
                (current.uid != 0
                    && current.gid != 0
                    && !p::geteuid().is_root()
                    && p::getegid().as_raw() != 0)
                    .then_some(current)
            })
            .ok_or("Privileged start requires --run-as-uid UID --run-as-gid GID (nonzero)")?;
        t::set_no_new_privs(true)?;
        t::clear_ambient_capability_set()?;
        if t::get_keep_capabilities()? {
            t::set_keep_capabilities(false)?;
        }
        let caps = t::capabilities(None)?;
        // Drop every kernel-supported bounding bit, including future unnamed capabilities.
        if caps.effective.contains(t::CapabilitySet::SETPCAP) {
            for bit in 0..64 {
                let cap = t::CapabilitySet::from_bits_retain(1u64 << bit);
                match t::capability_is_in_bounding_set(cap) {
                    Ok(true) => t::remove_capability_from_bounding_set(cap)?,
                    Ok(false) => (),
                    Err(rustix::io::Errno::INVAL) => (), // Not supported by this kernel.
                    Err(err) => return Err(err.into()),
                }
            }
        }
        let changing_identity =
            target != current || p::geteuid() != p::getuid() || p::getegid() != p::getgid();
        if caps.effective.contains(t::CapabilitySet::SETGID) {
            t::set_thread_groups(&[])?;
        } else if (changing_identity && !p::getgroups()?.is_empty())
            || p::getgroups()?.iter().any(|gid| gid.as_raw() == 0)
        {
            return Err("Cannot safely clear supplementary groups; start with an unprivileged UID/GID and groups".into());
        }
        let uid = p::Uid::from_raw(target.uid);
        let gid = p::Gid::from_raw(target.gid);
        // Even for an ordinary start, discard any different saved identity.
        t::set_thread_res_gid(gid, gid, gid)?;
        t::set_thread_res_uid(uid, uid, uid)?;
        let empty = t::CapabilitySets {
            effective: t::CapabilitySet::empty(),
            permitted: t::CapabilitySet::empty(),
            inheritable: t::CapabilitySet::empty(),
        };
        t::set_capabilities(None, empty)?;
        if p::getuid() != uid
            || p::geteuid() != uid
            || p::getgid() != gid
            || p::getegid() != gid
            || t::capabilities(None)? != empty
            || !t::no_new_privs()?
        {
            return Err("Privilege drop verification failed".into());
        }
        Ok(())
    }
}

#[cfg(all(unix, not(target_os = "linux"), not(target_vendor = "apple")))]
mod platform {
    use super::{BoxError, Identity};
    use nix::unistd as u;
    pub(super) fn drop_privileges(target: Option<Identity>) -> Result<(), BoxError> {
        let current = Identity {
            uid: u::getuid().as_raw(),
            gid: u::getgid().as_raw(),
        };
        let privileged = u::geteuid().is_root();
        let target = target
            .or_else(|| {
                (current.uid != 0 && current.gid != 0 && !privileged && u::getegid().as_raw() != 0)
                    .then_some(current)
            })
            .ok_or("Privileged start requires --run-as-uid UID --run-as-gid GID (nonzero)")?;
        if privileged {
            u::setgroups(&[])?;
            // Privileged POSIX setgid/setuid replace real, effective and saved IDs.
            u::setgid(u::Gid::from_raw(target.gid))?;
            u::setuid(u::Uid::from_raw(target.uid))?;
        } else if target != current
            || u::geteuid() != u::getuid()
            || u::getegid() != u::getgid()
            || u::getgroups()?.iter().any(|gid| gid.as_raw() == 0)
        {
            return Err("Start with the intended unprivileged UID/GID and groups".into());
        }
        if u::getuid().as_raw() != target.uid
            || u::geteuid().as_raw() != target.uid
            || u::getgid().as_raw() != target.gid
            || u::getegid().as_raw() != target.gid
        {
            return Err("Privilege drop verification failed".into());
        }
        Ok(())
    }
}

#[cfg(all(unix, target_vendor = "apple"))]
mod platform {
    use super::{BoxError, Identity};
    use nix::unistd as u;
    pub(super) fn drop_privileges(target: Option<Identity>) -> Result<(), BoxError> {
        let current = Identity {
            uid: u::getuid().as_raw(),
            gid: u::getgid().as_raw(),
        };
        if current.uid == 0
            || current.gid == 0
            || u::geteuid().is_root()
            || u::getegid().as_raw() == 0
        {
            return Err("Privileged start on macOS is refused; launch mynofy under the intended unprivileged account".into());
        }
        // nix deliberately excludes Apple's group APIs. Never perform a partial root drop.
        if u::getuid() != u::geteuid()
            || u::getgid() != u::getegid()
            || target.is_some_and(|target| target != current)
        {
            return Err("Launch mynofy under the intended unprivileged UID/GID on macOS".into());
        }
        Ok(())
    }
}

#[cfg(not(unix))]
mod platform {
    use super::{BoxError, Identity};
    pub(super) fn drop_privileges(target: Option<Identity>) -> Result<(), BoxError> {
        if target.is_some() {
            return Err("UID/GID privilege drop requires Unix".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parse(args: &[&str]) -> Result<(Option<Identity>, Vec<OsString>), BoxError> {
        arguments(args.iter().map(OsString::from).collect())
    }
    #[test]
    fn global_identity_flags_preserve_command_values() {
        let (id, rest) = parse(&[
            "--run-as-uid",
            "123",
            "admin",
            "clients",
            "add",
            "--name",
            "--run-as-gid",
            "--run-as-gid",
            "456",
        ])
        .unwrap();
        assert_eq!(id, Some(Identity { uid: 123, gid: 456 }));
        assert_eq!(
            rest,
            ["admin", "clients", "add", "--name", "--run-as-gid"].map(OsString::from)
        );
        assert_eq!(parse(&["help"]).unwrap().0, None);
    }
    #[test]
    fn refuses_ambiguous_or_privileged_targets() {
        for args in [
            vec!["--run-as-uid", "1"],
            vec!["--run-as-uid", "1", "--run-as-gid", "0"],
            vec!["--run-as-uid", "4294967295", "--run-as-gid", "1"],
            vec!["--run-as-uid", "+1", "--run-as-gid", "1"],
            vec!["--run-as-uid", "-1", "--run-as-gid", "1"],
            vec![
                "--run-as-uid",
                "1",
                "--run-as-uid",
                "2",
                "--run-as-gid",
                "1",
            ],
        ] {
            assert!(parse(&args).is_err(), "{args:?}");
        }
    }
}
