//! Thin wrapper over the `dicom-object` mutation API.
//!
//! Everything the de-id engine needs from the DICOM object model goes through
//! these functions, so a change in the upstream API is contained here.

use dicom_core::header::Header;
use dicom_core::{DataElement, Tag, VR};
use dicom_object::InMemDicomObject;

/// All element tags present in the object, sorted and de-duplicated.
pub fn tag_list(obj: &InMemDicomObject) -> Vec<Tag> {
    let mut v: Vec<Tag> = obj.iter().map(|e| e.tag()).collect();
    v.sort();
    v.dedup();
    v
}

/// Read an element as a single string value, if present and string-like.
pub fn get_string(obj: &InMemDicomObject, tag: Tag) -> Option<String> {
    obj.element(tag)
        .ok()
        .and_then(|e| e.to_str().ok())
        .map(|s| s.to_string())
}

/// Replace (or insert) a string-valued element.
pub fn set_string(obj: &mut InMemDicomObject, tag: Tag, vr: VR, value: &str) {
    obj.put_element(DataElement::new(tag, vr, value.to_string()));
}

/// Replace (or insert) a multi-valued UI element (backslash-joined).
pub fn set_multi_string(obj: &mut InMemDicomObject, tag: Tag, values: Vec<String>) {
    let joined = values.join("\\");
    obj.put_element(DataElement::new(tag, VR::UI, joined));
}

/// Remove an element, reporting whether it was present.
pub fn remove(obj: &mut InMemDicomObject, tag: Tag) -> bool {
    obj.remove_element(tag)
}

/// Set an element to an empty value, keeping its VR. Reports prior presence.
pub fn zero(obj: &mut InMemDicomObject, tag: Tag) -> bool {
    let vr = obj.element(tag).map(|e| e.vr()).unwrap_or(VR::UN);
    let present = obj.remove_element(tag);
    obj.put_element(DataElement::new(tag, vr, ""));
    present
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(g: u16, e: u16) -> Tag {
        Tag(g, e)
    }

    #[test]
    fn set_get_remove_zero_roundtrip() {
        let mut o = InMemDicomObject::new_empty();
        set_string(&mut o, tag(0x0010, 0x0010), VR::PN, "Doe^John");
        assert_eq!(get_string(&o, tag(0x0010, 0x0010)).as_deref(), Some("Doe^John"));
        assert!(remove(&mut o, tag(0x0010, 0x0010)));
        assert!(!remove(&mut o, tag(0x0010, 0x0010)));
        assert_eq!(get_string(&o, tag(0x0010, 0x0010)), None);
    }

    #[test]
    fn zero_makes_element_present_but_empty() {
        let mut o = InMemDicomObject::new_empty();
        set_string(&mut o, tag(0x0010, 0x0010), VR::PN, "Doe^John");
        assert!(zero(&mut o, tag(0x0010, 0x0010)));
        assert_ne!(get_string(&o, tag(0x0010, 0x0010)).as_deref(), Some("Doe^John"));
    }

    #[test]
    fn tag_list_is_sorted_and_complete() {
        let o = InMemDicomObject::from_element_iter([
            DataElement::new(tag(0x0010, 0x0020), VR::LO, "A"),
            DataElement::new(tag(0x0008, 0x0060), VR::CS, "CT"),
        ]);
        let t = tag_list(&o);
        assert_eq!(t, vec![tag(0x0008, 0x0060), tag(0x0010, 0x0020)]);
    }
}
