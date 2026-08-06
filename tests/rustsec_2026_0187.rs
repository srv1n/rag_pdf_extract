//! Regression for the deep-nesting parser denial of service covered by
//! RUSTSEC-2026-0187.  Keep the input generated and readable: the important
//! property is that lopdf parses bytes containing a deeply nested array, not
//! that we preserve an opaque malicious binary in the repository.

fn deeply_nested_pdf(depth: usize) -> Vec<u8> {
    let mut objects = Vec::new();
    let mut nested = Vec::with_capacity(depth * 2 + 1);
    nested.extend(std::iter::repeat_n(b'[', depth));
    nested.push(b'0');
    nested.extend(std::iter::repeat_n(b']', depth));

    let mut catalog = b"<< /Type /Catalog /Pages 2 0 R /Nested ".to_vec();
    catalog.extend_from_slice(&nested);
    catalog.extend_from_slice(b" >>");
    objects.push(catalog);
    objects.push(b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec());
    objects.push(b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".to_vec());

    let mut pdf = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::with_capacity(objects.len() + 1);
    offsets.push(0usize);
    for (index, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
        pdf.extend_from_slice(object);
        pdf.extend_from_slice(b"\nendobj\n");
    }

    let xref_offset = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n", objects.len() + 1).as_bytes());
    pdf.extend_from_slice(b"0000000000 65535 f \n");
    for offset in offsets.iter().skip(1) {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Root 1 0 R /Size {} >>\nstartxref\n{}\n%%EOF\n",
            objects.len() + 1,
            xref_offset
        )
        .as_bytes(),
    );
    pdf
}

#[test]
fn rustsec_2026_0187_deep_nesting_is_a_clean_error_through_crate_entry_point() {
    let fixture = deeply_nested_pdf(10_000);
    let result = pdf_extract::extract_text_from_mem(&fixture, None);

    assert!(
        matches!(
            result,
            Err(pdf_extract::OutputError::InvalidStructure { .. })
                | Err(pdf_extract::OutputError::Parse { .. })
        ),
        "crate entry point must reject deeply nested input cleanly: {result:?}"
    );
}
