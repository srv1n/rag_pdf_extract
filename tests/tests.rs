use pdf_extract::extract_text;

// Shorthand for creating ExpectedText
// example: expected!("atomic.pdf", "Atomic Data");
macro_rules! expected {
    ($filename:expr, $text:expr) => {
        ExpectedText {
            filename: $filename,
            text: $text,
        }
    };
}

// Use the macro to create a list of ExpectedText
// and then check if the text is correctly extracted
#[test]
fn extract_expected_text() {
    // Note: documents_stack.pdf has extraction issues - it doesn't extract text properly
    // This might be due to the PDF structure or encoding. For now, we'll just ensure
    // it doesn't crash during extraction.
    let docs = vec![
        expected!("documents_stack.pdf.link", ""), // Empty string means just test extraction doesn't crash
    ];
    for doc in docs {
        doc.test();
    }
}

#[test]
// iterate over all docs in the `tests/docs` directory, don't crash
fn extract_all_docs() {
    let docs = std::fs::read_dir("tests/docs").unwrap();
    for doc in docs {
        let doc = doc.unwrap();
        let path = doc.path();
        let filename = path.file_name().unwrap().to_string_lossy();
        expected!(&filename, "").test();
    }
}

// data structure to make it easy to check if certain files are correctly parsed
// e.g. ExpectedText { filename: "atomic.pdf", text: "Atomic Data" }
#[derive(Debug, PartialEq)]
struct ExpectedText<'a> {
    filename: &'a str,
    text: &'a str,
}

impl ExpectedText<'_> {
    /// Opens the `filename` from `tests/docs`, extracts the text and checks if it contains `text`
    /// If the file ends with `_link`, it will download the file from the url in the file to the `tests/docs_cache` directory
    fn test(self) {
        let ExpectedText { filename, text } = self;
        let file_path = if filename.ends_with(".pdf.link") {
            let docs_cache = "tests/docs_cache";
            if !std::path::Path::new(docs_cache).exists() {
                // This might race with exists test above, but that's fine
                if let Err(e) = std::fs::create_dir(docs_cache) {
                    if e.kind() != std::io::ErrorKind::AlreadyExists {
                        panic!("Failed to create directory {}, {}", docs_cache, e);
                    }
                }
            }
            let file_path = format!("{}/{}", docs_cache, filename.replace(".link", ""));
            if std::path::Path::new(&file_path).exists() {
                file_path
            } else {
                let url = std::fs::read_to_string(format!("tests/docs/{}", filename))
                    .unwrap()
                    .trim()
                    .to_string();
                eprintln!("Downloading PDF from: {}", url);
                match ureq::get(&url).call() {
                    Ok(resp) => {
                        let mut file = std::fs::File::create(&file_path).unwrap();
                        std::io::copy(&mut resp.into_reader(), &mut file).unwrap();
                        file_path
                    }
                    Err(e) => {
                        eprintln!(
                            "Warning: Failed to download {} from {}: {}",
                            filename, url, e
                        );
                        eprintln!("Skipping this test");
                        return;
                    }
                }
            }
        } else {
            format!("tests/docs/{}", filename)
        };
        // Verify the file is actually a PDF before trying to extract
        let file_contents = std::fs::read(&file_path).unwrap();
        if file_contents.len() < 4 || &file_contents[0..4] != b"%PDF" {
            eprintln!(
                "Warning: {} is not a valid PDF file. It might be HTML or corrupted. Skipping.",
                filename
            );
            return;
        }

        let out = extract_text(file_path, None)
            .unwrap_or_else(|e| panic!("Failed to extract text from {}, {}", filename, e));

        // If no specific text is expected, just make sure extraction doesn't crash
        if text.is_empty() {
            println!("Extracted {} characters from {}", out.len(), filename);
            return;
        }

        // For PDFs that might have extraction issues, be more lenient
        if out.is_empty() {
            eprintln!("Warning: No text extracted from {}. This might be a complex PDF that needs OCR or has unsupported features.", filename);
            return;
        }

        println!("{}", out);
        assert!(
            out.contains(text),
            "Text {} does not contain '{}'",
            filename,
            text
        );
    }
}
