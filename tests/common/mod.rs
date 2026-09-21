use dicom_core::{DataElement, Tag, VR};
use dicom_object::{FileMetaTableBuilder, InMemDicomObject};
use std::path::Path;

/// Write a minimal DICOM file with identity fields at `path`.
pub fn write_fixture(path: &Path, name: &str, study_uid: &str) {
    write_fixture_extra(path, name, study_uid, &[]);
}

/// Write a minimal DICOM file from hand-built bytes.
///
/// Needed for inputs the writer itself refuses to produce: text encoded as UTF-8
/// with no `SpecificCharacterSet` declared. Real-world files do contain these, so
/// the reader's behaviour on them has to be testable.
///
/// Contains: PatientName `程英平`, InstitutionName `万楠医学院附属医院`, and a
/// StudyDescription combining them with clinical text (`…^腰椎`).
pub fn write_raw_cjk_fixture(path: &Path) {
    fn el(g: u16, e: u16, vr: &[u8; 2], mut val: Vec<u8>, pad: u8) -> Vec<u8> {
        if val.len() % 2 == 1 {
            val.push(pad);
        }
        let long = matches!(
            vr,
            b"OB" | b"OW" | b"OF" | b"SQ" | b"UT" | b"UN" | b"OD" | b"OL" | b"UC" | b"UR" | b"OV"
        );
        let mut out = Vec::new();
        out.extend_from_slice(&g.to_le_bytes());
        out.extend_from_slice(&e.to_le_bytes());
        out.extend_from_slice(vr);
        if long {
            out.extend_from_slice(&[0, 0]);
            out.extend_from_slice(&(val.len() as u32).to_le_bytes());
        } else {
            out.extend_from_slice(&(val.len() as u16).to_le_bytes());
        }
        out.extend_from_slice(&val);
        out
    }
    let ui = |s: &str| s.as_bytes().to_vec();

    let meta_body: Vec<u8> = [
        el(0x0002, 0x0001, b"OB", vec![0, 1], 0),
        el(0x0002, 0x0002, b"UI", ui("1.2.840.10008.5.1.4.1.1.7"), 0),
        el(0x0002, 0x0003, b"UI", ui("1.2.3.4.5"), 0),
        el(0x0002, 0x0010, b"UI", ui("1.2.840.10008.1.2.1"), 0),
        el(0x0002, 0x0012, b"UI", ui("1.2.826.0.1.3680043.8.498.1"), 0),
    ]
    .concat();

    let mut blob = vec![0u8; 128];
    blob.extend_from_slice(b"DICM");
    blob.extend_from_slice(&el(
        0x0002,
        0x0000,
        b"UL",
        (meta_body.len() as u32).to_le_bytes().to_vec(),
        0,
    ));
    blob.extend_from_slice(&meta_body);
    // No (0008,0005): that is the point of the fixture.
    blob.extend_from_slice(&el(
        0x0008,
        0x0016,
        b"UI",
        ui("1.2.840.10008.5.1.4.1.1.7"),
        0,
    ));
    blob.extend_from_slice(&el(0x0008, 0x0018, b"UI", ui("1.2.3.4.5"), 0));
    blob.extend_from_slice(&el(
        0x0010,
        0x0010,
        b"PN",
        "程英平".as_bytes().to_vec(),
        b' ',
    ));
    blob.extend_from_slice(&el(
        0x0008,
        0x0080,
        b"LO",
        "万楠医学院附属医院".as_bytes().to_vec(),
        b' ',
    ));
    blob.extend_from_slice(&el(
        0x0008,
        0x1030,
        b"LO",
        "万楠医学院附属医院^腰椎".as_bytes().to_vec(),
        b' ',
    ));

    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, blob).unwrap();
}

/// Same as [`write_fixture`], plus caller-supplied extra elements (e.g. private
/// attributes, for exercising the leak check).
pub fn write_fixture_extra(path: &Path, name: &str, study_uid: &str, extra: &[(Tag, VR, &str)]) {
    let mut elems: Vec<DataElement<InMemDicomObject>> = vec![
        DataElement::new(dicom_dictionary_std::tags::PATIENT_NAME, VR::PN, name),
        DataElement::new(dicom_dictionary_std::tags::PATIENT_ID, VR::LO, "PID-1"),
        DataElement::new(
            dicom_dictionary_std::tags::STUDY_INSTANCE_UID,
            VR::UI,
            study_uid,
        ),
        DataElement::new(
            dicom_dictionary_std::tags::SOP_CLASS_UID,
            VR::UI,
            "1.2.840.10008.5.1.4.1.1.7",
        ),
        DataElement::new(
            dicom_dictionary_std::tags::SOP_INSTANCE_UID,
            VR::UI,
            format!("{study_uid}.1"),
        ),
        DataElement::new(dicom_dictionary_std::tags::MODALITY, VR::CS, "CT"),
    ];
    for (tag, vr, value) in extra {
        elems.push(DataElement::new(*tag, *vr, value.to_string()));
    }
    let file = InMemDicomObject::from_element_iter(elems)
        .with_meta(FileMetaTableBuilder::new().transfer_syntax("1.2.840.10008.1.2.1"))
        .unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    file.write_to_file(path).unwrap();
}
