//! What went wrong with an `ssh` run, read from its error output. ssh's own sentences are written for people who know ssh;
//! the app turns a [`Problem`] into what to do about it, and the host retries the ones that only need the shared
//! connection started again ([`Problem::needs_fresh_master`]).
//!
//! Matching is on stable fragments of OpenSSH's messages, case-insensitive, and on the *whole* text: ssh prints a banner
//! or warnings before the real error, and a verbose configuration prints debug lines after it.

/// The kinds of failure that have a different answer for the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Problem {
    /// The server's host key is not the one in `known_hosts`: a reinstalled server, or someone in between.
    HostKeyChanged,
    /// The host key is not known yet and no question could be asked.
    HostKeyUnknown,
    /// Every key and password offered was refused.
    AuthRefused,
    /// Too many keys offered: the server hung up before the right one came (`IdentitiesOnly` fixes it).
    TooManyAuthFailures,
    /// A private key file is readable by others, so ssh ignores it.
    KeyPermissions,
    /// A key that needs a passphrase and no agent or prompt could supply it.
    KeyPassphrase,
    /// The name did not resolve.
    UnknownHost,
    /// Nothing listens on the port, or a firewall answered with a reset.
    Refused,
    /// No answer at all.
    TimedOut,
    /// No route: wrong network, VPN down, or the interface is off.
    Unreachable,
    /// The connection was cut after it had worked.
    Dropped,
    /// The server and this ssh share no key exchange, cipher, MAC or host key algorithm.
    NoCommonAlgorithm,
    /// The jump host or `ProxyCommand` failed before the target was reached.
    Proxy,
    /// A line in the ssh configuration is wrong.
    BadConfig,
    /// The shared (multiplexed) connection's socket is stale, busy or refused: starting it again fixes it.
    StaleControl,
    /// The server refuses more sessions or channels (`MaxSessions`, `MaxStartups`).
    TooManySessions,
    /// Not one of the above.
    Other,
}

impl Problem {
    /// Whether closing the shared connection and starting it again is the cure.
    pub fn needs_fresh_master(self) -> bool {
        self == Problem::StaleControl
    }

    /// Whether trying again later can work without the user changing anything.
    pub fn transient(self) -> bool {
        matches!(self, Problem::TimedOut | Problem::Unreachable | Problem::Dropped | Problem::Refused | Problem::TooManySessions | Problem::StaleControl)
    }
}

/// Sorts ssh's error output. Specific causes are tested before general ones ("Permission denied" also ends a
/// "too many authentication failures" run, and "connection refused" can come from a proxy).
pub fn classify(stderr: &str) -> Problem {
    // OpenSSH writes both "Control socket" and "ControlSocket".
    let t = stderr.to_ascii_lowercase().replace("controlsocket", "control socket");
    let has = |s: &str| t.contains(s);
    if has("remote host identification has changed") || has("host key for") && has("has changed") {
        Problem::HostKeyChanged
    } else if has("host key verification failed") {
        Problem::HostKeyUnknown
    } else if has("unprotected private key file") || has("bad permissions") {
        Problem::KeyPermissions
    } else if has("too many authentication failures") {
        Problem::TooManyAuthFailures
    } else if has("no matching key exchange")
        || has("no matching host key type")
        || has("no matching cipher")
        || has("no matching mac")
        || has("no matching host key")
    {
        Problem::NoCommonAlgorithm
    } else if has("bad configuration option")
        || has("garbage at end of line")
        || has("bad ssh2 cipher spec")
        || has("/.ssh/config: line")
        || has("unsupported option")
    {
        Problem::BadConfig
    } else if has("control socket") && (has("connection refused") || has("no such file") || has("already exists") || has("disabling multiplexing"))
        || has("mux_client_request_session")
        || has("mux_client_hello_exchange")
        || has("mux_client_request_alive")
        || has("master is gone")
        || has("muxclient: master hello")
    {
        Problem::StaleControl
    } else if has("session open refused")
        || has("channel open failed") && has("administratively prohibited")
        || has("maxsessions")
        || has("connection closed by remote host") && has("kex_exchange")
    {
        Problem::TooManySessions
    } else if has("proxyjump") || has("proxycommand") || has("stdio forwarding failed") || has("channel 0: open failed") && has("connect failed") {
        Problem::Proxy
    } else if has("permission denied") || has("authentication failed") || has("no more authentication methods") {
        if has("passphrase") || has("incorrect passphrase") || has("sign_and_send_pubkey: signing failed") && has("agent") {
            Problem::KeyPassphrase
        } else {
            Problem::AuthRefused
        }
    } else if has("could not resolve hostname")
        || has("name or service not known")
        || has("nodename nor servname")
        || has("temporary failure in name resolution")
    {
        Problem::UnknownHost
    } else if has("connection timed out") || has("operation timed out") || has("connect timed out") || has("timed out during banner exchange") {
        Problem::TimedOut
    } else if has("connection refused") {
        Problem::Refused
    } else if has("no route to host") || has("network is unreachable") || has("host is down") || has("network is down") {
        Problem::Unreachable
    } else if has("broken pipe")
        || has("connection reset")
        || has("connection closed by")
        || has("connection to") && has("closed")
        || has("packet_write_wait")
        || has("kex_exchange_identification")
        || has("the agent closed the connection")
    {
        Problem::Dropped
    } else {
        Problem::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(s: &str) -> Problem {
        classify(s)
    }

    #[test]
    fn real_openssh_messages_are_recognised() {
        assert_eq!(
            k("@@@@@@@\n@    WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!     @\nOffending ECDSA key in /Users/a/.ssh/known_hosts:12"),
            Problem::HostKeyChanged
        );
        assert_eq!(k("Host key verification failed."), Problem::HostKeyUnknown);
        assert_eq!(k("ssh: Could not resolve hostname devbox: nodename nor servname provided, or not known"), Problem::UnknownHost);
        assert_eq!(k("ssh: connect to host 10.0.0.5 port 22: Operation timed out"), Problem::TimedOut);
        assert_eq!(k("ssh: connect to host 10.0.0.5 port 22: Connection refused"), Problem::Refused);
        assert_eq!(k("ssh: connect to host 10.0.0.5 port 22: No route to host"), Problem::Unreachable);
        assert_eq!(k("user@h: Permission denied (publickey,password)."), Problem::AuthRefused);
        assert_eq!(k("Received disconnect from 1.2.3.4 port 22:2: Too many authentication failures\nDisconnected from 1.2.3.4"), Problem::TooManyAuthFailures);
        assert_eq!(k("@         WARNING: UNPROTECTED PRIVATE KEY FILE!          @\nPermissions 0644 for 'k' are too open."), Problem::KeyPermissions);
        assert_eq!(k("Unable to negotiate with 1.2.3.4 port 22: no matching host key type found. Their offer: ssh-rsa"), Problem::NoCommonAlgorithm);
        assert_eq!(k("/Users/a/.ssh/config: line 9: Bad configuration option: foo"), Problem::BadConfig);
        assert_eq!(k("packet_write_wait: Connection to 1.2.3.4 port 22: Broken pipe"), Problem::Dropped);
        assert_eq!(k("Connection closed by 1.2.3.4 port 22"), Problem::Dropped);
        assert_eq!(k("Enter passphrase for key '/k': \nPermission denied (publickey)"), Problem::KeyPassphrase);
    }

    #[test]
    fn a_broken_shared_connection_is_told_from_a_broken_server() {
        for s in [
            "Control socket connect(/Users/a/.cache/codeyard/cm-1): Connection refused",
            "Control socket connect(/x): No such file or directory",
            "mux_client_request_session: read from master failed: Broken pipe",
            "mux_client_hello_exchange: read packet failed",
            "ControlSocket /x already exists, disabling multiplexing",
        ] {
            assert_eq!(k(s), Problem::StaleControl, "{s}");
            assert!(k(s).needs_fresh_master());
        }
        // The server refusing is not the socket's fault.
        assert!(!k("ssh: connect to host h port 22: Connection refused").needs_fresh_master());
    }

    #[test]
    fn the_cause_wins_over_the_banner_and_the_ending() {
        // A "Permission denied" ends the run of too many failures, and a proxy's refusal is the proxy's.
        assert_eq!(k("Too many authentication failures\nuser@h: Permission denied (publickey)."), Problem::TooManyAuthFailures);
        assert_eq!(k("Welcome to prod!\n\nuser@h: Permission denied (publickey)."), Problem::AuthRefused);
        assert_eq!(
            k(
                "channel 0: open failed: connect failed: Connection refused\nstdio forwarding failed\nssh_exchange_identification: Connection closed by remote host"
            ),
            Problem::Proxy
        );
        assert_eq!(k(""), Problem::Other);
        assert_eq!(k("something nobody has seen"), Problem::Other);
    }

    #[test]
    fn only_what_can_pass_by_itself_is_transient() {
        assert!(Problem::TimedOut.transient() && Problem::Dropped.transient() && Problem::StaleControl.transient());
        assert!(!Problem::AuthRefused.transient() && !Problem::HostKeyChanged.transient() && !Problem::BadConfig.transient());
    }
}
