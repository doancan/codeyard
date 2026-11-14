//! Real ssh: the shared connection holds many forwards at once. A GUI app on macOS starts with 256 open files and every
//! `localhost` forward is two listeners, so before ssh was started with a higher limit the 100-odd forward failed with
//! "Port forwarding failed" (the 99+ notifications). Run it under `ulimit -S -n 256` (the soft limit only: a lowered hard limit cannot be raised) to see that case; it fails at the 125th forward without the raised limit.

use host::forward::{self, LocalBind, OpenRequest, RemoteTarget};

/// Enough that two listeners each exceed 256 descriptors (3 + master socket + connections included).
const FORWARDS: u16 = 160;

#[cfg(unix)]
#[test]
fn a_shared_connection_holds_far_more_forwards_than_256_descriptors_allow() {
    let Some(_sshd) = crate::test_sshd::start("fdlimit") else { return };
    let alias = crate::test_sshd::ALIAS;
    // Starts the shared connection the way the app does (ControlMaster=auto, ControlPersist).
    host::ssh::probe(alias, &host::ssh::AgentBundle::discover()).expect("the test sshd does not answer");
    let mut opened = Vec::new();
    let mut first_failure = None;
    for i in 0..FORWARDS {
        let req = OpenRequest {
            alias: alias.into(),
            remote: 40_000 + i,
            target: RemoteTarget::Localhost,
            wanted_local: None,
            strict: false,
            bind: LocalBind::Loopback,
        };
        match forward::open(&req) {
            Ok(o) => opened.push(o),
            Err(e) => {
                first_failure = Some((i, e));
                break;
            }
        }
    }
    for o in &opened {
        let _ = forward::cancel(alias, &o.spec);
    }
    host::ssh::close_master(alias);
    assert_eq!(first_failure, None, "{} of {FORWARDS} forwards opened before the first failure", opened.len());
}
