/// Restore only a missing /Size in a classic xref's final trailer. Keep the
/// original dictionary (especially /Encrypt and /ID) and all object offsets.
pub(crate) fn restore_missing_size(bytes: &[u8]) -> Option<Vec<u8>> {
    let start = bytes.windows(9).rposition(|part| part == b"startxref")?;
    // Parse tokens without interpreting object or stream contents.
    fn token<'a>(input: &mut &'a [u8]) -> Option<&'a [u8]> {
        *input = input.trim_ascii_start();
        let end = input
            .iter()
            .position(|b| b.is_ascii_whitespace())
            .unwrap_or(input.len());
        if end == 0 {
            return None;
        }
        let value = &input[..end];
        *input = &input[end..];
        Some(value)
    }
    fn number(input: &mut &[u8]) -> Option<u32> {
        let value = token(input)?;
        if !value.iter().all(u8::is_ascii_digit) {
            return None;
        }
        std::str::from_utf8(value).ok()?.parse().ok()
    }
    let mut tail = &bytes[start + 9..];
    let offset = number(&mut tail)? as usize;
    let mut input = bytes.get(offset..start)?;
    if token(&mut input)? != b"xref" {
        return None;
    }
    let mut size = 0;
    loop {
        input = input.trim_ascii_start();
        if input.starts_with(b"trailer") {
            break;
        }
        let first = number(&mut input)?;
        let count = number(&mut input)?;
        size = size.max(first.checked_add(count)?);
        for index in 0..count {
            let object_offset = number(&mut input)? as usize;
            let generation = number(&mut input)?;
            match token(&mut input)? {
                b"f" => {}
                b"n" => {
                    let mut object = bytes.get(object_offset..offset)?;
                    if number(&mut object)? != first.checked_add(index)?
                        || number(&mut object)? != generation
                        || token(&mut object)? != b"obj"
                    {
                        return None;
                    }
                }
                _ => return None,
            }
        }
    }
    if size == 0 || token(&mut input)? != b"trailer" {
        return None;
    }
    input = input.trim_ascii_start();
    // Conservative: escaped names and existing /Size are left to the parser.
    if !input.starts_with(b"<<")
        || input.contains(&b'#')
        || [b"/Size".as_slice(), b"/Prev", b"/XRefStm"]
            .iter()
            .any(|name| input.windows(name.len()).any(|p| p == *name))
    {
        return None;
    }
    let insertion = start - input.len() + 2;
    let mut repaired = bytes.to_vec();
    repaired.splice(insertion..insertion, format!(" /Size {size} ").bytes());
    Some(repaired)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        load_pdf_from_mem, EncryptionFailure, ExtractionOptions, OutputError, PdfPassword,
        RepairKind,
    };
    use lopdf::{dictionary, Document, EncryptionState, EncryptionVersion, Object, Permissions};
    use std::convert::TryFrom;

    fn fixture(password: Option<&str>) -> Vec<u8> {
        let mut doc = Document::new();
        doc.reference_table.cross_reference_type = lopdf::xref::XrefType::CrossReferenceTable;
        doc.trailer.set(
            "ID",
            Object::Array(vec![
                Object::string_literal("fixture-id"),
                Object::string_literal("fixture-id"),
            ]),
        );
        let pages = doc.new_object_id();
        let page = doc.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
        });
        doc.objects.insert(
            pages,
            dictionary! {
                "Type" => "Pages", "Kids" => vec![Object::Reference(page)], "Count" => 1,
            }
            .into(),
        );
        let root = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages });
        doc.trailer.set("Root", root);
        if let Some(password) = password {
            let state = EncryptionState::try_from(EncryptionVersion::V1 {
                document: &doc,
                owner_password: "owner",
                user_password: password,
                permissions: Permissions::all(),
            })
            .unwrap();
            doc.encrypt(&state).unwrap();
        }
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();
        bytes
    }

    fn omit_size(bytes: &[u8]) -> Vec<u8> {
        let mut result = bytes.to_vec();
        let start = result.windows(6).rposition(|p| p == b"/Size ").unwrap();
        let end = start
            + 6
            + result[start + 6..]
                .iter()
                .take_while(|b| b.is_ascii_digit())
                .count();
        result.drain(start..end);
        result
    }

    #[test]
    fn missing_trailer_size_is_repaired_and_opt_out_is_respected() {
        let bytes = omit_size(&fixture(None));
        assert!(Document::load_mem(&bytes).is_err());
        let loaded = load_pdf_from_mem(&bytes, &ExtractionOptions::default()).unwrap();
        assert_eq!(loaded.document.get_pages().len(), 1);
        assert_eq!(loaded.repairs, vec![RepairKind::MissingTrailerSizeRepaired]);
        let mut options = ExtractionOptions::default();
        options.enable_repairs = false;
        assert!(matches!(
            load_pdf_from_mem(&bytes, &options),
            Err(OutputError::InvalidStructure { .. })
        ));
    }

    #[test]
    fn missing_trailer_size_keeps_password_authentication() {
        let bytes = omit_size(&fixture(Some("secret")));
        let mut options = ExtractionOptions::default();
        assert!(matches!(
            load_pdf_from_mem(&bytes, &options),
            Err(OutputError::Encrypted {
                reason: EncryptionFailure::PasswordRequired,
                ..
            })
        ));
        options.password = Some(PdfPassword::new("wrong"));
        assert!(matches!(
            load_pdf_from_mem(&bytes, &options),
            Err(OutputError::Encrypted {
                reason: EncryptionFailure::IncorrectPassword,
                ..
            })
        ));
        options.password = Some(PdfPassword::new("secret"));
        assert_eq!(
            load_pdf_from_mem(&bytes, &options)
                .unwrap()
                .document
                .get_pages()
                .len(),
            1
        );
    }

    #[test]
    fn existing_size_or_broken_xref_is_not_repaired() {
        assert!(restore_missing_size(&fixture(None)).is_none());
        let missing = omit_size(&fixture(None));
        let mut incremental = missing.clone();
        let trailer = incremental
            .windows(7)
            .rposition(|p| p == b"trailer")
            .unwrap();
        let dictionary = trailer
            + incremental[trailer..]
                .windows(2)
                .position(|p| p == b"<<")
                .unwrap()
            + 2;
        incremental.splice(dictionary..dictionary, b" /Prev 0 ".iter().copied());
        assert!(restore_missing_size(&incremental).is_none());
        let mut broken = missing;
        let xref = broken.windows(5).position(|p| p == b"xref\n").unwrap();
        broken[xref] = b'X';
        assert!(restore_missing_size(&broken).is_none());
        assert!(matches!(
            load_pdf_from_mem(&broken, &ExtractionOptions::default()),
            Err(OutputError::InvalidStructure { .. })
        ));
    }
}
