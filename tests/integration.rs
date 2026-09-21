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
fn clean_survives_the_character_sets_pacs_actually_use() {
    // DICOM's Simplified Chinese sets are GB18030 / GBK / ISO 2022 IR 58. (IR 149 is
    // Korean, not Chinese - easy to get backwards.) Each must survive a real
    // encode/decode round trip, not just an in-memory one.
    for cs in ["ISO_IR 192", "GB18030", "GBK", "ISO 2022 IR 58"] {
        let src = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        common::write_fixture_extra(
            &src.path().join("one.dcm"),
            "程英平",
            "1.2.3",
            &[
                (
                    dicom_dictionary_std::tags::SPECIFIC_CHARACTER_SET,
                    VR::CS,
                    cs,
                ),
                (Tag(0x0008, 0x0080), VR::LO, "万楠医学院附属医院"),
                (Tag(0x0008, 0x1030), VR::LO, "万楠医学院附属医院^腰椎"),
                (Tag(0x0008, 0x103E), VR::LO, "局部定位"),
            ],
        );

        let cfg_dir = tempfile::tempdir().unwrap();
        let cfg_path = cfg_dir.path().join("anon.toml");
        std::fs::write(
            &cfg_path,
            "[output]\nfilename = \"preserve\"\n[profile]\nclean_descriptors = true\n",
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

        let o = dicom_object::open_file(out.path().join("one.dcm"))
            .unwrap_or_else(|e| panic!("{cs}: output not readable: {e}"));
        let get = |t: Tag| {
            o.element(t)
                .ok()
                .and_then(|e| e.to_str().ok().map(|s| s.to_string()))
        };
        // the identifier is gone, the clinical Chinese text is intact
        assert_eq!(get(Tag(0x0008, 0x1030)).as_deref(), Some("腰椎"), "{cs}");
        assert_eq!(
            get(Tag(0x0008, 0x103E)).as_deref(),
            Some("局部定位"),
            "{cs}"
        );
        // direct identifiers are cleared as usual
        assert_eq!(get(Tag(0x0010, 0x0010)).as_deref(), Some(""), "{cs}");
        assert_eq!(get(Tag(0x0008, 0x0080)).as_deref(), Some(""), "{cs}");
        // and the file stays readable because the character set was kept
        assert_eq!(
            get(dicom_dictionary_std::tags::SPECIFIC_CHARACTER_SET).as_deref(),
            Some(cs),
            "{cs}"
        );
    }
}

#[test]
fn an_unsupported_character_set_neither_panics_nor_leaks() {
    // A file may declare a character set this build cannot handle (the crate
    // supports ISO-IR 148/159, for instance, only as unimplemented). Whatever the
    // tool does with such a file, it must not panic and must not retain identity.
    let src = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    common::write_fixture_extra(
        &src.path().join("one.dcm"),
        "Doe^John",
        "1.2.3",
        &[
            (
                dicom_dictionary_std::tags::SPECIFIC_CHARACTER_SET,
                VR::CS,
                "ISO 2022 IR 148",
            ),
            (Tag(0x0008, 0x1030), VR::LO, "Doe^John l-spine"),
        ],
    );

    let cfg_dir = tempfile::tempdir().unwrap();
    let cfg_path = cfg_dir.path().join("anon.toml");
    std::fs::write(
        &cfg_path,
        "[output]\nfilename = \"preserve\"\n[profile]\nclean_descriptors = true\n",
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

    // Observed: the crate falls back to a default codec rather than failing, so the
    // file is processed normally - and the identifier must still be gone.
    let audit = std::fs::read_to_string(out.path().join("audit.jsonl")).unwrap();
    assert!(audit.contains("\"status\":\"ok\""), "{audit}");

    let o = dicom_object::open_file(out.path().join("one.dcm")).unwrap();
    let study = o
        .element(Tag(0x0008, 0x1030))
        .ok()
        .and_then(|e| e.to_str().ok().map(|s| s.to_string()))
        .unwrap_or_default();
    assert_eq!(
        study, "l-spine",
        "identifier survived an unsupported character set"
    );
}

#[test]
fn non_ascii_text_under_an_unsupported_character_set_is_still_cleaned() {
    // The gap noted in the spec: an unimplemented charset combined with actual
    // non-ASCII bytes. The fallback must not produce an unreadable file or let an
    // accented name through.
    let src = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    common::write_fixture_extra(
        &src.path().join("one.dcm"),
        "Müller^Hans",
        "1.2.3",
        &[
            (
                dicom_dictionary_std::tags::SPECIFIC_CHARACTER_SET,
                VR::CS,
                "ISO 2022 IR 148",
            ),
            (Tag(0x0008, 0x0080), VR::LO, "Klinik München"),
            (Tag(0x0008, 0x1030), VR::LO, "Klinik München^l-spine"),
        ],
    );

    let cfg_dir = tempfile::tempdir().unwrap();
    let cfg_path = cfg_dir.path().join("anon.toml");
    std::fs::write(
        &cfg_path,
        "[output]\nfilename = \"preserve\"\n[profile]\nclean_descriptors = true\n",
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

    let audit = std::fs::read_to_string(out.path().join("audit.jsonl")).unwrap();
    assert!(audit.contains("\"status\":\"ok\""), "{audit}");

    let o = dicom_object::open_file(out.path().join("one.dcm"))
        .expect("output must stay readable under an unsupported character set");
    let get = |t: Tag| {
        o.element(t)
            .ok()
            .and_then(|e| e.to_str().ok().map(|s| s.to_string()))
    };
    // "Müller" and "München" are harvested despite the accented bytes.
    assert_eq!(get(Tag(0x0008, 0x1030)).as_deref(), Some("l-spine"));
    assert_eq!(get(Tag(0x0010, 0x0010)).as_deref(), Some(""));
    assert_eq!(get(Tag(0x0008, 0x0080)).as_deref(), Some(""));
}

#[test]
fn exported_fingerprint_verifies_a_tree_without_the_originals() {
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
    let dir = tempfile::tempdir().unwrap();
    let fp = dir.path().join("ids.json");

    Command::cargo_bin("dicom-anonymizer")
        .unwrap()
        .args(["-i"])
        .arg(src.path())
        .args(["-o"])
        .arg(out.path())
        .args(["--export-identifiers"])
        .arg(&fp)
        .assert()
        .success();
    assert!(fp.exists());

    // The artefact is a verification aid, not a new PHI file.
    let json = std::fs::read_to_string(&fp).unwrap();
    assert!(!json.contains("Doe"), "{json}");
    assert!(!json.contains("John"), "{json}");
    assert!(!json.contains("YiJiShan"), "{json}");

    // The anonymised tree verifies clean, needing only the fingerprint.
    Command::cargo_bin("dicom-anonymizer")
        .unwrap()
        .args(["-i"])
        .arg(out.path())
        .args(["--verify"])
        .arg(&fp)
        .assert()
        .success()
        .stderr(predicate::str::contains("verify: 0 hit(s)"));

    // The un-anonymised source is caught, so the check discriminates.
    Command::cargo_bin("dicom-anonymizer")
        .unwrap()
        .args(["-i"])
        .arg(src.path())
        .args(["--verify"])
        .arg(&fp)
        .assert()
        .failure()
        .stderr(predicate::str::contains("leak:"));
}

#[test]
fn verify_rejects_a_file_that_is_not_a_fingerprint() {
    let src = tempfile::tempdir().unwrap();
    common::write_fixture(&src.path().join("one.dcm"), "Doe^John", "1.2.3");
    let dir = tempfile::tempdir().unwrap();
    let bad = dir.path().join("bad.json");
    std::fs::write(&bad, "this is not a fingerprint").unwrap();

    Command::cargo_bin("dicom-anonymizer")
        .unwrap()
        .args(["-i"])
        .arg(src.path())
        .args(["--verify"])
        .arg(&bad)
        .assert()
        .failure()
        .stderr(predicate::str::contains("is not a fingerprint"));
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
