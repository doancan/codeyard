//! Human review of a publisher's first native program (docs/registry.md, "Native programs"). An upload that needs it
//! is recorded but left out of the index, and its files wait in `review/` beside the database, out of the folder the
//! CDN serves. A reviewer lists the queue, looks at the packages, and approves (the version is listed and the
//! publisher's later native programs are not held) or rejects it (its version number stays used).

use std::path::PathBuf;

use ext_manifest::{ExtId, Version};

use crate::db::{self, Review};
use crate::{Error, Yard, internal};

/// One version waiting for a reviewer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Waiting {
    pub id: ExtId,
    pub version: Version,
    /// The publisher identity that signed it.
    pub publisher: String,
    /// Unix seconds.
    pub submitted: u64,
    /// Its packages (one per platform), to inspect before deciding: (target triple, file).
    pub packages: Vec<(String, PathBuf)>,
    /// What the install sheet says about it.
    pub permissions: Vec<String>,
}

impl Yard {
    /// The review queue, oldest first.
    pub fn review_queue(&self) -> Result<Vec<Waiting>, Error> {
        let mut conn = self.lock();
        let tx = conn.transaction().map_err(internal("database"))?;
        let mut out = Vec::new();
        for (id, version, owner, submitted) in db::pending_reviews(&tx).map_err(internal("database"))? {
            let release = db::release_of(&tx, &id, &version).map_err(internal("database"))?;
            let publisher = db::extension_publisher(&tx, &id).map_err(internal("database"))?.map_or(owner, |p| p.to_string());
            let mut packages = Vec::new();
            let mut permissions = Vec::new();
            if let Some(r) = release {
                for (t, p) in &r.targets {
                    packages.push((t.clone(), self.review_dir().join(format!("{}.cyx", p.digest))));
                }
                if let Some((_, first)) = packages.first()
                    && let Ok(pkg) = std::fs::read(first).map_err(|e| e.to_string()).and_then(|b| ext_pkg::read(&b).map_err(|e| e.to_string()))
                {
                    permissions = pkg.manifest.permissions.describe();
                }
            }
            out.push(Waiting { id, version, publisher, submitted, packages, permissions });
        }
        Ok(out)
    }

    /// List a waiting version: its files move into the public folder, the index lists it, and its publisher's later
    /// native programs are listed without review.
    pub fn approve(&self, id: &ExtId, version: &Version, now: u64) -> Result<(), Error> {
        let mut conn = self.lock();
        let tx = conn.transaction().map_err(internal("database"))?;
        let release = self.waiting(&tx, id, version)?;
        // Files first: a crash before the commit leaves them public but unlisted, and approving again finishes it.
        for p in release.targets.values() {
            for ext in ["cyx", "sigstore.json", "details.json"] {
                let (from, to) = (self.review_dir().join(format!("{}.{ext}", p.digest)), self.packages().join(format!("{}.{ext}", p.digest)));
                if from.exists() {
                    std::fs::rename(&from, &to).map_err(|e| Error::Internal(format!("cannot move {} into the public folder: {e}", from.display())))?;
                }
            }
        }
        let owner = crate::names::owner_key(&db::extension_publisher(&tx, id).map_err(internal("database"))?.ok_or_else(|| missing(id, version))?);
        db::set_review(&tx, id, version, Review::Approved, "").map_err(internal("database"))?;
        db::set_owner_reviewed(&tx, &owner, now).map_err(internal("database"))?;
        self.sign_index(&tx, now)?;
        tx.commit().map_err(internal("database"))
    }

    /// Refuse a waiting version: it is never listed, its files are deleted, and its version number stays used. The
    /// reason is kept for the publisher.
    pub fn reject(&self, id: &ExtId, version: &Version, reason: &str) -> Result<(), Error> {
        if reason.trim().is_empty() || reason.len() > 500 || reason.chars().any(char::is_control) {
            return Err(Error::Invalid("give a reason of at most 500 characters on one line; the publisher sees it".into()));
        }
        let mut conn = self.lock();
        let tx = conn.transaction().map_err(internal("database"))?;
        let release = self.waiting(&tx, id, version)?;
        db::set_review(&tx, id, version, Review::Rejected, reason.trim()).map_err(internal("database"))?;
        tx.commit().map_err(internal("database"))?;
        for p in release.targets.values() {
            for ext in ["cyx", "sigstore.json", "details.json"] {
                let _ = std::fs::remove_file(self.review_dir().join(format!("{}.{ext}", p.digest)));
            }
        }
        Ok(())
    }

    fn waiting(&self, tx: &rusqlite::Transaction, id: &ExtId, version: &Version) -> Result<ext_core::yard::Release, Error> {
        match db::review(tx, id, version).map_err(internal("database"))? {
            Some(Review::Pending) => db::release_of(tx, id, version).map_err(internal("database"))?.ok_or_else(|| missing(id, version)),
            Some(Review::Approved) => Err(Error::Conflict(format!("{id} {version} was approved already"))),
            Some(Review::Rejected) => Err(Error::Conflict(format!("{id} {version} was rejected already"))),
            None => Err(missing(id, version)),
        }
    }
}

fn missing(id: &ExtId, version: &Version) -> Error {
    Error::NotFound(format!("{id} {version} is not waiting for review"))
}
