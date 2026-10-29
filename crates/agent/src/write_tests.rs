use super::*;
use std::sync::Barrier;

#[test]
fn saving_a_hard_link_preserves_its_other_name() {
    let root = std::env::temp_dir().join(format!("codeyard-save-hard-link-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let target = root.join("document.txt");
    let alias = root.join("alias.txt");
    fs::write(&target, b"original").unwrap();
    fs::hard_link(&target, &alias).unwrap();
    assert_eq!(write(&target, b"saved version", Some(content_hash(b"original"))), Ok(content_hash(b"saved version")));
    assert_eq!(fs::read(&alias).unwrap(), b"saved version", "saving must update the linked file, rather than replace one of its names");
    fs::write(&alias, b"still linked").unwrap();
    assert_eq!(fs::read(&target).unwrap(), b"still linked", "both names must continue to address the same file");
}

#[test]
fn concurrent_saves_through_hard_link_names_share_the_same_version_guard() {
    let root = std::env::temp_dir().join(format!("codeyard-save-hard-link-race-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let target = root.join("document.txt");
    let alias = root.join("alias.txt");
    fs::write(&target, vec![0; 1024 * 1024]).unwrap();
    fs::hard_link(&target, &alias).unwrap();
    let expected = content_hash(&vec![0; 1024 * 1024]);
    let barrier = Arc::new(Barrier::new(16));
    let jobs: Vec<_> = (1..=16)
        .map(|index| {
            let barrier = barrier.clone();
            let path = if index % 2 == 0 { target.clone() } else { alias.clone() };
            std::thread::spawn(move || {
                let data = vec![index; 1024 * 1024];
                barrier.wait();
                (write(&path, &data, Some(expected)), data)
            })
        })
        .collect();
    let results: Vec<_> = jobs.into_iter().map(|job| job.join().unwrap()).collect();
    let winners: Vec<_> = results.iter().filter(|(result, _)| result.is_ok()).collect();
    assert_eq!(winners.len(), 1, "linked names must share one version guard");
    assert_eq!(results.iter().filter(|(result, _)| matches!(result, Err(Error::Conflict { .. }))).count(), 15);
    assert_eq!(fs::read(&target).unwrap(), winners[0].1);
    assert_eq!(fs::read(&alias).unwrap(), winners[0].1);
}

#[test]
fn concurrent_saves_from_the_same_version_report_one_success_and_conflicts() {
    let root = std::env::temp_dir().join(format!("codeyard-save-version-race-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let path = root.join("document.txt");
    let original = vec![0; 1024 * 1024];
    fs::write(&path, &original).unwrap();
    let expected = content_hash(&original);
    let barrier = Arc::new(Barrier::new(16));
    let jobs: Vec<_> = (1..=16)
        .map(|index| {
            let barrier = barrier.clone();
            let path = path.clone();
            std::thread::spawn(move || {
                let data = vec![index; 1024 * 1024];
                barrier.wait();
                let result = write(&path, &data, Some(expected));
                (result, data)
            })
        })
        .collect();
    let results: Vec<_> = jobs.into_iter().map(|job| job.join().unwrap()).collect();
    let winners: Vec<_> = results.iter().filter(|(result, _)| result.is_ok()).collect();
    assert_eq!(winners.len(), 1, "only one save may accept the original disk version");
    assert_eq!(results.iter().filter(|(result, _)| matches!(result, Err(Error::Conflict { .. }))).count(), 15);
    assert_eq!(fs::read(&path).unwrap(), winners[0].1);
    assert_eq!(fs::read_dir(&root).unwrap().count(), 1, "failed saves must not leave staging files");
}
