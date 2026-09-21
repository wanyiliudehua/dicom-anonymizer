use dicom_core::{DataElement, VR};
use dicom_object::{FileMetaTableBuilder, InMemDicomObject};
use std::path::Path;

/// Write a minimal DICOM file with identity fields at `path`.
pub fn write_fixture(path: &Path, name: &str, study_uid: &str) {
    let obj = InMemDicomObject::from_element_iter([
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
    ]);
    let file = obj
        .with_meta(FileMetaTableBuilder::new().transfer_syntax("1.2.840.10008.1.2.1"))
        .unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    file.write_to_file(path).unwrap();
}
