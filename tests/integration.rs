mod common;

use assert_cmd::Command;
use dicom_core::{Tag, VR};
use predicates::prelude::*;

#[test]
fn end_to_end_mirrors_tree_and_deidentifies() {
    let src = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    common::write_fixture(&src.path().join("a/one.dcm"), "Doe^John", "1.2.3");
    common::write_fixture(&src.path().join("b/two.dcm"), "Roe^Jane", "1.2.3");
    std::fs::write(src.path().join("b/notes.txt"), b"ignore me").unwrap();

    let cfg_dir = tempfile::tempdir().unwrap();
    let cfg_path = cfg_dir.path().join("anon.toml");
    std::fs::write(&cfg_path, "[output]\nfilename = \"preserve\"\n").unwrap();

    Command::cargo_bin("dicom-anonymizer")
        .unwrap()
        .args(["-i"])
        .arg(src.path())
        .args(["-o"])
        .arg(out.path())
        .args(["--config"])
        .arg(&cfg_path)
        .assert()
        .success();

    assert!(out.path().join("a/one.dcm").exists());
    assert!(out.path().join("b/two.dcm").exists());
    assert!(!out.path().join("b/notes.txt").exists());
    assert!(out.path().join("audit.jsonl").exists());

    // the non-DICOM file is logged as skipped (spec §11)
    let audit = std::fs::read_to_string(out.path().join("audit.jsonl")).unwrap();
    assert!(
        audit.contains("\"status\":\"skipped\""),
        "missing skipped record"
    );
    assert!(
        audit.contains("notes.txt"),
        "skipped record should name the file"
    );

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
fn leak_check_passes_on_a_clean_run() {
    let src = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    common::write_fixture_extra(
        &src.path().join("one.dcm"),
        "Doe^John",
        "1.2.3",
        &[(
            Tag(0x0008, 0x0080),
            VR::LO,
            "WanNan Medical College YiJiShan Hospital",
        )],
    );

    Command::cargo_bin("dicom-anonymizer")
        .unwrap()
        .args(["-i"])
        .arg(src.path())
        .args(["-o"])
        .arg(out.path())
        .arg("--check-leaks")
        .assert()
        .success()
        .stderr(predicate::str::contains("leak check: 0 hit(s)"));
}

#[test]
fn leak_check_fails_when_a_retained_attribute_carries_phi() {
    let src = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    // A private block the profile is told to retain, carrying the patient name.
    common::write_fixture_extra(
        &src.path().join("one.dcm"),
        "Doe^John",
        "1.2.3",
        &[
            (Tag(0x0009, 0x0010), VR::LO, "ACME 1.0"),
            (Tag(0x0009, 0x1001), VR::LO, "Doe^John"),
        ],
    );

    let cfg_dir = tempfile::tempdir().unwrap();
    let cfg_path = cfg_dir.path().join("anon.toml");
    std::fs::write(&cfg_path, "[profile]\nretain_safe_private = true\n").unwrap();

    Command::cargo_bin("dicom-anonymizer")
        .unwrap()
        .args(["-i"])
        .arg(src.path())
        .args(["-o"])
        .arg(out.path())
        .args(["--config"])
        .arg(&cfg_path)
        .arg("--check-leaks")
        .assert()
        .failure()
        .stderr(predicate::str::contains("leak:"));
}

#[test]
fn leak_check_is_skipped_in_dry_run() {
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
        .arg("--check-leaks")
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "--check-leaks needs an output tree",
        ));
}

#[test]
fn series_layout_groups_outputs_by_series() {
    let src = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    for (rel, name, sop) in [
        ("a/one.dcm", "Doe^John", "1.2.3.1"),
        ("b/two.dcm", "Roe^Jane", "1.2.3.2"),
    ] {
        common::write_fixture_extra(
            &src.path().join(rel),
            name,
            sop,
            &[(
                dicom_dictionary_std::tags::SERIES_INSTANCE_UID,
                VR::UI,
                "1.2.840.9",
            )],
        );
    }

    let cfg_dir = tempfile::tempdir().unwrap();
    let cfg_path = cfg_dir.path().join("anon.toml");
    std::fs::write(
        &cfg_path,
        "[output]\nfilename = \"preserve\"\nlayout = \"series\"\n",
    )
    .unwrap();

    Command::cargo_bin("dicom-anonymizer")
        .unwrap()
        .args(["-i"])
        .arg(src.path())
        .args(["-o"])
        .arg(out.path())
        .args(["--config"])
        .arg(&cfg_path)
        .assert()
        .success();

    // Both files share a series, so they land flattened in one directory.
    let dirs: Vec<_> = std::fs::read_dir(out.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .collect();
    assert_eq!(dirs.len(), 1, "expected exactly one series directory");
    let series_dir = dirs[0].path();
    assert!(series_dir.join("one.dcm").exists());
    assert!(series_dir.join("two.dcm").exists());
    // the input directory structure is not mirrored
    assert!(!out.path().join("a").exists());
    assert!(!out.path().join("b").exists());

    // and the directory is named after the *new* (remapped) series UID
    let o = dicom_object::open_file(series_dir.join("one.dcm")).unwrap();
    let series = o
        .element(dicom_dictionary_std::tags::SERIES_INSTANCE_UID)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert_ne!(series, "1.2.840.9", "series UID must be remapped");
    assert_eq!(
        series_dir.file_name().unwrap().to_string_lossy(),
        series,
        "the directory name must be the remapped series UID"
    );
}

#[test]
fn series_layout_falls_back_to_mirror_without_a_series_uid() {
    let src = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    common::write_fixture(&src.path().join("a/one.dcm"), "Doe^John", "1.2.3");

    let cfg_dir = tempfile::tempdir().unwrap();
    let cfg_path = cfg_dir.path().join("anon.toml");
    std::fs::write(
        &cfg_path,
        "[output]\nfilename = \"preserve\"\nlayout = \"series\"\n[uids]\nstrategy = \"zero\"\n",
    )
    .unwrap();

    Command::cargo_bin("dicom-anonymizer")
        .unwrap()
        .args(["-i"])
        .arg(src.path())
        .args(["-o"])
        .arg(out.path())
        .args(["--config"])
        .arg(&cfg_path)
        .assert()
        .success();

    // With no series UID to group by, the mirrored path is kept (no data loss).
    assert!(out.path().join("a/one.dcm").exists());
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

#[test]
fn default_output_names_use_new_sop_instance_uid() {
    let src = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    common::write_fixture(&src.path().join("a/one.dcm"), "Doe^John", "1.2.3");

    Command::cargo_bin("dicom-anonymizer")
        .unwrap()
        .args(["-i"])
        .arg(src.path())
        .args(["-o"])
        .arg(out.path())
        .assert()
        .success();

    // original filename is gone; directory is preserved
    assert!(!out.path().join("a/one.dcm").exists());
    let entries: Vec<_> = std::fs::read_dir(out.path().join("a"))
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(entries.len(), 1);
    let name = entries[0].file_name().to_string_lossy().to_string();
    assert!(
        name.starts_with("2.25.") && name.ends_with(".dcm"),
        "unexpected name {name}"
    );

    let o = dicom_object::open_file(out.path().join("a").join(&name)).unwrap();
    let sop = o
        .element(dicom_dictionary_std::tags::SOP_INSTANCE_UID)
        .ok()
        .and_then(|e| e.to_str().ok().map(|s| s.to_string()))
        .unwrap();
    assert_eq!(format!("{sop}.dcm"), name);
    assert_eq!(o.meta().media_storage_sop_instance_uid(), sop);
}
