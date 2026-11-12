use super::{image_memory_bounds, ExtractionOptions, OutputError, ResourceLimitKind};
use lopdf::{Document, Object, Stream};
use std::convert::TryFrom;

impl Default for ExtractionOptions {
    fn default() -> Self {
        Self {
            emit_output_spans: false,
            max_pages: Some(10_000),
            max_objects: Some(1_000_000),
            max_recursion_depth: Some(8),
            max_decompressed_stream_bytes: Some(128 * 1024 * 1024),
            max_output_bytes: Some(128 * 1024 * 1024),
            password: None,
            enable_repairs: true,
        }
    }
}

fn decompression_limit_error(limit: usize, observed: Option<usize>) -> OutputError {
    OutputError::resource_limit(ResourceLimitKind::DecompressedStreamBytes, limit, observed)
}

fn bounded_decompressed_content(
    stream: &Stream,
    remaining: usize,
    image_limit: usize,
) -> Result<Vec<u8>, OutputError> {
    // Image XObjects are decoded one at a time, so charging every raw bitmap
    // cumulatively rejects ordinary scanned volumes. Enforce the configured
    // limit against each image's decoded working size, then charge its stored
    // bytes to the cumulative document budget.
    if stream
        .dict
        .get(b"Subtype")
        .ok()
        .and_then(|value| value.as_name().ok())
        == Some(b"Image".as_slice())
    {
        let width = stream
            .dict
            .get(b"Width")
            .ok()
            .and_then(|value| value.as_i64().ok())
            .and_then(|value| usize::try_from(value).ok())
            .unwrap_or(usize::MAX);
        let height = stream
            .dict
            .get(b"Height")
            .ok()
            .and_then(|value| value.as_i64().ok())
            .and_then(|value| usize::try_from(value).ok())
            .unwrap_or(usize::MAX);
        let bits_per_component = stream
            .dict
            .get(b"BitsPerComponent")
            .ok()
            .and_then(|value| value.as_i64().ok())
            .and_then(|value| usize::try_from(value).ok())
            .unwrap_or(8);
        let color_space_name = stream.dict.get(b"ColorSpace").ok().and_then(|value| {
            value.as_name().ok().or_else(|| {
                value
                    .as_array()
                    .ok()
                    .and_then(|values| values.first())
                    .and_then(|value| value.as_name().ok())
            })
        });
        let components = match color_space_name {
            Some(b"DeviceGray") | Some(b"Indexed") => 1,
            Some(b"DeviceRGB") => 3,
            Some(b"DeviceCMYK") => 4,
            _ => 4,
        };
        let predictor = stream
            .dict
            .get(b"DecodeParms")
            .ok()
            .and_then(|value| value.as_dict().ok())
            .and_then(|params| params.get(b"Predictor").ok())
            .and_then(|value| value.as_i64().ok())
            .is_some_and(|value| (10..=15).contains(&value));
        let working_set_bytes = image_memory_bounds(
            width,
            height,
            components,
            bits_per_component,
            predictor,
            stream.content.len(),
        )
        .map(|bounds| bounds.working_set_bytes)
        .unwrap_or(usize::MAX);
        if working_set_bytes > image_limit {
            return Err(decompression_limit_error(
                image_limit,
                Some(working_set_bytes),
            ));
        }
        if stream.content.len() > remaining {
            return Err(decompression_limit_error(
                remaining,
                Some(stream.content.len()),
            ));
        }
        return Ok(stream.content.clone());
    }

    let filters = match stream.filters() {
        Ok(filters) if !filters.is_empty() => filters,
        _ => {
            if stream.content.len() > remaining {
                return Err(decompression_limit_error(
                    remaining,
                    Some(stream.content.len()),
                ));
            }
            return Ok(stream.content.clone());
        }
    };

    let mut input = stream.content.clone();
    for filter in filters {
        let output = match filter {
            b"FlateDecode" => bounded_flate_decode(&input, remaining)?,
            b"LZWDecode" => bounded_lzw_decode(stream, &input, remaining)?,
            b"ASCII85Decode" => bounded_ascii85_decode(&input, remaining)?,
            _ => {
                // Image codecs such as DCT/JPX are intentionally opaque to
                // text extraction and are not inflated by lopdf's content
                // decoder. Bound their stored bytes without rejecting every
                // ordinary JPEG-backed PDF. All filters that this crate (or
                // lopdf) can inflate are handled above with bounded decoders.
                if input.len() > remaining {
                    return Err(decompression_limit_error(remaining, Some(input.len())));
                }
                input.clone()
            }
        };
        input = output;
    }
    Ok(input)
}

pub(super) fn bounded_flate_decode(input: &[u8], limit: usize) -> Result<Vec<u8>, OutputError> {
    fn read_bounded<R: std::io::Read>(mut reader: R, limit: usize) -> Result<Vec<u8>, bool> {
        let mut output = Vec::new();
        let mut buffer = [0u8; 8192];
        loop {
            let read = reader.read(&mut buffer).map_err(|_| false)?;
            if read == 0 {
                return Ok(output);
            }
            if output.len().saturating_add(read) > limit {
                return Err(true);
            }
            output.extend_from_slice(&buffer[..read]);
        }
    }

    let zlib = flate2::read::ZlibDecoder::new(std::io::Cursor::new(input));
    match read_bounded(zlib, limit) {
        Ok(output) => Ok(output),
        Err(true) => Err(decompression_limit_error(
            limit,
            Some(limit.saturating_add(1)),
        )),
        Err(false) => {
            // Match lopdf's raw-deflate compatibility fallback without ever
            // allowing the fallback decoder to allocate beyond the budget.
            if input.len() > 2 {
                let raw = flate2::read::DeflateDecoder::new(std::io::Cursor::new(&input[2..]));
                match read_bounded(raw, limit) {
                    Ok(output) => Ok(output),
                    Err(true) => Err(decompression_limit_error(
                        limit,
                        Some(limit.saturating_add(1)),
                    )),
                    Err(false) => {
                        if input.len() > limit {
                            Err(decompression_limit_error(limit, Some(input.len())))
                        } else {
                            Ok(input.to_vec())
                        }
                    }
                }
            } else if input.len() > limit {
                Err(decompression_limit_error(limit, Some(input.len())))
            } else {
                Ok(input.to_vec())
            }
        }
    }
}

fn bounded_ascii85_decode(input: &[u8], limit: usize) -> Result<Vec<u8>, OutputError> {
    let mut output = Vec::new();
    let mut group = [0u8; 5];
    let mut group_len = 0usize;
    let flush = |group: &[u8; 5], count: usize, output: &mut Vec<u8>| {
        let mut value = 0u32;
        for digit in group.iter().take(5) {
            value = value
                .saturating_mul(85)
                .saturating_add((*digit - b'!') as u32);
        }
        let bytes = value.to_be_bytes();
        let count = if count == 5 {
            4
        } else {
            count.saturating_sub(1)
        };
        output.extend_from_slice(&bytes[..count]);
    };

    let mut index = 0usize;
    while index < input.len() {
        let byte = input[index];
        index += 1;
        if byte.is_ascii_whitespace() {
            continue;
        }
        if byte == b'~' && input.get(index) == Some(&b'>') {
            break;
        }
        if byte == b'z' {
            if group_len != 0 {
                break;
            }
            if output.len().saturating_add(4) > limit {
                return Err(decompression_limit_error(
                    limit,
                    Some(limit.saturating_add(1)),
                ));
            }
            output.extend_from_slice(&[0; 4]);
            continue;
        }
        if !(b'!'..=b'u').contains(&byte) {
            break;
        }
        group[group_len] = byte;
        group_len += 1;
        if group_len == 5 {
            if output.len().saturating_add(4) > limit {
                return Err(decompression_limit_error(
                    limit,
                    Some(limit.saturating_add(1)),
                ));
            }
            flush(&group, group_len, &mut output);
            group_len = 0;
        }
    }
    if group_len > 0 {
        for digit in group.iter_mut().skip(group_len) {
            *digit = b'u';
        }
        let decoded_len = group_len.saturating_sub(1);
        if output.len().saturating_add(decoded_len) > limit {
            return Err(decompression_limit_error(
                limit,
                Some(limit.saturating_add(1)),
            ));
        }
        flush(&group, group_len, &mut output);
    }
    Ok(output)
}

struct BudgetWriter {
    output: Vec<u8>,
    limit: usize,
    exceeded: bool,
}

impl std::io::Write for BudgetWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.output.len().saturating_add(bytes.len()) > self.limit {
            self.exceeded = true;
            return Err(std::io::Error::new(
                std::io::ErrorKind::WriteZero,
                "decompression budget exceeded",
            ));
        }
        self.output.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn bounded_lzw_decode(stream: &Stream, input: &[u8], limit: usize) -> Result<Vec<u8>, OutputError> {
    let early_change = stream
        .dict
        .get(b"DecodeParms")
        .ok()
        .and_then(|object| object.as_dict().ok())
        .and_then(|params| params.get(b"EarlyChange").ok())
        .and_then(|object| object.as_i64().ok())
        .map(|value| value != 0)
        .unwrap_or(true);
    let mut decoder = if early_change {
        weezl::decode::Decoder::with_tiff_size_switch(weezl::BitOrder::Msb, 8)
    } else {
        weezl::decode::Decoder::new(weezl::BitOrder::Msb, 8)
    };
    let mut writer = BudgetWriter {
        output: Vec::new(),
        limit,
        exceeded: false,
    };
    let _ = decoder.into_stream(&mut writer).decode_all(input);
    if writer.exceeded {
        Err(decompression_limit_error(
            limit,
            Some(limit.saturating_add(1)),
        ))
    } else {
        Ok(writer.output)
    }
}

pub(crate) fn validate_document_budgets(
    document: &Document,
    options: &ExtractionOptions,
) -> Result<(), OutputError> {
    if let Some(limit) = options.max_pages {
        let pages = document.get_pages().len();
        if pages > limit {
            return Err(OutputError::resource_limit(
                ResourceLimitKind::Pages,
                limit,
                Some(pages),
            ));
        }
    }
    if let Some(limit) = options.max_objects {
        let indexed_objects = document
            .reference_table
            .entries
            .values()
            .filter(|entry| {
                matches!(
                    entry,
                    lopdf::xref::XrefEntry::Normal { .. }
                        | lopdf::xref::XrefEntry::Compressed { .. }
                )
            })
            .count();
        let objects = indexed_objects.max(document.objects.len());
        if objects > limit {
            return Err(OutputError::resource_limit(
                ResourceLimitKind::Objects,
                limit,
                Some(objects),
            ));
        }
    }
    if let Some(limit) = options.max_decompressed_stream_bytes {
        accounted_stream_bytes(document, limit)?;
    }
    Ok(())
}

pub(super) fn accounted_stream_bytes(document: &Document, limit: usize) -> Result<usize, OutputError> {
    let mut total = 0usize;
    for object in document.objects.values() {
        let Object::Stream(stream) = object else {
            continue;
        };
        let remaining = limit.saturating_sub(total);
        let decompressed = match bounded_decompressed_content(stream, remaining, limit) {
            Ok(decompressed) => decompressed,
            Err(OutputError::ResourceLimit {
                kind: ResourceLimitKind::DecompressedStreamBytes,
                observed,
                ..
            }) => {
                return Err(OutputError::resource_limit(
                    ResourceLimitKind::DecompressedStreamBytes,
                    limit,
                    observed.map(|value| total.saturating_add(value)),
                ));
            }
            Err(error) => return Err(error),
        };
        total = total.saturating_add(decompressed.len());
        if total > limit {
            return Err(OutputError::resource_limit(
                ResourceLimitKind::DecompressedStreamBytes,
                limit,
                Some(total),
            ));
        }
    }
    Ok(total)
}

