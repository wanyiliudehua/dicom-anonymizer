mod common;

use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn end_to_end_mirrors_tree_and_deidentifies() {
    let src = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    common::write_fixture(&src.path().join("a/one.dcm"), "Doe^John", "1.2.3");
    common::write_fixture(&src.path().join("b/two.dcm"), "Roe^Jane", "1.2.3");
    std::fs::write(src.path().join("b/notes.txt"), b"ignore me").unwrap();

    Command::cargo_bin("dicom-anonymizer")
        .unwrap()
        .args(["-i"])
        .arg(src.path())
        .args(["-o"])
        .arg(out.path())
        .assert()
        .success();

    assert!(out.path().join("a/one.dcm").exists());
    assert!(out.path().join("b/two.dcm").exists());
    assert!(!out.path().join("b/notes.txt").exists());
    assert!(out.path().join("audit.jsonl").exists());

    let o1 = dicom_object::open_file(out.path().join("a/one.dcm")).unwrap();
    let n1 = o1
        .element(dicom_dictionary_std::tags::PATIENT_NAME)
        .ok()
        .and_then(|e| e.to_str().ok().map(|s| s.to_string()));
    assert_ne!(n1.as_deref(), Some("Doe^John"));

    // Same original study UID in both files must map to the same new UID.
    let u1 = o1
        .element(dicom_dictionary_std::tags::STUDY_INSTANCE_UID)
        .ok()
        .and_then(|e| e.to_str().ok().map(|s| s.to_string()));
    let o2 = dicom_object::open_file(out.path().join("b/two.dcm")).unwrap();
    let u2 = o2
        .element(dicom_dictionary_std::tags::STUDY_INSTANCE_UID)
        .ok()
        .and_then(|e| e.to_str().ok().map(|s| s.to_string()));
    assert_eq!(u1, u2);
    assert!(u1.unwrap().starts_with("2.25."));
}

#[test]
fn dry_run_writes_no_files() {
    let src = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    common::write_fixture(&src.path().join("one.dcm"), "Doe^John", "1.2.3");

    Command::cargo_bin("dicom-anonymizer")
        .unwrap()
        .args(["-i"])
        .arg(src.path())
        .args(["-o"])
        .arg(out.path())
        .arg("--dry-run")
        .assert()
        .success()
        .stdout(predicate::str::contains("\"status\":\"ok\""));

    assert!(std::fs::read_dir(out.path()).unwrap().next().is_none());
}

#[test]
fn empty_input_is_success() {
    let src = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    Command::cargo_bin("dicom-anonymizer")
        .unwrap()
        .args(["-i"])
        .arg(src.path())
        .args(["-o"])
        .arg(out.path())
        .assert()
        .success();
}
