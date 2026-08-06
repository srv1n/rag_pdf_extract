use lopdf::{
    dictionary, Dictionary, Document, EncryptionState, EncryptionVersion, Object, Permissions,
    Stream,
};
use pdf_extract::{
    extract_text_from_mem, parse_pdf, EncryptionFailure, ExtractionOptions, OutputError,
};
use std::convert::TryFrom;

fn minimal_pdf(page_count: usize) -> Vec<u8> {
    let mut objects = Vec::new();
    objects.push(b"<< /Type /Catalog /Pages 2 0 R >>".to_vec());
    let kids = (0..page_count)
        .map(|index| format!("{} 0 R", index + 3))
        .collect::<Vec<_>>()
        .join(" ");
    objects.push(format!("<< /Type /Pages /Kids [{kids}] /Count {page_count} >>").into_bytes());
    for _ in 0..page_count {
        objects.push(b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".to_vec());
    }

    let mut pdf = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
        pdf.extend_from_slice(object);
        pdf.extend_from_slice(b"\nendobj\n");
    }
    let xref_offset = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n", objects.len() + 1).as_bytes());
    pdf.extend_from_slice(b"0000000000 65535 f \n");
    for offset in offsets {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Root 1 0 R /Size {} /ID [<00112233445566778899aabbccddeeff> <00112233445566778899aabbccddeeff>] >>\nstartxref\n{}\n%%EOF\n",
            objects.len() + 1,
            xref_offset
        )
        .as_bytes(),
    );
    pdf
}

fn encrypted_pdf(user_password: &str) -> Vec<u8> {
    let mut document = Document::load_mem(&minimal_pdf(1)).expect("minimal PDF");
    let root = document
        .trailer
        .get(b"Root")
        .expect("root")
        .as_reference()
        .expect("root reference");
    document.get_object(root).expect("load root");
    let version = EncryptionVersion::V1 {
        document: &document,
        owner_password: "owner-password",
        user_password,
        permissions: Permissions::all(),
    };
    let state = EncryptionState::try_from(version).expect("encryption state");
    document.encrypt(&state).expect("encrypt fixture");
    let mut bytes = Vec::new();
    document
        .save_to(&mut bytes)
        .expect("serialize encrypted PDF");
    bytes
}

fn add_prefix_without_xref_rewrite(pdf: &[u8], prefix: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(prefix.len() + pdf.len());
    out.extend_from_slice(prefix);
    out.extend_from_slice(pdf);
    out
}

fn deep_form_xobject_pdf(depth: usize) -> Vec<u8> {
    let mut document = Document::new();
    let font_id = document.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica",
    });

    let mut next_form: Option<(String, lopdf::ObjectId)> = None;
    for level in (1..=depth).rev() {
        let mut resources = Dictionary::new();
        let content = if let Some((next_name, next_id)) = next_form.as_ref() {
            let mut xobjects = Dictionary::new();
            xobjects.set(next_name.as_str(), Object::Reference(*next_id));
            resources.set("XObject", xobjects);
            format!("/{next_name} Do\n").into_bytes()
        } else {
            Vec::new()
        };
        let mut dict = dictionary! {
            "Type" => "XObject",
            "Subtype" => "Form",
            "BBox" => vec![0.into(), 0.into(), 10.into(), 10.into()],
        };
        if !resources.is_empty() {
            dict.set("Resources", resources);
        }
        let id = document.add_object(Stream::new(dict, content));
        next_form = Some((format!("Fm{level}"), id));
    }
    let (root_form_name, root_form_id) = next_form.expect("root form");

    let mut fonts = Dictionary::new();
    fonts.set("F1", Object::Reference(font_id));
    let mut xobjects = Dictionary::new();
    xobjects.set(root_form_name.as_str(), Object::Reference(root_form_id));
    let resources = dictionary! {
        "Font" => fonts,
        "XObject" => xobjects,
    };
    let content =
        format!("BT /F1 12 Tf 72 720 Td (VISIBLE SURVIVES) Tj ET\n/{root_form_name} Do\n");
    let content_id = document.add_object(Stream::new(Dictionary::new(), content.into_bytes()));
    let pages_id = document.new_object_id();
    let page_id = document.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "Contents" => content_id,
        "Resources" => resources,
        "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
    });
    document.objects.insert(
        pages_id,
        dictionary! {
            "Type" => "Pages",
            "Kids" => vec![Object::Reference(page_id)],
            "Count" => 1,
        }
        .into(),
    );
    let catalog_id = document.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    document.trailer.set("Root", catalog_id);
    let mut bytes = Vec::new();
    document
        .save_to(&mut bytes)
        .expect("serialize deep form PDF");
    bytes
}

fn write_temp_pdf(name: &str, bytes: &[u8]) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "pdf-extract-rework-core-{name}-{}-{}.pdf",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(&path, bytes).expect("write temp pdf");
    path
}

#[test]
fn prefixed_pdf_with_absolute_xref_tries_original_before_repair() {
    let pdf = deep_form_xobject_pdf(1);
    let prefixed = add_prefix_without_xref_rewrite(&pdf, b"garbage before header\n");
    let path = write_temp_pdf("prefixed-original", &prefixed);

    let results = parse_pdf(
        path.to_str().expect("utf8 temp path"),
        1,
        "file",
        None,
        None,
        None,
        Some(500),
        None,
        Some(true),
        ExtractionOptions::default(),
    )
    .expect("original bytes should parse before repair");
    std::fs::remove_file(&path).expect("remove temp pdf");

    assert!(
        results.iter().all(|result| result.repairs.is_empty()),
        "repair must not be recorded when original bytes parse"
    );
}

#[test]
fn failed_repair_preserves_original_typed_error_and_marks_repair_attempted() {
    let encrypted = encrypted_pdf("secret");
    let prefixed = add_prefix_without_xref_rewrite(&encrypted, b"garbage before header\n");

    let error = extract_text_from_mem(&prefixed, None).expect_err("missing password");

    match error {
        OutputError::Encrypted {
            reason: EncryptionFailure::PasswordRequired,
            context,
        } => {
            assert!(
                context.repair_attempted,
                "repair fallback must annotate the original typed error"
            );
        }
        other => panic!("expected original PasswordRequired error, got {:?}", other),
    }
}

#[test]
fn deep_form_xobject_depth_skip_preserves_page_text() {
    let bytes = deep_form_xobject_pdf(9);
    let path = write_temp_pdf("deep-form", &bytes);

    let results = parse_pdf(
        path.to_str().expect("utf8 temp path"),
        1,
        "file",
        None,
        None,
        None,
        Some(500),
        None,
        Some(true),
        ExtractionOptions {
            max_recursion_depth: Some(2),
            ..ExtractionOptions::default()
        },
    )
    .expect("depth excess in nested Form XObjects should not abort extraction");
    std::fs::remove_file(&path).expect("remove temp pdf");

    let text = results
        .iter()
        .map(|result| result.content_core.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        text.contains("VISIBLE SURVIVES"),
        "page text should survive nested stream skip; got: {:?}",
        text
    );
}
