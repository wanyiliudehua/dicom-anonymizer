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
fn text_without_a_declared_character_set_round_trips_and_is_still_cleaned() {
    // A file carrying UTF-8 text while declaring no SpecificCharacterSet is not
    // conformant, yet such files exist in the wild. Reading maps those bytes to
    // Latin-1-ish characters, which the same codec can write back, so the bytes
    // survive untouched - and cleaning runs on that same decoding, so it still
    // matches. (Constructing such a value in memory instead would be rejected by
    // the writer, which is what made this look like a hard failure at first.)
    let src = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    common::write_raw_cjk_fixture(&src.path().join("one.dcm"));

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

    let written = std::fs::read(out.path().join("one.dcm")).unwrap();
    let contains = |needle: &[u8]| written.windows(needle.len()).any(|w| w == needle);
    // The patient name's bytes are gone outright.
    assert!(
        !contains("程英平".as_bytes()),
        "patient name bytes survived"
    );
    // The institution's bytes are gone from both the attribute and the
    // descriptor that repeated them.
    assert!(
        !contains("万楠医学院附属医院".as_bytes()),
        "institution bytes survived"
    );
    // Clinical text is preserved byte for byte.
    assert!(contains("腰椎".as_bytes()), "clinical text was lost");
}

#[test]
fn a_finished_run_marks_the_tree_as_complete() {
    let src = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    common::write_fixture(&src.path().join("one.dcm"), "Doe^John", "1.2.3");
    std::fs::write(src.path().join("notes.txt"), b"not dicom").unwrap();

    Command::cargo_bin("dicom-anonymizer")
        .unwrap()
        .args(["-i"])
        .arg(src.path())
        .args(["-o"])
        .arg(out.path())
        .assert()
        .success();

    let marker = std::fs::read_to_string(out.path().join("run.json")).unwrap();
    assert!(marker.contains("\"outcome\": \"complete\""), "{marker}");
    assert!(marker.contains("\"total\": 2"), "{marker}");
    assert!(marker.contains("\"skipped\": 1"), "{marker}");
    assert!(marker.contains("\"audit_complete\": true"), "{marker}");
}

#[test]
fn an_unwritable_file_marks_the_tree_as_partial() {
    let src = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    common::write_fixture(&src.path().join("good.dcm"), "Doe^John", "1.2.3");
    common::write_broken_dicom(&src.path().join("bad.dcm"));

    Command::cargo_bin("dicom-anonymizer")
        .unwrap()
        .args(["-i"])
        .arg(src.path())
        .args(["-o"])
        .arg(out.path())
        .assert()
        .failure();

    let marker = std::fs::read_to_string(out.path().join("run.json")).unwrap();
    assert!(
        marker.contains("\"outcome\": \"partial\""),
        "a run that left a file un-anonymised must not claim to be complete: {marker}"
    );
    assert!(marker.contains("\"error\": 1"), "{marker}");
}

#[test]
fn verify_reports_the_run_manifest() {
    let src = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    common::write_fixture(&src.path().join("one.dcm"), "Doe^John", "1.2.3");
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

    Command::cargo_bin("dicom-anonymizer")
        .unwrap()
        .args(["-i"])
        .arg(out.path())
        .args(["--verify"])
        .arg(&fp)
        .assert()
        .success()
        .stderr(predicate::str::contains("manifest: outcome=Complete"));

    // A tree without the marker says so instead of pretending.
    Command::cargo_bin("dicom-anonymizer")
        .unwrap()
        .args(["-i"])
        .arg(src.path())
        .args(["--verify"])
        .arg(&fp)
        .assert()
        .stderr(predicate::str::contains("manifest: none"));
}

#[test]
fn an_input_over_the_size_limit_is_a_loud_error() {
    let src = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    common::write_fixture(&src.path().join("one.dcm"), "Doe^John", "1.2.3");

    let cfg_dir = tempfile::tempdir().unwrap();
    let cfg_path = cfg_dir.path().join("anon.toml");
    std::fs::write(&cfg_path, "[limits]\nmax_input_bytes = 10\n").unwrap();

    Command::cargo_bin("dicom-anonymizer")
        .unwrap()
        .args(["-i"])
        .arg(src.path())
        .args(["-o"])
        .arg(out.path())
        .args(["--config"])
        .arg(&cfg_path)
        .assert()
        .failure();

    // Loud, not a quiet skip: an un-anonymised file must fail the run and be recorded.
    let audit = std::fs::read_to_string(out.path().join("audit.jsonl")).unwrap();
    assert!(audit.contains("\"status\":\"error\""), "{audit}");
    assert!(audit.contains("max_input_bytes"), "{audit}");

    // Nothing was written for it.
    let stray: Vec<String> = std::fs::read_dir(out.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n != "run.json" && !n.ends_with(".jsonl"))
        .collect();
    assert!(
        stray.is_empty(),
        "nothing should have been written: {stray:?}"
    );

    // And the marker must not claim the tree is complete.
    let marker = std::fs::read_to_string(out.path().join("run.json")).unwrap();
    assert!(marker.contains("\"outcome\": \"partial\""), "{marker}");
}

#[test]
fn a_zero_size_limit_disables_the_guard() {
    let src = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    common::write_fixture(&src.path().join("one.dcm"), "Doe^John", "1.2.3");

    let cfg_dir = tempfile::tempdir().unwrap();
    let cfg_path = cfg_dir.path().join("anon.toml");
    std::fs::write(&cfg_path, "[limits]\nmax_input_bytes = 0\n").unwrap();

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
}

#[test]
fn verify_scans_non_dicom_sidecar_files() {
    // Real exports ship a sidecar XML (CloudPACS dumps every attribute) that repeats
    // the patient name, ID and institution. Nothing in it is DICOM, so a DICOM-only
    // scan reports 0 hits - a false all-clear on a tree full of identifiers.
    let src = tempfile::tempdir().unwrap();
    common::write_fixture(&src.path().join("one.dcm"), "Doe^John", "1.2.3");
    let dir = tempfile::tempdir().unwrap();
    let fp = dir.path().join("ids.json");

    Command::cargo_bin("dicom-anonymizer")
        .unwrap()
        .args(["-i"])
        .arg(src.path())
        .args(["-o"])
        .arg(dir.path().join("out"))
        .args(["--export-identifiers"])
        .arg(&fp)
        .assert()
        .success();

    // A tree holding only the sidecar: no DICOM at all.
    let tree = tempfile::tempdir().unwrap();
    std::fs::write(
        tree.path().join("study.xml"),
        "<?xml version=\"1.0\"?><Study><Patient>Doe^John</Patient></Study>",
    )
    .unwrap();

    Command::cargo_bin("dicom-anonymizer")
        .unwrap()
        .args(["-i"])
        .arg(tree.path())
        .args(["--verify"])
        .arg(&fp)
        .assert()
        .failure()
        .stderr(predicate::str::contains("study.xml"));
}

#[test]
fn non_dicom_error_mode_fails_instead_of_dropping_the_file() {
    // A run that drops a file from the output tree should be able to say so loudly
    // rather than reporting success while the tree is missing something.
    let src = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    common::write_fixture(&src.path().join("one.dcm"), "Doe^John", "1.2.3");
    std::fs::write(
        src.path().join("study.xml"),
        "<Study><Patient>Doe^John</Patient></Study>",
    )
    .unwrap();

    let cfg_dir = tempfile::tempdir().unwrap();
    let cfg_path = cfg_dir.path().join("anon.toml");
    std::fs::write(&cfg_path, "[output]\nnon_dicom = \"error\"\n").unwrap();

    Command::cargo_bin("dicom-anonymizer")
        .unwrap()
        .args(["-i"])
        .arg(src.path())
        .args(["-o"])
        .arg(out.path())
        .args(["--config"])
        .arg(&cfg_path)
        .assert()
        .failure();

    let audit = std::fs::read_to_string(out.path().join("audit.jsonl")).unwrap();
    assert!(audit.contains("\"status\":\"error\""), "{audit}");
    assert!(audit.contains("study.xml"), "{audit}");
    // the message has to name the setting, or the user cannot act on it
    assert!(audit.contains("non_dicom"), "{audit}");

    let marker = std::fs::read_to_string(out.path().join("run.json")).unwrap();
    assert!(marker.contains("\"outcome\": \"partial\""), "{marker}");

    // The DICOM file is still processed: the error is per-file, not fatal to the batch.
    let wrote_dicom = std::fs::read_dir(out.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .any(|e| e.path().extension().map(|x| x == "dcm").unwrap_or(false));
    assert!(wrote_dicom, "the DICOM should still be anonymised");
}

#[test]
fn require_complete_accepts_a_finished_run() {
    let src = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    common::write_fixture(&src.path().join("one.dcm"), "Doe^John", "1.2.3");
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

    Command::cargo_bin("dicom-anonymizer")
        .unwrap()
        .args(["-i"])
        .arg(out.path())
        .args(["--verify"])
        .arg(&fp)
        .arg("--require-complete")
        .assert()
        .success();
}

#[test]
fn require_complete_rejects_a_tree_without_a_marker() {
    let src = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    common::write_fixture(&src.path().join("one.dcm"), "Doe^John", "1.2.3");
    let fp = dir.path().join("ids.json");
    Command::cargo_bin("dicom-anonymizer")
        .unwrap()
        .args(["-i"])
        .arg(src.path())
        .args(["-o"])
        .arg(dir.path().join("out"))
        .args(["--export-identifiers"])
        .arg(&fp)
        .assert()
        .success();

    // A tree that carries no marker, e.g. one produced by another tool.
    let tree = tempfile::tempdir().unwrap();
    std::fs::write(tree.path().join("notes.txt"), "nothing identifying").unwrap();

    Command::cargo_bin("dicom-anonymizer")
        .unwrap()
        .args(["-i"])
        .arg(tree.path())
        .args(["--verify"])
        .arg(&fp)
        .arg("--require-complete")
        .assert()
        .failure()
        .stderr(predicate::str::contains("no run.json"));
}

#[test]
fn require_complete_rejects_a_partial_run() {
    let src = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    common::write_fixture(&src.path().join("good.dcm"), "Doe^John", "1.2.3");
    common::write_broken_dicom(&src.path().join("bad.dcm"));
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
        .failure();

    // The tree is clean but not complete, and that distinction is the point.
    Command::cargo_bin("dicom-anonymizer")
        .unwrap()
        .args(["-i"])
        .arg(out.path())
        .args(["--verify"])
        .arg(&fp)
        .arg("--require-complete")
        .assert()
        .failure()
        .stderr(predicate::str::contains("not a complete run"));
}

#[test]
fn require_complete_without_verify_is_an_argument_error() {
    let src = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    common::write_fixture(&src.path().join("one.dcm"), "Doe^John", "1.2.3");

    Command::cargo_bin("dicom-anonymizer")
        .unwrap()
        .args(["-i"])
        .arg(src.path())
        .args(["-o"])
        .arg(out.path())
        .arg("--require-complete")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("only applies to --verify"));
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
