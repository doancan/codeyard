//! Self-managed instances the user added to an extension's `[[oauth]]` provider (#184, docs/plugin-architecture.md
//! §6.4): a GitLab or Gitea server of their own, with the OAuth application they registered there. Kept in
//! `<data>/extensions/instances.json` next to the permission switches, and like them per extension id *and* publisher
//! identity: a package with the same id from another publisher, or a sideloaded one, gets none of them.
//!
//! Only the app's own UI adds one, after a dialog that names the extension, its publisher and the host; nothing an
//! extension or a workspace can write reaches this file.
//!
//! The same file keeps the hosts the user allowed for an extension's `net-from-setting` (a server named in one of its
//! settings, such as a company GitHub Enterprise): added the same way, after the same kind of dialog, and joining the
//! extension's `net` while that permission is on. An instance is the provider's endpoints on another host
//! (`OAuthProvider::instance`), so it signs requests to that host only, under an account id of its own
//! (`gitlab@git.example.com`) that remotes on the host carry.

use std::collections::BTreeMap;
use std::path::PathBuf;

use ext_manifest::{ExtId, Manifest, OAuthProvider};
use serde::{Deserialize, Serialize};

use crate::grants::identity;
use crate::{Error, Source, Store, write_atomic};

const FORMAT: u32 = 1;

/// One instance: the `[[oauth]]` id it copies, the host it is on and the user's client id there.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Instance {
    pub provider: String,
    pub host: String,
    pub client_id: String,
}

impl Instance {
    /// What the user typed, checked: the provider offers instances, the host is a plain host name other than the
    /// provider's own, and the client id is one token.
    pub fn new(m: &Manifest, provider: &str, host: &str, client_id: &str) -> Result<Instance, String> {
        let p = m.oauth_provider(provider).ok_or_else(|| format!("{} has no account provider '{provider}'", m.id))?;
        if !p.instances {
            return Err(format!("{} does not offer self-managed instances of {}", m.id, p.name));
        }
        let host = host.trim().trim_end_matches('/').to_ascii_lowercase();
        let host = host.strip_prefix("https://").unwrap_or(&host).to_string();
        match ext_manifest::check_https_url(&format!("https://{host}/")) {
            Ok(h) if h == host => {}
            Ok(_) => return Err(format!("'{host}' must be a host name only, without a port or path")),
            Err(e) => return Err(format!("'{host}': {e}")),
        }
        if p.hosts.contains(&host) || p.home().as_deref() == Some(host.as_str()) {
            return Err(format!("{host} is already {}'s own host", p.name));
        }
        let client_id = client_id.trim();
        let ok = !client_id.is_empty()
            && client_id.len() <= ext_manifest::limits::ITEM_LEN
            && !client_id.chars().any(|c| c.is_whitespace() || c.is_control() || matches!(c, '&' | '#' | '"'));
        if !ok {
            return Err("the client id must be one word without spaces, &, # or quotes".into());
        }
        Ok(Instance { provider: provider.to_string(), host, client_id: client_id.to_string() })
    }

    /// The account id remotes on this host carry: `gitlab@git.example.com`.
    pub fn account(&self) -> String {
        ext_manifest::instance_account(&self.provider, &self.host)
    }

    /// The provider as Codeyard signs in to and signs for it; `None` when the manifest no longer offers instances.
    pub fn oauth(&self, m: &Manifest) -> Option<OAuthProvider> {
        m.oauth_provider(&self.provider)?.instance(&self.host, &self.client_id)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Record {
    publisher: String,
    #[serde(default)]
    instances: Vec<Instance>,
    /// Hosts the user allowed for `net-from-setting`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    hosts: Vec<String>,
}

/// A host the user typed for an extension's `net-from-setting`, checked: the extension declares it, and it is a plain
/// host name (an `https://` URL is reduced to its host).
pub fn user_host(m: &Manifest, typed: &str) -> Result<String, String> {
    if m.permissions.net_from_setting.is_none() {
        return Err(format!("{} does not connect to servers named in its settings", m.id));
    }
    let host = typed.trim().trim_end_matches('/').to_ascii_lowercase();
    let host = host.strip_prefix("https://").unwrap_or(&host).to_string();
    match ext_manifest::check_https_url(&format!("https://{host}/")) {
        Ok(h) if h == host => Ok(host),
        Ok(_) => Err(format!("'{host}' must be a host name only, without a port or path")),
        Err(e) => Err(format!("'{host}': {e}")),
    }
}

#[derive(Default, Serialize, Deserialize)]
struct File {
    format: u32,
    extensions: BTreeMap<String, Record>,
}

impl Store {
    /// `<data>/extensions/instances.json`.
    pub fn instances_path(&self) -> PathBuf {
        self.root.join("instances.json")
    }

    fn read_instances(&self) -> File {
        std::fs::read(self.instances_path()).ok().and_then(|b| serde_json::from_slice::<File>(&b).ok()).filter(|f| f.format == FORMAT).unwrap_or_default()
    }

    fn write_instances(&self, file: &File) -> Result<(), Error> {
        write_atomic(&self.instances_path(), &serde_json::to_vec_pretty(file).expect("instances serialize"))
    }

    /// The instances of `id` as installed from `source`; none for a package from another identity.
    pub fn instances(&self, id: &ExtId, source: &Source) -> Vec<Instance> {
        let who = identity(source);
        self.read_instances().extensions.remove(id.as_str()).filter(|r| r.publisher == who).map(|r| r.instances).unwrap_or_default()
    }

    /// The instances of every installed extension whose record matches its installed identity, by id.
    pub fn all_instances(&self) -> BTreeMap<String, Vec<Instance>> {
        let Ok((lock, _)) = self.read_lock() else { return BTreeMap::new() };
        let records = self.read_instances().extensions;
        lock.extensions
            .iter()
            .filter_map(|(id, l)| {
                let r = records.get(id.as_str())?;
                (r.publisher == identity(&l.source)).then(|| (id.to_string(), r.instances.clone()))
            })
            .collect()
    }

    /// Add `instance` to `id` from `source` (the user agreed to it), replacing one on the same host.
    pub fn add_instance(&self, id: &ExtId, source: &Source, instance: Instance) -> Result<(), Error> {
        let _guard = self.guard()?;
        let mut file = self.read_instances();
        file.format = FORMAT;
        let who = identity(source);
        let r = file.extensions.entry(id.to_string()).or_default();
        if r.publisher != who {
            *r = Record { publisher: who, ..Default::default() };
        }
        r.instances.retain(|i| !(i.provider == instance.provider && i.host == instance.host));
        r.instances.push(instance);
        self.write_instances(&file)
    }

    /// Remove the instance whose account is `account` (`gitlab@git.example.com`).
    pub fn remove_instance(&self, id: &ExtId, account: &str) -> Result<(), Error> {
        let _guard = self.guard()?;
        let mut file = self.read_instances();
        if let Some(r) = file.extensions.get_mut(id.as_str()) {
            r.instances.retain(|i| i.account() != account);
            if r.instances.is_empty() && r.hosts.is_empty() {
                file.extensions.remove(id.as_str());
            }
            self.write_instances(&file)?;
        }
        Ok(())
    }

    /// The `net-from-setting` hosts the user allowed for every installed extension whose record matches its installed
    /// identity, by id.
    pub fn all_user_hosts(&self) -> BTreeMap<String, Vec<String>> {
        let Ok((lock, _)) = self.read_lock() else { return BTreeMap::new() };
        let records = self.read_instances().extensions;
        lock.extensions
            .iter()
            .filter_map(|(id, l)| {
                let r = records.get(id.as_str()).filter(|r| !r.hosts.is_empty())?;
                (r.publisher == identity(&l.source)).then(|| (id.to_string(), r.hosts.clone()))
            })
            .collect()
    }

    /// Allow `host` (checked with [`user_host`]) for `id` from `source`: the user agreed to it.
    pub fn add_user_host(&self, id: &ExtId, source: &Source, host: &str) -> Result<(), Error> {
        let _guard = self.guard()?;
        let mut file = self.read_instances();
        file.format = FORMAT;
        let who = identity(source);
        let r = file.extensions.entry(id.to_string()).or_default();
        if r.publisher != who {
            *r = Record { publisher: who, ..Default::default() };
        }
        if !r.hosts.iter().any(|h| h == host) {
            r.hosts.push(host.to_string());
        }
        self.write_instances(&file)
    }

    /// Take back a host allowed for `net-from-setting`.
    pub fn remove_user_host(&self, id: &ExtId, host: &str) -> Result<(), Error> {
        let _guard = self.guard()?;
        let mut file = self.read_instances();
        if let Some(r) = file.extensions.get_mut(id.as_str()) {
            r.hosts.retain(|h| h != host);
            if r.instances.is_empty() && r.hosts.is_empty() {
                file.extensions.remove(id.as_str());
            }
            self.write_instances(&file)?;
        }
        Ok(())
    }

    /// Forget the instances of an uninstalled extension.
    pub(crate) fn forget_instances(&self, id: &ExtId) {
        let mut file = self.read_instances();
        if file.extensions.remove(id.as_str()).is_some() {
            let _ = self.write_instances(&file);
        }
    }
}

#[cfg(test)]
#[path = "instances_tests.rs"]
mod tests;
