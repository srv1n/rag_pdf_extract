use pdf_extract::{
    decompress_content_ext_bytes, extract_pdf_location, parse_pdf, ExtractionOptions, LAParams,
};
use std::env;
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut max_tokens = 350usize;
    let mut paths = Vec::new();
    let mut args = env::args().skip(1);

    while let Some(arg) = args.next() {
        if arg == "--max-tokens" {
            let value = args.next().ok_or("--max-tokens requires a value")?;
            max_tokens = value.parse()?;
        } else {
            paths.push(arg);
        }
    }

    if paths.is_empty() {
        eprintln!("Usage: pdf_batch_benchmark [--max-tokens N] <pdf> [<pdf> ...]");
        std::process::exit(2);
    }

    println!(
        "path\tstatus\twall_ms\tchunks\tchars\tcompressed_ext_bytes\tuncompressed_ext_bytes\tfragments\toutput_spans\tchunk_location_boxes\tspan_location_bytes\tchunk_location_bytes"
    );

    for (idx, path) in paths.iter().enumerate() {
        let mut laparams = LAParams::default();
        laparams.all_texts = true;

        let started = Instant::now();
        let result = parse_pdf(
            path,
            idx as i64 + 1,
            "file",
            None,
            None,
            None,
            Some(max_tokens),
            Some(laparams),
            Some(true),
            ExtractionOptions {
                emit_output_spans: true,
                ..ExtractionOptions::default()
            },
        );
        let wall_ms = started.elapsed().as_millis();

        match result {
            Ok(docs) => {
                let chars = docs
                    .iter()
                    .map(|doc| doc.content_core.content.chars().count())
                    .sum::<usize>();
                let compressed_ext_bytes = docs
                    .iter()
                    .map(|doc| doc.content_ext.ext_json.len())
                    .sum::<usize>();
                let mut uncompressed_ext_bytes = 0usize;
                let mut fragments = 0usize;
                let mut output_spans = 0usize;
                let mut chunk_location_boxes = 0usize;
                let mut span_location_bytes = 0usize;
                let mut chunk_location_bytes = 0usize;
                for doc in &docs {
                    fragments += extract_pdf_location(&doc.content_ext)
                        .map(|location| location.fragments.len())
                        .unwrap_or(0);
                    let Ok(bytes) = decompress_content_ext_bytes(&doc.content_ext) else {
                        continue;
                    };
                    uncompressed_ext_bytes += bytes.len();
                    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
                        continue;
                    };
                    output_spans += value
                        .get("output_spans")
                        .and_then(|value| value.as_array())
                        .map(|items| items.len())
                        .unwrap_or(0);
                    if let Some(spans) = value.get("output_spans") {
                        span_location_bytes += serde_json::to_vec(spans)
                            .map(|bytes| bytes.len())
                            .unwrap_or(0);
                    }
                    if let Some(locations) = value.get("chunk_locations") {
                        chunk_location_bytes += serde_json::to_vec(locations)
                            .map(|bytes| bytes.len())
                            .unwrap_or(0);
                        chunk_location_boxes += locations
                            .as_array()
                            .map(|items| {
                                items
                                    .iter()
                                    .filter_map(|item| {
                                        item.get("bboxes").and_then(|b| b.as_array())
                                    })
                                    .map(|bboxes| bboxes.len())
                                    .sum::<usize>()
                            })
                            .unwrap_or(0);
                    }
                }
                println!(
                    "{path}\tok\t{wall_ms}\t{}\t{chars}\t{compressed_ext_bytes}\t{uncompressed_ext_bytes}\t{fragments}\t{output_spans}\t{chunk_location_boxes}\t{span_location_bytes}\t{chunk_location_bytes}",
                    docs.len()
                );
            }
            Err(err) => {
                println!("{path}\terr:{err}\t{wall_ms}\t0\t0\t0\t0\t0\t0\t0\t0\t0");
            }
        }
    }

    Ok(())
}
