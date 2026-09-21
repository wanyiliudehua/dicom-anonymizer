use dicom_core::{DataElement, Tag, VR};
use dicom_object::{FileMetaTableBuilder, InMemDicomObject};
use std::path::Path;

/// Write a minimal DICOM file with identity fields at `path`.
pub fn write_fixture(path: &Path, name: &str, study_uid: &str) {
    write_fixture_extra(path, name, study_uid, &[]);
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
