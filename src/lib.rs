use adobe_cmap_parser::{ByteMapping, CIDRange, CodeRange};
use encoding_rs::UTF_16BE;
use euclid::*;
use image::RgbImage;
use lopdf::content::Content;
use lopdf::encryption::DecryptionError;
use lopdf::*;

use ocrs::{ImageSource, OcrEngine, OcrEngineParams};
use ordered_float::OrderedFloat;
use rten::Model;
#[allow(unused)]
use rten_tensor::prelude::*;
use itertools::Itertools;
use std::fmt::{Debug, Formatter};

use euclid::vec2;
use rayon::prelude::*;
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::marker::PhantomData;
use std::rc::Rc;
use std::result::Result;
use std::slice::Iter;
use std::str;
use std::{fmt, path};
use unicode_normalization::UnicodeNormalization;

mod encodings;
use crate::form::form_fields;
mod core_fonts;

mod form;
mod glyphnames;
mod zapfglyphnames;

use lazy_static::lazy_static;
use regex::Regex;

lazy_static! {
    static ref NUMBERED_HEADING: Regex = Regex::new(
        r"(?x)
        ^
        (?P<number>
            (?:\d+\.)+\d+         | # Matches 1.1, 2.3.4, etc.
            [IVXLCDM]+\.          | # Matches VII.
            (?:Section|Article|Chapter)\s+[A-Z0-9]+ | # Matches Section 2, Article B, etc.
            \d+\s+[A-Z][A-Z\s]+      # Matches '1 UNITED STATES DISTRICT COURT SOUTHERN DISTRICT OF NEW YORK'
        )
       
        "
    ).unwrap();
}

pub struct Space;
pub type Transform = Transform2D<f64, Space, Space>;

// Constants for text extraction
const SPACE_THRESHOLD_RATIO: f64 = 0.25; // Typical space is 25% of font size
const MIN_SPACE_GAP: f64 = 0.1; // Minimum 10% of font size to be considered a gap
const PARAGRAPH_GAP_RATIO: f64 = 1.5; // Paragraph break if gap is 150% of font size
const LINE_HEIGHT_RATIO: f64 = 1.2; // Normal line height is ~120% of font size
const SAME_LINE_THRESHOLD: f64 = 0.5; // Consider same line if Y difference < 50% font size

// Helper function to determine if there's a space between characters
fn should_insert_space(current_x: f64, last_end: f64, font_size: f64) -> bool {
    let gap = current_x - last_end;
    // Use adaptive threshold based on font size
    // Smaller fonts tend to have tighter spacing
    let threshold = if font_size < 10.0 {
        font_size * MIN_SPACE_GAP
    } else {
        font_size * SPACE_THRESHOLD_RATIO
    };
    gap > threshold
}

// Helper function to determine if there's a paragraph break
fn is_paragraph_break(current_y: f64, last_y: f64, font_size: f64) -> bool {
    let y_gap = (current_y - last_y).abs();
    y_gap > font_size * PARAGRAPH_GAP_RATIO
}

// Helper function to determine if we've moved to a new line
fn is_new_line(current_x: f64, last_end: f64, current_y: f64, last_y: f64, font_size: f64) -> bool {
    let y_diff = (current_y - last_y).abs();
    // New line if we've moved down/up significantly and moved back to the left
    y_diff > font_size * SAME_LINE_THRESHOLD && current_x < last_end
}

// Add this struct to store OCR engine configuration
pub struct OcrConfig {
    pub detection_model: Option<String>,
    pub recognition_model: Option<String>,
}

// Create a struct to hold the OCR engine instance
pub struct OcrHandler {
    pub engine: OcrEngine,
}

impl OcrHandler {
    fn new(config: &OcrConfig) -> Result<Self, Box<dyn std::error::Error>> {
        let detection_model = if let Some(path) = &config.detection_model {
            // Convert the loaded model to the expected type
            Some(Model::load_file(path)?)
        } else {
            None
        };
        let recognition_model = if let Some(path) = &config.recognition_model {
            // Convert the loaded model to the expected type
            Some(Model::load_file(path)?)
        } else {
            None
        };

        let engine = OcrEngine::new(OcrEngineParams {
            detection_model,
            recognition_model,
            ..Default::default()
        })?;

        Ok(Self { engine })
    }
    fn process_image(&self, img: &RgbImage) -> Result<String, Box<dyn std::error::Error>> {
        let img_source = ImageSource::from_bytes(img.as_raw(), img.dimensions())?;
        let ocr_input = self.engine.prepare_input(img_source)?;

        // Get text with layout information
        let word_rects = self.engine.detect_words(&ocr_input)?;
        let line_rects = self.engine.find_text_lines(&ocr_input, &word_rects);
        let line_texts = self.engine.recognize_text(&ocr_input, &line_rects)?;

        // Combine all text into a single string
        let text: String = line_texts
            .iter()
            .flatten()
            .filter(|l| l.to_string().len() > 1)
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join(" ");

        Ok(text)
    }
}

#[derive(Debug)]
pub enum OutputError {
    FormatError(std::fmt::Error),
    IoError(std::io::Error),
    PdfError(lopdf::Error),
    Custom(String),
    Other(String), // Add this variant if it doesn't exist
}

impl std::fmt::Display for OutputError {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
        match self {
            OutputError::FormatError(e) => write!(f, "Formating error: {}", e),
            OutputError::IoError(e) => write!(f, "IO error: {}", e),
            OutputError::PdfError(e) => write!(f, "PDF error: {}", e),
            OutputError::Custom(e) => write!(f, "Custom error: {}", e),
            OutputError::Other(e) => write!(f, "Other error: {}", e),
        }
    }
}

impl std::error::Error for OutputError {}

impl From<std::fmt::Error> for OutputError {
    fn from(e: std::fmt::Error) -> Self {
        OutputError::FormatError(e)
    }
}

impl From<std::io::Error> for OutputError {
    fn from(e: std::io::Error) -> Self {
        OutputError::IoError(e)
    }
}

impl From<lopdf::Error> for OutputError {
    fn from(e: lopdf::Error) -> Self {
        OutputError::PdfError(e)
    }
}

macro_rules! dlog {
    ($($e:expr),*) => { {$(let _ = $e;)*} }
    //($($t:tt)*) => { println!($($t)*) }
}

fn get_info(doc: &Document) -> Option<&Dictionary> {
    match doc.trailer.get(b"Info") {
        Ok(&Object::Reference(ref id)) => match doc.get_object(*id) {
            Ok(&Object::Dictionary(ref info)) => {
                return Some(info);
            }
            _ => {}
        },
        _ => {}
    }
    None
}

fn get_catalog(doc: &Document) -> &Dictionary {
    match doc.trailer.get(b"Root").unwrap() {
        &Object::Reference(ref id) => match doc.get_object(*id) {
            Ok(&Object::Dictionary(ref catalog)) => {
                return catalog;
            }
            _ => {}
        },
        _ => {}
    }
    panic!();
}

fn get_pages(doc: &Document) -> &Dictionary {
    let catalog = get_catalog(doc);
    match catalog.get(b"Pages").unwrap() {
        &Object::Reference(ref id) => match doc.get_object(*id) {
            Ok(&Object::Dictionary(ref pages)) => {
                return pages;
            }
            other => {
                dlog!("pages: {:?}", other)
            }
        },
        other => {
            dlog!("pages: {:?}", other)
        }
    }
    dlog!("catalog {:?}", catalog);
    panic!();
}

#[allow(non_upper_case_globals)]
const PDFDocEncoding: &'static [u16] = &[
    0x0000, 0x0001, 0x0002, 0x0003, 0x0004, 0x0005, 0x0006, 0x0007, 0x0008, 0x0009, 0x000a, 0x000b,
    0x000c, 0x000d, 0x000e, 0x000f, 0x0010, 0x0011, 0x0012, 0x0013, 0x0014, 0x0015, 0x0016, 0x0017,
    0x02d8, 0x02c7, 0x02c6, 0x02d9, 0x02dd, 0x02db, 0x02da, 0x02dc, 0x0020, 0x0021, 0x0022, 0x0023,
    0x0024, 0x0025, 0x0026, 0x0027, 0x0028, 0x0029, 0x002a, 0x002b, 0x002c, 0x002d, 0x002e, 0x002f,
    0x0030, 0x0031, 0x0032, 0x0033, 0x0034, 0x0035, 0x0036, 0x0037, 0x0038, 0x0039, 0x003a, 0x003b,
    0x003c, 0x003d, 0x003e, 0x003f, 0x0040, 0x0041, 0x0042, 0x0043, 0x0044, 0x0045, 0x0046, 0x0047,
    0x0048, 0x0049, 0x004a, 0x004b, 0x004c, 0x004d, 0x004e, 0x004f, 0x0050, 0x0051, 0x0052, 0x0053,
    0x0054, 0x0055, 0x0056, 0x0057, 0x0058, 0x0059, 0x005a, 0x005b, 0x005c, 0x005d, 0x005e, 0x005f,
    0x0060, 0x0061, 0x0062, 0x0063, 0x0064, 0x0065, 0x0066, 0x0067, 0x0068, 0x0069, 0x006a, 0x006b,
    0x006c, 0x006d, 0x006e, 0x006f, 0x0070, 0x0071, 0x0072, 0x0073, 0x0074, 0x0075, 0x0076, 0x0077,
    0x0078, 0x0079, 0x007a, 0x007b, 0x007c, 0x007d, 0x007e, 0x0000, 0x2022, 0x2020, 0x2021, 0x2026,
    0x2014, 0x2013, 0x0192, 0x2044, 0x2039, 0x203a, 0x2212, 0x2030, 0x201e, 0x201c, 0x201d, 0x2018,
    0x2019, 0x201a, 0x2122, 0xfb01, 0xfb02, 0x0141, 0x0152, 0x0160, 0x0178, 0x017d, 0x0131, 0x0142,
    0x0153, 0x0161, 0x017e, 0x0000, 0x20ac, 0x00a1, 0x00a2, 0x00a3, 0x00a4, 0x00a5, 0x00a6, 0x00a7,
    0x00a8, 0x00a9, 0x00aa, 0x00ab, 0x00ac, 0x0000, 0x00ae, 0x00af, 0x00b0, 0x00b1, 0x00b2, 0x00b3,
    0x00b4, 0x00b5, 0x00b6, 0x00b7, 0x00b8, 0x00b9, 0x00ba, 0x00bb, 0x00bc, 0x00bd, 0x00be, 0x00bf,
    0x00c0, 0x00c1, 0x00c2, 0x00c3, 0x00c4, 0x00c5, 0x00c6, 0x00c7, 0x00c8, 0x00c9, 0x00ca, 0x00cb,
    0x00cc, 0x00cd, 0x00ce, 0x00cf, 0x00d0, 0x00d1, 0x00d2, 0x00d3, 0x00d4, 0x00d5, 0x00d6, 0x00d7,
    0x00d8, 0x00d9, 0x00da, 0x00db, 0x00dc, 0x00dd, 0x00de, 0x00df, 0x00e0, 0x00e1, 0x00e2, 0x00e3,
    0x00e4, 0x00e5, 0x00e6, 0x00e7, 0x00e8, 0x00e9, 0x00ea, 0x00eb, 0x00ec, 0x00ed, 0x00ee, 0x00ef,
    0x00f0, 0x00f1, 0x00f2, 0x00f3, 0x00f4, 0x00f5, 0x00f6, 0x00f7, 0x00f8, 0x00f9, 0x00fa, 0x00fb,
    0x00fc, 0x00fd, 0x00fe, 0x00ff,
];

fn pdf_to_utf8(s: &[u8]) -> String {
    if s.len() > 2 && s[0] == 0xfe && s[1] == 0xff {
        return UTF_16BE
            .decode_without_bom_handling_and_without_replacement(&s[2..])
            .unwrap()
            .to_string();
    } else {
        let r: Vec<u8> = s
            .iter()
            .map(|x| *x)
            .flat_map(|x| {
                let k = PDFDocEncoding[x as usize];
                vec![(k >> 8) as u8, k as u8].into_iter()
            })
            .collect();
        return UTF_16BE
            .decode_without_bom_handling_and_without_replacement(&r)
            .unwrap()
            .to_string();
    }
}

fn to_utf8(encoding: &[u16], s: &[u8]) -> String {
    if s.len() > 2 && s[0] == 0xfe && s[1] == 0xff {
        return UTF_16BE
            .decode_without_bom_handling_and_without_replacement(&s[2..])
            .unwrap()
            .to_string();
    } else {
        let r: Vec<u8> = s
            .iter()
            .map(|x| *x)
            .flat_map(|x| {
                let k = encoding[x as usize];
                vec![(k >> 8) as u8, k as u8].into_iter()
            })
            .collect();
        return UTF_16BE
            .decode_without_bom_handling_and_without_replacement(&r)
            .unwrap()
            .to_string();
    }
}

fn maybe_deref<'a>(doc: &'a Document, o: &'a Object) -> &'a Object {
    match o {
        &Object::Reference(r) => doc.get_object(r).expect("missing object reference"),
        _ => o,
    }
}

fn maybe_get_obj<'a>(doc: &'a Document, dict: &'a Dictionary, key: &[u8]) -> Option<&'a Object> {
    dict.get(key).map(|o| maybe_deref(doc, o)).ok()
}

// an intermediate trait that can be used to chain conversions that may have failed
trait FromOptObj<'a> {
    fn from_opt_obj(doc: &'a Document, obj: Option<&'a Object>, key: &[u8]) -> Self;
}

// conditionally convert to Self returns None if the conversion failed
trait FromObj<'a>
where
    Self: std::marker::Sized,
{
    fn from_obj(doc: &'a Document, obj: &'a Object) -> Option<Self>;
}

impl<'a, T: FromObj<'a>> FromOptObj<'a> for Option<T> {
    fn from_opt_obj(doc: &'a Document, obj: Option<&'a Object>, _key: &[u8]) -> Self {
        obj.and_then(|x| T::from_obj(doc, x))
    }
}

impl<'a, T: FromObj<'a>> FromOptObj<'a> for T {
    fn from_opt_obj(doc: &'a Document, obj: Option<&'a Object>, key: &[u8]) -> Self {
        T::from_obj(doc, obj.expect(&String::from_utf8_lossy(key))).expect("wrong type")
    }
}

// we follow the same conventions as pdfium for when to support indirect objects:
// on arrays, streams and dicts
impl<'a, T: FromObj<'a>> FromObj<'a> for Vec<T> {
    fn from_obj(doc: &'a Document, obj: &'a Object) -> Option<Self> {
        maybe_deref(doc, obj)
            .as_array()
            .map(|x| {
                x.iter()
                    .map(|x| T::from_obj(doc, x).expect("wrong type"))
                    .collect()
            })
            .ok()
    }
}

// XXX: These will panic if we don't have the right number of items
// we don't want to do that
impl<'a, T: FromObj<'a>> FromObj<'a> for [T; 4] {
    fn from_obj(doc: &'a Document, obj: &'a Object) -> Option<Self> {
        maybe_deref(doc, obj)
            .as_array()
            .map(|x| {
                let mut all = x.iter().map(|x| T::from_obj(doc, x).expect("wrong type"));
                [
                    all.next().unwrap(),
                    all.next().unwrap(),
                    all.next().unwrap(),
                    all.next().unwrap(),
                ]
            })
            .ok()
    }
}

impl<'a, T: FromObj<'a>> FromObj<'a> for [T; 3] {
    fn from_obj(doc: &'a Document, obj: &'a Object) -> Option<Self> {
        maybe_deref(doc, obj)
            .as_array()
            .map(|x| {
                let mut all = x.iter().map(|x| T::from_obj(doc, x).expect("wrong type"));
                [
                    all.next().unwrap(),
                    all.next().unwrap(),
                    all.next().unwrap(),
                ]
            })
            .ok()
    }
}

impl<'a> FromObj<'a> for f64 {
    fn from_obj(_doc: &Document, obj: &Object) -> Option<Self> {
        match obj {
            &Object::Integer(i) => Some(i as f64),
            &Object::Real(f) => Some(f.into()),
            _ => None,
        }
    }
}

impl<'a> FromObj<'a> for i64 {
    fn from_obj(_doc: &Document, obj: &Object) -> Option<Self> {
        match obj {
            &Object::Integer(i) => Some(i),
            _ => None,
        }
    }
}

impl<'a> FromObj<'a> for &'a Dictionary {
    fn from_obj(doc: &'a Document, obj: &'a Object) -> Option<&'a Dictionary> {
        maybe_deref(doc, obj).as_dict().ok()
    }
}

impl<'a> FromObj<'a> for &'a Stream {
    fn from_obj(doc: &'a Document, obj: &'a Object) -> Option<&'a Stream> {
        maybe_deref(doc, obj).as_stream().ok()
    }
}

impl<'a> FromObj<'a> for &'a Object {
    fn from_obj(doc: &'a Document, obj: &'a Object) -> Option<&'a Object> {
        Some(maybe_deref(doc, obj))
    }
}

fn get<'a, T: FromOptObj<'a>>(doc: &'a Document, dict: &'a Dictionary, key: &[u8]) -> T {
    T::from_opt_obj(doc, dict.get(key).ok(), key)
}

fn maybe_get<'a, T: FromObj<'a>>(doc: &'a Document, dict: &'a Dictionary, key: &[u8]) -> Option<T> {
    maybe_get_obj(doc, dict, key).and_then(|o| T::from_obj(doc, o))
}

fn get_name_string<'a>(doc: &'a Document, dict: &'a Dictionary, key: &[u8]) -> String {
    pdf_to_utf8(
        dict.get(key)
            .map(|o| maybe_deref(doc, o))
            .unwrap_or_else(|_| panic!("deref"))
            .as_name()
            .expect("name"),
    )
}

#[allow(dead_code)]
fn maybe_get_name_string<'a>(
    doc: &'a Document,
    dict: &'a Dictionary,
    key: &[u8],
) -> Option<String> {
    maybe_get_obj(doc, dict, key)
        .and_then(|n| n.as_name().ok())
        .map(|n| pdf_to_utf8(n))
}

fn maybe_get_name<'a>(doc: &'a Document, dict: &'a Dictionary, key: &[u8]) -> Option<&'a [u8]> {
    maybe_get_obj(doc, dict, key).and_then(|n| n.as_name().ok())
}
fn maybe_get_array<'a>(
    doc: &'a Document,
    dict: &'a Dictionary,
    key: &[u8],
) -> Option<&'a Vec<Object>> {
    maybe_get_obj(doc, dict, key).and_then(|n| n.as_array().ok())
}

#[derive(Clone)]
struct PdfSimpleFont<'a> {
    font: &'a Dictionary,
    doc: &'a Document,
    encoding: Option<Vec<u16>>,
    unicode_map: Option<HashMap<u32, String>>,
    widths: HashMap<CharCode, f64>, // should probably just use i32 here
    missing_width: f64,
}

#[derive(Clone)]
struct PdfType3Font<'a> {
    font: &'a Dictionary,
    doc: &'a Document,
    encoding: Option<Vec<u16>>,
    unicode_map: Option<HashMap<u32, String>>,
    widths: HashMap<CharCode, f64>, // should probably just use i32 here
}

fn make_font<'a>(doc: &'a Document, font: &'a Dictionary) -> Rc<dyn PdfFont + 'a> {
    let subtype = get_name_string(doc, font, b"Subtype");
    dlog!("MakeFont({})", subtype);
    if subtype == "Type0" {
        Rc::new(PdfCIDFont::new(doc, font))
    } else if subtype == "Type3" {
        Rc::new(PdfType3Font::new(doc, font))
    } else {
        Rc::new(PdfSimpleFont::new(doc, font))
    }
}

fn is_core_font(name: &str) -> bool {
    match name {
        "Courier-Bold"
        | "Courier-BoldOblique"
        | "Courier-Oblique"
        | "Courier"
        | "Helvetica-Bold"
        | "Helvetica-BoldOblique"
        | "Helvetica-Oblique"
        | "Helvetica"
        | "Symbol"
        | "Times-Bold"
        | "Times-BoldItalic"
        | "Times-Italic"
        | "Times-Roman"
        | "ZapfDingbats" => true,
        _ => false,
    }
}

fn encoding_to_unicode_table(name: &[u8]) -> Vec<u16> {
    let encoding = match &name[..] {
        b"MacRomanEncoding" => encodings::MAC_ROMAN_ENCODING,
        b"MacExpertEncoding" => encodings::MAC_EXPERT_ENCODING,
        b"WinAnsiEncoding" => encodings::WIN_ANSI_ENCODING,
        _ => panic!("unexpected encoding {:?}", pdf_to_utf8(name)),
    };
    let encoding_table = encoding
        .iter()
        .map(|x| {
            if let &Some(x) = x {
                glyphnames::name_to_unicode(x).unwrap()
            } else {
                0
            }
        })
        .collect();
    encoding_table
}

/* "Glyphs in the font are selected by single-byte character codes obtained from a string that
    is shown by the text-showing operators. Logically, these codes index into a table of 256
    glyphs; the mapping from codes to glyphs is called the font's encoding. Each font program
    has a built-in encoding. Under some circumstances, the encoding can be altered by means
    described in Section 5.5.5, "Character Encoding."
*/
impl<'a> PdfSimpleFont<'a> {
    fn new(doc: &'a Document, font: &'a Dictionary) -> PdfSimpleFont<'a> {
        let base_name = get_name_string(doc, font, b"BaseFont");
        let subtype = get_name_string(doc, font, b"Subtype");

        let encoding: Option<&Object> = get(doc, font, b"Encoding");
        dlog!(
            "base_name {} {} enc:{:?} {:?}",
            base_name,
            subtype,
            encoding,
            font
        );
        let descriptor: Option<&Dictionary> = get(doc, font, b"FontDescriptor");
        let mut type1_encoding = None;
        if let Some(descriptor) = descriptor {
            dlog!("descriptor {:?}", descriptor);
            if subtype == "Type1" {
                let file = maybe_get_obj(doc, descriptor, b"FontFile");
                match file {
                    Some(&Object::Stream(ref s)) => {
                        let s = get_contents(s);
                        //dlog!("font contents {:?}", pdf_to_utf8(&s));
                        type1_encoding =
                            Some(type1_encoding_parser::get_encoding_map(&s).expect("encoding"));
                    }
                    _ => {
                        dlog!("font file {:?}", file)
                    }
                }
            } else if subtype == "TrueType" {
                let file = maybe_get_obj(doc, descriptor, b"FontFile2");
                match file {
                    Some(&Object::Stream(ref s)) => {
                        let _s = get_contents(s);
                        //File::create(format!("/tmp/{}", base_name)).unwrap().write_all(&s);
                    }
                    _ => {
                        dlog!("font file {:?}", file)
                    }
                }
            }

            let font_file3 = get::<Option<&Object>>(doc, descriptor, b"FontFile3");
            match font_file3 {
                Some(&Object::Stream(ref s)) => {
                    let subtype = get_name_string(doc, &s.dict, b"Subtype");
                    dlog!("font file {}, {:?}", subtype, s);
                }
                None => {}
                _ => {
                    dlog!("unexpected")
                }
            }

            let charset = maybe_get_obj(doc, descriptor, b"CharSet");
            let _charset = match charset {
                Some(&Object::String(ref s, _)) => Some(pdf_to_utf8(&s)),
                _ => None,
            };
            //dlog!("charset {:?}", charset);
        }

        let mut unicode_map = get_unicode_map(doc, font);

        let mut encoding_table = None;
        match encoding {
            Some(&Object::Name(ref encoding_name)) => {
                dlog!("encoding {:?}", pdf_to_utf8(encoding_name));
                encoding_table = Some(encoding_to_unicode_table(encoding_name));
            }
            Some(&Object::Dictionary(ref encoding)) => {
                //dlog!("Encoding {:?}", encoding);
                let mut table =
                    if let Some(base_encoding) = maybe_get_name(doc, encoding, b"BaseEncoding") {
                        dlog!("BaseEncoding {:?}", base_encoding);
                        encoding_to_unicode_table(base_encoding)
                    } else {
                        Vec::from(PDFDocEncoding)
                    };
                let differences = maybe_get_array(doc, encoding, b"Differences");
                if let Some(differences) = differences {
                    dlog!("Differences");
                    let mut code = 0;
                    for o in differences {
                        let o = maybe_deref(doc, o);
                        match o {
                            &Object::Integer(i) => {
                                code = i;
                            }
                            &Object::Name(ref n) => {
                                let name = pdf_to_utf8(&n);
                                // XXX: names of Type1 fonts can map to arbitrary strings instead of real
                                // unicode names, so we should probably handle this differently
                                let unicode = glyphnames::name_to_unicode(&name);
                                if let Some(unicode) = unicode {
                                    table[code as usize] = unicode;
                                    if let Some(ref mut unicode_map) = unicode_map {
                                        let be = [unicode];
                                        match unicode_map.entry(code as u32) {
                                            // If there's a unicode table entry missing use one based on the name
                                            Entry::Vacant(v) => {
                                                v.insert(String::from_utf16(&be).unwrap());
                                            }
                                            Entry::Occupied(e) => {
                                                if e.get() != &String::from_utf16(&be).unwrap() {
                                                    let normal_match =
                                                        e.get().nfkc().eq(String::from_utf16(&be)
                                                            .unwrap()
                                                            .nfkc());
                                                    // println!(
                                                    //     "Unicode mismatch {} {} {:?} {:?} {:?}",
                                                    //     normal_match,
                                                    //     name,
                                                    //     e.get(),
                                                    //     String::from_utf16(&be),
                                                    //     be
                                                    // );
                                                }
                                            }
                                        }
                                    }
                                } else {
                                    match unicode_map {
                                        Some(ref mut unicode_map)
                                            if base_name.contains("FontAwesome") =>
                                        {
                                            // the fontawesome tex package will use glyph names that don't have a corresponding unicode
                                            // code point, so we'll use an empty string instead. See issue #76
                                            match unicode_map.entry(code as u32) {
                                                Entry::Vacant(v) => {
                                                    v.insert("".to_owned());
                                                }
                                                Entry::Occupied(e) => {
                                                    panic!("unexpected entry in unicode map")
                                                }
                                            }
                                        }
                                        _ => {
                                            println!(
                                                "unknown glyph name '{}' for font {}",
                                                name, base_name
                                            );
                                        }
                                    }
                                }
                                dlog!("{} = {} ({:?})", code, name, unicode);
                                if let Some(ref mut unicode_map) = unicode_map {
                                    // The unicode map might not have the code in it, but the code might
                                    // not be used so we don't want to panic here.
                                    // An example of this is the 'suppress' character in the TeX Latin Modern font.
                                    // This shows up in https://arxiv.org/pdf/2405.01295v1.pdf
                                    dlog!("{} {:?}", code, unicode_map.get(&(code as u32)));
                                }
                                code += 1;
                            }
                            _ => {
                                panic!("wrong type {:?}", o);
                            }
                        }
                    }
                }
                let name = encoding
                    .get(b"Type")
                    .and_then(|x| x.as_name())
                    .and_then(|x| Ok(pdf_to_utf8(x)));
                dlog!("name: {}", name);

                encoding_table = Some(table);
            }
            None => {
                if let Some(type1_encoding) = type1_encoding {
                    let mut table = Vec::from(PDFDocEncoding);
                    dlog!("type1encoding");
                    for (code, name) in type1_encoding {
                        let unicode = glyphnames::name_to_unicode(&pdf_to_utf8(&name));
                        if let Some(unicode) = unicode {
                            table[code as usize] = unicode;
                        } else {
                            dlog!("unknown character {}", pdf_to_utf8(&name));
                        }
                    }
                    encoding_table = Some(table)
                } else if subtype == "TrueType" {
                    encoding_table = Some(
                        encodings::WIN_ANSI_ENCODING
                            .iter()
                            .map(|x| {
                                if let &Some(x) = x {
                                    glyphnames::name_to_unicode(x).unwrap()
                                } else {
                                    0
                                }
                            })
                            .collect(),
                    );
                }
            }
            _ => {
                dlog!("unknown encoding {:?}", encoding);
            }
        }

        let mut width_map = HashMap::new();
        /* "Ordinarily, a font dictionary that refers to one of the standard fonts
        should omit the FirstChar, LastChar, Widths, and FontDescriptor entries.
        However, it is permissible to override a standard font by including these
        entries and embedding the font program in the PDF file."

        Note: some PDFs include a descriptor but still don't include these entries */

        // If we have widths prefer them over the core font widths. Needed for https://dkp.de/wp-content/uploads/parteitage/Sozialismusvorstellungen-der-DKP.pdf
        if let (Some(first_char), Some(last_char), Some(widths)) = (
            maybe_get::<i64>(doc, font, b"FirstChar"),
            maybe_get::<i64>(doc, font, b"LastChar"),
            maybe_get::<Vec<f64>>(doc, font, b"Widths"),
        ) {
            // Some PDF's don't have these like fips-197.pdf
            let mut i: i64 = 0;
            dlog!(
                "first_char {:?}, last_char: {:?}, widths: {} {:?}",
                first_char,
                last_char,
                widths.len(),
                widths
            );

            for w in widths {
                width_map.insert((first_char + i) as CharCode, w);
                i += 1;
            }
            assert_eq!(first_char + i - 1, last_char);
        } else if is_core_font(&base_name) {
            for font_metrics in core_fonts::metrics().iter() {
                if font_metrics.0 == base_name {
                    if let Some(ref encoding) = encoding_table {
                        dlog!("has encoding");
                        for w in font_metrics.2 {
                            let c = glyphnames::name_to_unicode(w.2).unwrap();
                            for i in 0..encoding.len() {
                                if encoding[i] == c {
                                    width_map.insert(i as CharCode, w.1 as f64);
                                }
                            }
                        }
                    } else {
                        // Instead of using the encoding from the core font we'll just look up all
                        // of the character names. We should probably verify that this produces the
                        // same result.

                        let mut table = vec![0; 256];
                        for w in font_metrics.2 {
                            dlog!("{} {}", w.0, w.2);
                            // -1 is "not encoded"
                            if w.0 != -1 {
                                table[w.0 as usize] = if base_name == "ZapfDingbats" {
                                    zapfglyphnames::zapfdigbats_names_to_unicode(w.2)
                                        .unwrap_or_else(|| panic!("bad name {:?}", w))
                                } else {
                                    glyphnames::name_to_unicode(w.2).unwrap()
                                }
                            }
                        }

                        let encoding = &table[..];
                        for w in font_metrics.2 {
                            width_map.insert(w.0 as CharCode, w.1 as f64);
                            // -1 is "not encoded"
                        }
                        encoding_table = Some(encoding.to_vec());
                    }
                    /* "Ordinarily, a font dictionary that refers to one of the standard fonts
                    should omit the FirstChar, LastChar, Widths, and FontDescriptor entries.
                    However, it is permissible to override a standard font by including these
                    entries and embedding the font program in the PDF file."

                    Note: some PDFs include a descriptor but still don't include these entries */
                    // assert!(maybe_get_obj(doc, font, b"FirstChar").is_none());
                    // assert!(maybe_get_obj(doc, font, b"LastChar").is_none());
                    // assert!(maybe_get_obj(doc, font, b"Widths").is_none());
                }
            }
        } else {
            panic!("no widths");
        }

        let missing_width = get::<Option<f64>>(doc, font, b"MissingWidth").unwrap_or(0.);
        PdfSimpleFont {
            doc,
            font,
            widths: width_map,
            encoding: encoding_table,
            missing_width,
            unicode_map,
        }
    }

    #[allow(dead_code)]
    fn get_type(&self) -> String {
        get_name_string(self.doc, self.font, b"Type")
    }
    #[allow(dead_code)]
    fn get_basefont(&self) -> String {
        get_name_string(self.doc, self.font, b"BaseFont")
    }
    #[allow(dead_code)]
    fn get_subtype(&self) -> String {
        get_name_string(self.doc, self.font, b"Subtype")
    }
    #[allow(dead_code)]
    fn get_widths(&self) -> Option<&Vec<Object>> {
        maybe_get_obj(self.doc, self.font, b"Widths")
            .map(|widths| widths.as_array().expect("Widths should be an array"))
    }
    /* For type1: This entry is obsolescent and its use is no longer recommended. (See
     * implementation note 42 in Appendix H.) */
    #[allow(dead_code)]
    fn get_name(&self) -> Option<String> {
        maybe_get_name_string(self.doc, self.font, b"Name")
    }

    #[allow(dead_code)]
    fn get_descriptor(&self) -> Option<PdfFontDescriptor> {
        maybe_get_obj(self.doc, self.font, b"FontDescriptor")
            .and_then(|desc| desc.as_dict().ok())
            .map(|desc| PdfFontDescriptor {
                desc: desc,
                doc: self.doc,
            })
    }
}
impl<'a> PdfType3Font<'a> {
    fn new(doc: &'a Document, font: &'a Dictionary) -> PdfType3Font<'a> {
        let unicode_map = get_unicode_map(doc, font);
        let encoding: Option<&Object> = get(doc, font, b"Encoding");

        let encoding_table;
        match encoding {
            Some(&Object::Name(ref encoding_name)) => {
                dlog!("encoding {:?}", pdf_to_utf8(encoding_name));
                encoding_table = Some(encoding_to_unicode_table(encoding_name));
            }
            Some(&Object::Dictionary(ref encoding)) => {
                //dlog!("Encoding {:?}", encoding);
                let mut table =
                    if let Some(base_encoding) = maybe_get_name(doc, encoding, b"BaseEncoding") {
                        dlog!("BaseEncoding {:?}", base_encoding);
                        encoding_to_unicode_table(base_encoding)
                    } else {
                        Vec::from(PDFDocEncoding)
                    };
                let differences = maybe_get_array(doc, encoding, b"Differences");
                if let Some(differences) = differences {
                    dlog!("Differences");
                    let mut code = 0;
                    for o in differences {
                        match o {
                            &Object::Integer(i) => {
                                code = i;
                            }
                            &Object::Name(ref n) => {
                                let name = pdf_to_utf8(&n);
                                // XXX: names of Type1 fonts can map to arbitrary strings instead of real
                                // unicode names, so we should probably handle this differently
                                let unicode = glyphnames::name_to_unicode(&name);
                                if let Some(unicode) = unicode {
                                    table[code as usize] = unicode;
                                }
                                dlog!("{} = {} ({:?})", code, name, unicode);
                                if let Some(ref unicode_map) = unicode_map {
                                    dlog!("{} {:?}", code, unicode_map.get(&(code as u32)));
                                }
                                code += 1;
                            }
                            _ => {
                                panic!("wrong type");
                            }
                        }
                    }
                }
                let name_encoded = encoding.get(b"Type");
                if let Ok(Object::Name(name)) = name_encoded {
                    dlog!("name: {}", pdf_to_utf8(name));
                } else {
                    dlog!("name not found");
                }

                encoding_table = Some(table);
            }
            _ => {
                panic!()
            }
        }

        let first_char: i64 = get(doc, font, b"FirstChar");
        let last_char: i64 = get(doc, font, b"LastChar");
        let widths: Vec<f64> = get(doc, font, b"Widths");

        let mut width_map = HashMap::new();

        let mut i = 0;
        dlog!(
            "first_char {:?}, last_char: {:?}, widths: {} {:?}",
            first_char,
            last_char,
            widths.len(),
            widths
        );

        for w in widths {
            width_map.insert((first_char + i) as CharCode, w);
            i += 1;
        }
        assert_eq!(first_char + i - 1, last_char);
        PdfType3Font {
            doc,
            font,
            widths: width_map,
            encoding: encoding_table,
            unicode_map,
        }
    }
}

type CharCode = u32;

struct PdfFontIter<'a> {
    i: Iter<'a, u8>,
    font: &'a dyn PdfFont,
}

impl<'a> Iterator for PdfFontIter<'a> {
    type Item = (CharCode, u8);
    fn next(&mut self) -> Option<(CharCode, u8)> {
        self.font.next_char(&mut self.i)
    }
}

trait PdfFont: Debug {
    fn get_width(&self, id: CharCode) -> f64;
    fn next_char(&self, iter: &mut Iter<u8>) -> Option<(CharCode, u8)>;
    fn decode_char(&self, char: CharCode) -> String;

    /*fn char_codes<'a>(&'a self, chars: &'a [u8]) -> PdfFontIter {
        let p = self;
        PdfFontIter{i: chars.iter(), font: p as &PdfFont}
    }*/
}

impl<'a> dyn PdfFont + 'a {
    fn char_codes(&'a self, chars: &'a [u8]) -> PdfFontIter {
        PdfFontIter {
            i: chars.iter(),
            font: self,
        }
    }
    fn decode(&self, chars: &[u8]) -> String {
        let strings = self
            .char_codes(chars)
            .map(|x| self.decode_char(x.0))
            .collect::<Vec<_>>();
        strings.join("")
    }
}

impl<'a> PdfFont for PdfSimpleFont<'a> {
    fn get_width(&self, id: CharCode) -> f64 {
        let width = self.widths.get(&id);
        if let Some(width) = width {
            return *width;
        } else {
            let mut widths = self.widths.iter().collect::<Vec<_>>();
            widths.sort_by_key(|x| x.0);
            dlog!(
                "missing width for {} len(widths) = {}, {:?} falling back to missing_width {:?}",
                id,
                self.widths.len(),
                widths,
                self.font
            );
            return self.missing_width;
        }
    }
    /*fn decode(&self, chars: &[u8]) -> String {
        let encoding = self.encoding.as_ref().map(|x| &x[..]).unwrap_or(&PDFDocEncoding);
        to_utf8(encoding, chars)
    }*/

    fn next_char(&self, iter: &mut Iter<u8>) -> Option<(CharCode, u8)> {
        iter.next().map(|x| (*x as CharCode, 1))
    }
    fn decode_char(&self, char: CharCode) -> String {
        let slice = [char as u8];
        if let Some(ref unicode_map) = self.unicode_map {
            let s = unicode_map.get(&char);
            let s = match s {
                None => {
                    println!(
                        "missing char {:?} in unicode map {:?} for {:?}",
                        char, unicode_map, self.font
                    );
                    // some pdf's like http://arxiv.org/pdf/2312.00064v1 are missing entries in their unicode map but do have
                    // entries in the encoding.
                    let encoding = self
                        .encoding
                        .as_ref()
                        .map(|x| &x[..])
                        .expect("missing unicode map and encoding");
                    let s = to_utf8(encoding, &slice);
                    println!("falling back to encoding {} -> {:?}", char, s);
                    s
                }
                Some(s) => s.clone(),
            };
            return s;
        }
        let encoding = self
            .encoding
            .as_ref()
            .map(|x| &x[..])
            .unwrap_or(&PDFDocEncoding);
        //dlog!("char_code {:?} {:?}", char, self.encoding);
        let s = to_utf8(encoding, &slice);
        s
    }
}

impl<'a> fmt::Debug for PdfSimpleFont<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.font.fmt(f)
    }
}

impl<'a> PdfFont for PdfType3Font<'a> {
    fn get_width(&self, id: CharCode) -> f64 {
        let width = self.widths.get(&id);
        if let Some(width) = width {
            return *width;
        } else {
            panic!("missing width for {} {:?}", id, self.font);
        }
    }
    /*fn decode(&self, chars: &[u8]) -> String {
        let encoding = self.encoding.as_ref().map(|x| &x[..]).unwrap_or(&PDFDocEncoding);
        to_utf8(encoding, chars)
    }*/

    fn next_char(&self, iter: &mut Iter<u8>) -> Option<(CharCode, u8)> {
        iter.next().map(|x| (*x as CharCode, 1))
    }
    fn decode_char(&self, char: CharCode) -> String {
        let slice = [char as u8];
        if let Some(ref unicode_map) = self.unicode_map {
            let s = unicode_map.get(&char);
            let s = match s {
                None => {
                    panic!("missing char {:?} in map {:?}", char, unicode_map)
                }
                Some(s) => s.clone(),
            };
            return s;
        }
        let encoding = self
            .encoding
            .as_ref()
            .map(|x| &x[..])
            .unwrap_or(&PDFDocEncoding);
        //dlog!("char_code {:?} {:?}", char, self.encoding);
        let s = to_utf8(encoding, &slice);
        s
    }
}

impl<'a> fmt::Debug for PdfType3Font<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.font.fmt(f)
    }
}

struct PdfCIDFont<'a> {
    font: &'a Dictionary,
    #[allow(dead_code)]
    doc: &'a Document,
    #[allow(dead_code)]
    encoding: ByteMapping,
    to_unicode: Option<HashMap<u32, String>>,
    widths: HashMap<CharCode, f64>, // should probably just use i32 here
    default_width: Option<f64>, // only used for CID fonts and we should probably brake out the different font types
}

fn get_unicode_map<'a>(doc: &'a Document, font: &'a Dictionary) -> Option<HashMap<u32, String>> {
    let to_unicode = maybe_get_obj(doc, font, b"ToUnicode");
    dlog!("ToUnicode: {:?}", to_unicode);
    let mut unicode_map = None;
    match to_unicode {
        Some(&Object::Stream(ref stream)) => {
            let contents = get_contents(stream);
            dlog!("Stream: {}", String::from_utf8(contents.clone()).unwrap());

            let cmap = adobe_cmap_parser::get_unicode_map(&contents).unwrap();
            let mut unicode = HashMap::new();
            // "It must use the beginbfchar, endbfchar, beginbfrange, and endbfrange operators to
            // define the mapping from character codes to Unicode character sequences expressed in
            // UTF-16BE encoding."
            for (&k, v) in cmap.iter() {
                let mut be: Vec<u16> = Vec::new();
                let mut i = 0;
                assert!(v.len() % 2 == 0);
                while i < v.len() {
                    be.push(((v[i] as u16) << 8) | v[i + 1] as u16);
                    i += 2;
                }
                match &be[..] {
                    [0xd800..=0xdfff] => {
                        // this range is not specified as not being encoded
                        // we ignore them so we don't an error from from_utt16
                        continue;
                    }
                    _ => {}
                }
                let s = String::from_utf16(&be).unwrap();

                unicode.insert(k, s);
            }
            unicode_map = Some(unicode);

            dlog!("map: {:?}", unicode_map);
        }
        None => {}
        Some(&Object::Name(ref name)) => {
            let name = pdf_to_utf8(name);
            if name != "Identity-H" {
                todo!("unsupported ToUnicode name: {:?}", name);
            }
        }
        _ => {
            panic!("unsupported cmap {:?}", to_unicode)
        }
    }
    unicode_map
}

impl<'a> PdfCIDFont<'a> {
    fn new(doc: &'a Document, font: &'a Dictionary) -> PdfCIDFont<'a> {
        let base_name = get_name_string(doc, font, b"BaseFont");
        let descendants =
            maybe_get_array(doc, font, b"DescendantFonts").expect("Descendant fonts required");
        let ciddict = maybe_deref(doc, &descendants[0])
            .as_dict()
            .expect("should be CID dict");
        let encoding =
            maybe_get_obj(doc, font, b"Encoding").expect("Encoding required in type0 fonts");
        dlog!("base_name {} {:?}", base_name, font);

        let encoding = match encoding {
            &Object::Name(ref name) => {
                let name = pdf_to_utf8(name);
                dlog!("encoding {:?}", name);
                assert!(name == "Identity-H");
                ByteMapping {
                    codespace: vec![CodeRange {
                        width: 2,
                        start: 0,
                        end: 0xffff,
                    }],
                    cid: vec![CIDRange {
                        src_code_lo: 0,
                        src_code_hi: 0xffff,
                        dst_CID_lo: 0,
                    }],
                }
            }
            &Object::Stream(ref stream) => {
                let contents = get_contents(stream);
                dlog!("Stream: {}", String::from_utf8(contents.clone()).unwrap());
                adobe_cmap_parser::get_byte_mapping(&contents).unwrap()
            }
            _ => {
                panic!("unsupported encoding {:?}", encoding)
            }
        };

        // Sometimes a Type0 font might refer to the same underlying data as regular font. In this case we may be able to extract some encoding
        // data.
        // We should also look inside the truetype data to see if there's a cmap table. It will help us convert as well.
        // This won't work if the cmap has been subsetted. A better approach might be to hash glyph contents and use that against
        // a global library of glyph hashes
        let unicode_map = get_unicode_map(doc, font);

        dlog!("descendents {:?} {:?}", descendants, ciddict);

        let font_dict = maybe_get_obj(doc, ciddict, b"FontDescriptor").expect("required");
        dlog!("{:?}", font_dict);
        let _f = font_dict.as_dict().expect("must be dict");
        let default_width = get::<Option<i64>>(doc, ciddict, b"DW").unwrap_or(1000);
        let w: Option<Vec<&Object>> = get(doc, ciddict, b"W");
        dlog!("widths {:?}", w);
        let mut widths = HashMap::new();
        let mut i = 0;
        if let Some(w) = w {
            while i < w.len() {
                if let &Object::Array(ref wa) = w[i + 1] {
                    let cid = w[i].as_i64().expect("id should be num");
                    let mut j = 0;
                    dlog!("wa: {:?} -> {:?}", cid, wa);
                    for w in wa {
                        widths.insert((cid + j) as CharCode, as_num(w));
                        j += 1;
                    }
                    i += 2;
                } else {
                    let c_first = w[i].as_i64().expect("first should be num");
                    let c_last = w[i].as_i64().expect("last should be num");
                    let c_width = as_num(&w[i]);
                    for id in c_first..c_last {
                        widths.insert(id as CharCode, c_width);
                    }
                    i += 3;
                }
            }
        }
        PdfCIDFont {
            doc,
            font,
            widths,
            to_unicode: unicode_map,
            encoding,
            default_width: Some(default_width as f64),
        }
    }
}

impl<'a> PdfFont for PdfCIDFont<'a> {
    fn get_width(&self, id: CharCode) -> f64 {
        let width = self.widths.get(&id);
        if let Some(width) = width {
            dlog!("GetWidth {} -> {}", id, *width);
            return *width;
        } else {
            dlog!("missing width for {} falling back to default_width", id);
            return self.default_width.unwrap();
        }
    } /*
      fn decode(&self, chars: &[u8]) -> String {
          self.char_codes(chars);

          //let utf16 = Vec::new();

          let encoding = self.encoding.as_ref().map(|x| &x[..]).unwrap_or(&PDFDocEncoding);
          to_utf8(encoding, chars)
      }*/

    fn next_char(&self, iter: &mut Iter<u8>) -> Option<(CharCode, u8)> {
        let mut c = *iter.next()? as u32;
        let mut code = None;
        'outer: for width in 1..=4 {
            for range in &self.encoding.codespace {
                if c as u32 >= range.start && c as u32 <= range.end && range.width == width {
                    code = Some((c as u32, width));
                    break 'outer;
                }
            }
            let next = *iter.next()?;
            c = ((c as u32) << 8) | next as u32;
        }
        let code = code?;
        for range in &self.encoding.cid {
            if code.0 >= range.src_code_lo && code.0 <= range.src_code_hi {
                return Some((code.0 + range.dst_CID_lo, code.1 as u8));
            }
        }
        None
    }
    fn decode_char(&self, char: CharCode) -> String {
        let s = self.to_unicode.as_ref().and_then(|x| x.get(&char));
        if let Some(s) = s {
            s.clone()
        } else {
            dlog!(
                "Unknown character {:?} in {:?} {:?}",
                char,
                self.font,
                self.to_unicode
            );
            "".to_string()
        }
    }
}

impl<'a> fmt::Debug for PdfCIDFont<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.font.fmt(f)
    }
}

#[derive(Copy, Clone)]
struct PdfFontDescriptor<'a> {
    desc: &'a Dictionary,
    doc: &'a Document,
}

impl<'a> PdfFontDescriptor<'a> {
    #[allow(dead_code)]
    fn get_file(&self) -> Option<&'a Object> {
        maybe_get_obj(self.doc, self.desc, b"FontFile")
    }
}

impl<'a> fmt::Debug for PdfFontDescriptor<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.desc.fmt(f)
    }
}

#[derive(Clone, Debug)]
struct Type0Func {
    domain: Vec<f64>,
    range: Vec<f64>,
    contents: Vec<u8>,
    size: Vec<i64>,
    bits_per_sample: i64,
    encode: Vec<f64>,
    decode: Vec<f64>,
}

#[allow(dead_code)]
fn interpolate(x: f64, x_min: f64, _x_max: f64, y_min: f64, y_max: f64) -> f64 {
    let divisor = x - x_min;
    if divisor != 0. {
        y_min + (x - x_min) * ((y_max - y_min) / divisor)
    } else {
        // (x - x_min) will be 0 which means we want to discard the interpolation
        // and arbitrarily choose y_min to match pdfium
        y_min
    }
}

impl Type0Func {
    #[allow(dead_code)]
    fn eval(&self, _input: &[f64], _output: &mut [f64]) {
        let _n_inputs = self.domain.len() / 2;
        let _n_ouputs = self.range.len() / 2;
    }
}

#[derive(Clone, Debug)]
struct Type2Func {
    c0: Option<Vec<f64>>,
    c1: Option<Vec<f64>>,
    n: f64,
}

#[derive(Clone, Debug)]
enum Function {
    Type0(Type0Func),
    Type2(Type2Func),
    #[allow(dead_code)]
    Type3,
    #[allow(dead_code)]
    Type4,
}

impl Function {
    fn new(doc: &Document, obj: &Object) -> Function {
        let dict = match obj {
            &Object::Dictionary(ref dict) => dict,
            &Object::Stream(ref stream) => &stream.dict,
            _ => panic!(),
        };
        let function_type: i64 = get(doc, dict, b"FunctionType");
        let f = match function_type {
            0 => {
                let stream = match obj {
                    &Object::Stream(ref stream) => stream,
                    _ => panic!(),
                };
                let range: Vec<f64> = get(doc, dict, b"Range");
                let domain: Vec<f64> = get(doc, dict, b"Domain");
                let contents = get_contents(stream);
                let size: Vec<i64> = get(doc, dict, b"Size");
                let bits_per_sample = get(doc, dict, b"BitsPerSample");
                // We ignore 'Order' like pdfium, poppler and pdf.js

                let encode = get::<Option<Vec<f64>>>(doc, dict, b"Encode");
                // maybe there's some better way to write this.
                let encode = encode.unwrap_or_else(|| {
                    let mut default = Vec::new();
                    for i in &size {
                        default.extend([0., (i - 1) as f64].iter());
                    }
                    default
                });
                let decode =
                    get::<Option<Vec<f64>>>(doc, dict, b"Decode").unwrap_or_else(|| range.clone());

                Function::Type0(Type0Func {
                    domain,
                    range,
                    size,
                    contents,
                    bits_per_sample,
                    encode,
                    decode,
                })
            }
            2 => {
                let c0 = get::<Option<Vec<f64>>>(doc, dict, b"C0");
                let c1 = get::<Option<Vec<f64>>>(doc, dict, b"C1");
                let n = get::<f64>(doc, dict, b"N");
                Function::Type2(Type2Func { c0, c1, n })
            }
            _ => {
                panic!("unhandled function type {}", function_type)
            }
        };
        f
    }
}

fn as_num(o: &Object) -> f64 {
    match o {
        &Object::Integer(i) => i as f64,
        &Object::Real(f) => f.into(),
        _ => {
            panic!("not a number")
        }
    }
}

#[derive(Clone)]
struct TextState<'a> {
    font: Option<Rc<dyn PdfFont + 'a>>,
    font_size: f64,
    character_spacing: f64,
    word_spacing: f64,
    horizontal_scaling: f64,
    leading: f64,
    rise: f64,
    tm: Transform,
}

// XXX: We'd ideally implement this without having to copy the uncompressed data
fn get_contents(contents: &Stream) -> Vec<u8> {
    if contents.filter().is_ok() {
        contents
            .decompressed_content()
            .unwrap_or_else(|_| contents.content.clone())
    } else {
        contents.content.clone()
    }
}

#[derive(Debug, Clone)]
struct PdfImage<'a> {
    pub id: ObjectId,
    pub width: i64,
    pub height: i64,
    pub color_space: Option<String>,
    pub filters: Option<Vec<String>>,
    pub bits_per_component: Option<i64>,
    pub content: &'a [u8],
    pub origin_dict: &'a Dictionary,
}

impl<'a> PdfImage<'a> {
    fn to_rgb_image(&self) -> Option<RgbImage> {
        // First handle common compression filters
        let decoded_data = if let Some(filters) = &self.filters {
            let mut data = self.content.to_vec();

            for filter in filters {
                match filter.as_str() {
                    "DCTDecode" | "DCT" => {
                        // JPEG data
                        return image::load_from_memory_with_format(
                            &data,
                            image::ImageFormat::Jpeg,
                        )
                        .ok()
                        .map(|img| img.into_rgb8());
                    }
                    // "JPXDecode" => {
                    //     // JPEG2000 data
                    //     return image::load_from_memory_with_format(&data, image::ImageFormat::)
                    //         .ok()
                    //         .map(|img| img.into_rgb8());
                    // },
                    "FlateDecode" => {
                        // Decompress using flate/zlib
                        data = miniz_oxide::inflate::decompress_to_vec_zlib(&data).ok()?;
                    }
                    _ => return None, // Unsupported filter
                }
            }
            data
        } else {
            self.content.to_vec()
        };

        // Handle different color spaces
        match &self.color_space {
            Some(cs) => match cs.as_str() {
                "DeviceRGB" => {
                    let mut img = RgbImage::new(self.width as u32, self.height as u32);
                    for y in 0..self.height as u32 {
                        for x in 0..self.width as u32 {
                            let pos = ((y * self.width as u32 + x) * 3) as usize;
                            if pos + 2 < decoded_data.len() {
                                img.put_pixel(
                                    x,
                                    y,
                                    image::Rgb([
                                        decoded_data[pos],
                                        decoded_data[pos + 1],
                                        decoded_data[pos + 2],
                                    ]),
                                );
                            }
                        }
                    }
                    Some(img)
                }
                "DeviceGray" => {
                    let mut img = RgbImage::new(self.width as u32, self.height as u32);
                    for y in 0..self.height as u32 {
                        for x in 0..self.width as u32 {
                            let pos = (y * self.width as u32 + x) as usize;
                            if pos < decoded_data.len() {
                                let gray = decoded_data[pos];
                                img.put_pixel(x, y, image::Rgb([gray, gray, gray]));
                            }
                        }
                    }
                    Some(img)
                }
                _ => None, // Other color spaces not supported for now
            },
            None => None,
        }
    }
}

// In your content processing function:
fn process_xobject(
    doc: &Document,
    resources: &Dictionary,
    ocr_handler: Option<&OcrHandler>,
    text_segments: &mut Vec<TextSegment>,
    position: (f64, f64),
    size: (f64, f64),
    page_num: u32,
    current_font_size: f64,
    current_transformed_font_size: f64,
    page_char_counter: &mut usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let xobject = doc.get_dict_in_dict(resources, b"XObject")?;

    for (_, xvalue) in xobject.iter() {
        let id = xvalue.as_reference()?;
        let xvalue = doc.get_object(id)?;
        let xvalue = xvalue.as_stream()?;
        let dict = &xvalue.dict;

        // Only process images
        if dict.get(b"Subtype")?.as_name()? != b"Image" {
            continue;
        }

        // Extract image information
        let width = dict.get(b"Width")?.as_i64()?;
        let height = dict.get(b"Height")?.as_i64()?;
        let color_space = match dict.get(b"ColorSpace") {
            Ok(cs) => match cs {
                Object::Array(array) => {
                    Some(String::from_utf8_lossy(array[0].as_name()?).to_string())
                }
                Object::Name(name) => Some(String::from_utf8_lossy(name).to_string()),
                _ => None,
            },
            Err(_) => None,
        };

        let bits_per_component = match dict.get(b"BitsPerComponent") {
            Ok(bpc) => Some(bpc.as_i64()?),
            Err(_) => None,
        };

        let mut filters = vec![];
        if let Ok(filter) = dict.get(b"Filter") {
            match filter {
                Object::Array(array) => {
                    for obj in array.iter() {
                        let name = obj.as_name()?;
                        filters.push(String::from_utf8_lossy(name).to_string());
                    }
                }
                Object::Name(name) => {
                    filters.push(String::from_utf8_lossy(name).to_string());
                }
                _ => {}
            }
        };

        let pdf_image = PdfImage {
            id,
            width,
            height,
            color_space,
            bits_per_component,
            filters: Some(filters),
            content: &xvalue.content,
            origin_dict: &xvalue.dict,
        };

        println!("ocr_handler before");
        // Only attempt OCR if we have a handler
        if let Some(handler) = ocr_handler {
            println!("ocr_handler after");
            if let Some(img) = pdf_image.to_rgb_image() {
                println!("img");
                if let Ok(ocr_text) = handler.process_image(&img) {
                    println!("ocr_text");
                    if !ocr_text.is_empty() {
                        let ocr_text_len = ocr_text.chars().count();
                        text_segments.push(TextSegment {
                            content: ocr_text,
                            font_size: current_font_size,
                            transformed_font_size: current_transformed_font_size,
                            x: position.0,
                            y: position.1,
                            is_bold: false,
                            font_name: "OCR".to_string(),
                            font_weight: FontWeight::Regular,
                            is_italic: false,
                            page_num,
                            cutat: "Image".to_string(),
                            fill_color: None,
                            stroke_color: None,
                            char_start: *page_char_counter,
                            char_end: *page_char_counter + ocr_text_len,
                            width: size.0,
                            height: size.1,
                        });
                        *page_char_counter += ocr_text_len;
                    }
                } else {
                    if let Err(e) = handler.process_image(&img) {
                        println!("OCR processing error: {}", e);
                    }
                }
            }
        }
    }

    Ok(())
}

#[derive(Clone)]
struct GraphicsState<'a> {
    ctm: Transform,
    ts: TextState<'a>,
    smask: Option<Dictionary>,
    fill_colorspace: ColorSpace,
    fill_color: Vec<f64>,
    stroke_colorspace: ColorSpace,
    stroke_color: Vec<f64>,
    line_width: f64,
}

fn show_text(
    gs: &mut GraphicsState,
    s: &[u8],
    _tlm: &Transform,
    _flip_ctm: &Transform,
    output: &mut dyn OutputDev,
) -> Result<(), OutputError> {
    let ts = &mut gs.ts;
    let font = ts.font.as_ref().unwrap();
    //let encoding = font.encoding.as_ref().map(|x| &x[..]).unwrap_or(&PDFDocEncoding);
    dlog!("{:?}", font.decode(s));
    dlog!("{:?}", font.decode(s).as_bytes());
    dlog!("{:?}", s);
    output.begin_word()?;

    for (c, length) in font.char_codes(s) {
        // 5.3.3 Text Space Details
        let tsm = Transform2D::row_major(ts.horizontal_scaling, 0., 0., 1.0, 0., ts.rise);
        // Trm = Tsm × Tm × CTM
        let trm = tsm.post_transform(&ts.tm.post_transform(&gs.ctm));
        //dlog!("ctm: {:?} tm {:?}", gs.ctm, tm);
        //dlog!("current pos: {:?}", position);
        // 5.9 Extraction of Text Content

        //dlog!("w: {}", font.widths[&(*c as i64)]);
        let w0 = font.get_width(c) / 1000.;

        let mut spacing = ts.character_spacing;
        // "Word spacing is applied to every occurrence of the single-byte character code 32 in a
        //  string when using a simple font or a composite font that defines code 32 as a
        //  single-byte code. It does not apply to occurrences of the byte value 32 in
        //  multiple-byte codes."
        let is_space = c == 32 && length == 1;
        if is_space {
            spacing += ts.word_spacing
        }

        output.output_character(&trm, w0, spacing, ts.font_size, &font.decode_char(c))?;
        let tj = 0.;
        let ty = 0.;
        let tx = ts.horizontal_scaling * ((w0 - tj / 1000.) * ts.font_size + spacing);
        dlog!(
            "horizontal {} adjust {} {} {} {}",
            ts.horizontal_scaling,
            tx,
            w0,
            ts.font_size,
            spacing
        );
        // dlog!("w0: {}, tx: {}", w0, tx);
        ts.tm = ts
            .tm
            .pre_transform(&Transform2D::create_translation(tx, ty));
        let _trm = ts.tm.pre_transform(&gs.ctm);
        //dlog!("post pos: {:?}", trm);
    }
    output.end_word()?;
    Ok(())
}

#[derive(Debug, Clone, Copy)]
pub struct MediaBox {
    pub llx: f64,
    pub lly: f64,
    pub urx: f64,
    pub ury: f64,
}

fn apply_state(doc: &Document, gs: &mut GraphicsState, state: &Dictionary) {
    for (k, v) in state.iter() {
        let k: &[u8] = k.as_ref();
        match k {
            b"SMask" => match maybe_deref(doc, v) {
                &Object::Name(ref name) => {
                    if name == b"None" {
                        gs.smask = None;
                    } else {
                        panic!("unexpected smask name")
                    }
                }
                &Object::Dictionary(ref dict) => {
                    gs.smask = Some(dict.clone());
                }
                _ => {
                    panic!("unexpected smask type {:?}", v)
                }
            },
            b"Type" => match v {
                &Object::Name(ref name) => {
                    assert_eq!(name, b"ExtGState")
                }
                _ => {
                    panic!("unexpected type")
                }
            },
            _ => {
                dlog!("unapplied state: {:?} {:?}", k, v);
            }
        }
    }
}

#[derive(Debug)]
pub enum PathOp {
    MoveTo(f64, f64),
    LineTo(f64, f64),
    // XXX: is it worth distinguishing the different kinds of curve ops?
    CurveTo(f64, f64, f64, f64, f64, f64),
    Rect(f64, f64, f64, f64),
    Close,
}

#[derive(Debug)]
pub struct Path {
    pub ops: Vec<PathOp>,
}

impl Path {
    fn new() -> Path {
        Path { ops: Vec::new() }
    }
    fn current_point(&self) -> (f64, f64) {
        match self.ops.last().unwrap() {
            &PathOp::MoveTo(x, y) => (x, y),
            &PathOp::LineTo(x, y) => (x, y),
            &PathOp::CurveTo(_, _, _, _, x, y) => (x, y),
            _ => {
                panic!()
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct CalGray {
    white_point: [f64; 3],
    black_point: Option<[f64; 3]>,
    gamma: Option<f64>,
}

#[derive(Clone, Debug)]
pub struct CalRGB {
    white_point: [f64; 3],
    black_point: Option<[f64; 3]>,
    gamma: Option<[f64; 3]>,
    matrix: Option<Vec<f64>>,
}

#[derive(Clone, Debug)]
pub struct Lab {
    white_point: [f64; 3],
    black_point: Option<[f64; 3]>,
    range: Option<[f64; 4]>,
}

#[derive(Clone, Debug)]
pub enum AlternateColorSpace {
    DeviceGray,
    DeviceRGB,
    DeviceCMYK,
    CalRGB(CalRGB),
    CalGray(CalGray),
    Lab(Lab),
    ICCBased(Vec<u8>),
}

#[derive(Clone)]
pub struct Separation {
    name: String,
    alternate_space: AlternateColorSpace,
    tint_transform: Box<Function>,
}

#[derive(Clone)]
pub enum ColorSpace {
    DeviceGray,
    DeviceRGB,
    DeviceCMYK,
    Pattern,
    CalRGB(CalRGB),
    CalGray(CalGray),
    Lab(Lab),
    Separation(Separation),
    ICCBased(Vec<u8>),
}

fn make_colorspace<'a>(doc: &'a Document, name: &[u8], resources: &'a Dictionary) -> ColorSpace {
    match name {
        b"DeviceGray" => ColorSpace::DeviceGray,
        b"DeviceRGB" => ColorSpace::DeviceRGB,
        b"DeviceCMYK" => ColorSpace::DeviceCMYK,
        b"Pattern" => ColorSpace::Pattern,
        _ => {
            let colorspaces: &Dictionary = get(&doc, resources, b"ColorSpace");
            let cs: &Object = maybe_get_obj(doc, colorspaces, &name[..])
                .unwrap_or_else(|| panic!("missing colorspace {:?}", &name[..]));
            if let Ok(cs) = cs.as_array() {
                let cs_name = pdf_to_utf8(cs[0].as_name().expect("first arg must be a name"));
                match cs_name.as_ref() {
                    "Separation" => {
                        let name = pdf_to_utf8(cs[1].as_name().expect("second arg must be a name"));
                        let alternate_space = match &maybe_deref(doc, &cs[2]) {
                            Object::Name(name) => match &name[..] {
                                b"DeviceGray" => AlternateColorSpace::DeviceGray,
                                b"DeviceRGB" => AlternateColorSpace::DeviceRGB,
                                b"DeviceCMYK" => AlternateColorSpace::DeviceCMYK,
                                _ => panic!("unexpected color space name"),
                            },
                            Object::Array(cs) => {
                                let cs_name =
                                    pdf_to_utf8(cs[0].as_name().expect("first arg must be a name"));
                                match cs_name.as_ref() {
                                    "ICCBased" => {
                                        let stream = maybe_deref(doc, &cs[1]).as_stream().unwrap();
                                        dlog!("ICCBased {:?}", stream);
                                        // XXX: we're going to be continually decompressing everytime this object is referenced
                                        AlternateColorSpace::ICCBased(get_contents(stream))
                                    }
                                    "CalGray" => {
                                        let dict =
                                            cs[1].as_dict().expect("second arg must be a dict");
                                        AlternateColorSpace::CalGray(CalGray {
                                            white_point: get(&doc, dict, b"WhitePoint"),
                                            black_point: get(&doc, dict, b"BackPoint"),
                                            gamma: get(&doc, dict, b"Gamma"),
                                        })
                                    }
                                    "CalRGB" => {
                                        let dict =
                                            cs[1].as_dict().expect("second arg must be a dict");
                                        AlternateColorSpace::CalRGB(CalRGB {
                                            white_point: get(&doc, dict, b"WhitePoint"),
                                            black_point: get(&doc, dict, b"BackPoint"),
                                            gamma: get(&doc, dict, b"Gamma"),
                                            matrix: get(&doc, dict, b"Matrix"),
                                        })
                                    }
                                    "Lab" => {
                                        let dict =
                                            cs[1].as_dict().expect("second arg must be a dict");
                                        AlternateColorSpace::Lab(Lab {
                                            white_point: get(&doc, dict, b"WhitePoint"),
                                            black_point: get(&doc, dict, b"BackPoint"),
                                            range: get(&doc, dict, b"Range"),
                                        })
                                    }
                                    _ => panic!("Unexpected color space name"),
                                }
                            }
                            _ => panic!("Alternate space should be name or array {:?}", cs[2]),
                        };
                        let tint_transform = Box::new(Function::new(doc, maybe_deref(doc, &cs[3])));

                        dlog!("{:?} {:?} {:?}", name, alternate_space, tint_transform);
                        ColorSpace::Separation(Separation {
                            name,
                            alternate_space,
                            tint_transform,
                        })
                    }
                    "ICCBased" => {
                        let stream = maybe_deref(doc, &cs[1]).as_stream().unwrap();
                        dlog!("ICCBased {:?}", stream);
                        // XXX: we're going to be continually decompressing everytime this object is referenced
                        ColorSpace::ICCBased(get_contents(stream))
                    }
                    "CalGray" => {
                        let dict = cs[1].as_dict().expect("second arg must be a dict");
                        ColorSpace::CalGray(CalGray {
                            white_point: get(&doc, dict, b"WhitePoint"),
                            black_point: get(&doc, dict, b"BackPoint"),
                            gamma: get(&doc, dict, b"Gamma"),
                        })
                    }
                    "CalRGB" => {
                        let dict = cs[1].as_dict().expect("second arg must be a dict");
                        ColorSpace::CalRGB(CalRGB {
                            white_point: get(&doc, dict, b"WhitePoint"),
                            black_point: get(&doc, dict, b"BackPoint"),
                            gamma: get(&doc, dict, b"Gamma"),
                            matrix: get(&doc, dict, b"Matrix"),
                        })
                    }
                    "Lab" => {
                        let dict = cs[1].as_dict().expect("second arg must be a dict");
                        ColorSpace::Lab(Lab {
                            white_point: get(&doc, dict, b"WhitePoint"),
                            black_point: get(&doc, dict, b"BackPoint"),
                            range: get(&doc, dict, b"Range"),
                        })
                    }
                    "Pattern" => ColorSpace::Pattern,
                    "DeviceGray" => ColorSpace::DeviceGray,
                    "DeviceRGB" => ColorSpace::DeviceRGB,
                    "DeviceCMYK" => ColorSpace::DeviceCMYK,
                    _ => {
                        panic!("color_space {:?} {:?} {:?}", name, cs_name, cs)
                    }
                }
            } else if let Ok(cs) = cs.as_name() {
                match pdf_to_utf8(cs).as_ref() {
                    "DeviceRGB" => ColorSpace::DeviceRGB,
                    "DeviceGray" => ColorSpace::DeviceGray,
                    _ => panic!(),
                }
            } else {
                panic!();
            }
        }
    }
}

#[derive(Debug, Clone)]
struct TextSegment {
    content: String,
    font_size: f64,
    transformed_font_size: f64,
    x: f64,
    y: f64,
    is_bold: bool,
    font_name: String, // New field to store the font name
    font_weight: FontWeight,
    is_italic: bool,
    page_num: u32,
    cutat: String,
    // font_color: (f64, f64, f64),
    fill_color: Option<(u8, u8, u8)>,
    stroke_color: Option<(u8, u8, u8)>,
    // Position tracking
    char_start: usize,    // Character position in page
    char_end: usize,      // End character position in page
    width: f64,           // Width of text segment
    height: f64,          // Height of text segment
}
struct Processor<'a> {
    _none: PhantomData<&'a ()>,
}

impl<'a> Processor<'a> {
    fn new() -> Processor<'a> {
        Processor { _none: PhantomData }
    }

    fn create_text_segment(
        content: String,
        font_size: f64,
        transformed_font_size: f64,
        x: f64,
        y: f64,
        is_bold: bool,
        font_name: String,
        font_weight: FontWeight,
        is_italic: bool,
        page_num: u32,
        cutat: String,
        fill_color: Option<(u8, u8, u8)>,
        stroke_color: Option<(u8, u8, u8)>,
        char_start: usize,
        char_end: usize,
    ) -> TextSegment {
        let char_count = content.chars().count();
        // Calculate approximate width based on average character width
        // For multi-line segments, we need a better approximation
        let avg_chars_per_line = 80.0; // Typical line length
        let lines = (char_count as f64 / avg_chars_per_line).max(1.0);
        let approx_width = if lines > 1.0 {
            // For multi-line text, use average line width
            avg_chars_per_line * font_size * 0.6
        } else {
            // For single line, use actual character count
            char_count as f64 * font_size * 0.6
        };
        TextSegment {
            content,
            font_size,
            transformed_font_size,
            x,
            y,
            is_bold,
            font_name,
            font_weight,
            is_italic,
            page_num,
            cutat,
            fill_color,
            stroke_color,
            char_start,
            char_end,
            width: approx_width,
            height: font_size,
        }
    }

    fn is_visible_text(
        &self,
        transform: &Transform,
        font_size: f64,
        color: &(f64, f64, f64),
        media_box: &MediaBox,
    ) -> bool {
        const MIN_FONT_SIZE: f64 = 4.0;
        const MIN_COLOR_DIFF: f64 = 0.1;

        // if font_size < MIN_FONT_SIZE {
        //     return false;
        // }

        let point = transform.transform_point(Point2D::new(0.0, 0.0));
        if point.x < media_box.llx
            || point.x > media_box.urx
            || point.y < media_box.lly
            || point.y > media_box.ury
        {
            return false;
        }

        // Assuming white background, check if text color is too close to white
        if color.0 > 1.0 - MIN_COLOR_DIFF
            && color.1 > 1.0 - MIN_COLOR_DIFF
            && color.2 > 1.0 - MIN_COLOR_DIFF
        {
            return false;
        }

        true
    }

    fn process_stream(
        &mut self,
        doc: &'a Document,
        ocr_handler: Option<&OcrHandler>,
        content: Vec<u8>,
        resources: &'a Dictionary,
        media_box: &MediaBox,
        page_num: u32,
        text_segments: &mut Vec<TextSegment>,
    ) -> Result<(), OutputError> {
        let mut text = String::new();
        let mut current_line = String::new();
        let mut current_word = String::new();
        let mut last_end = 100000.0;
        let mut last_y = 0.0;
        let mut current_is_bold = false;
        let mut current_font_size = 0.0;
        let mut current_transformed_font_size = 0.0;
        let mut current_x = 0.0;
        let mut current_y = 0.0;
        let mut first_char = false;
        let mut current_font = String::new();
        let mut current_font_weight = FontWeight::Regular;
        let mut current_is_italic = false;
        let mut current_color = (0.0, 0.0, 0.0); // Default to black
        let mut is_clipping = false;
        let mut current_transform = Transform::default();
        let mut current_font_color = (0.0, 0.0, 0.0); // Default to black
        let mut page_char_counter = 0usize; // Track character position in page
        let mut current_segment_start = 0usize; // Start position of current segment

        let content = Content::decode(&content).unwrap();
        let mut gs: GraphicsState = GraphicsState {
            ts: TextState {
                font: None,
                font_size: std::f64::NAN,
                character_spacing: 0.,
                word_spacing: 0.,
                horizontal_scaling: 100. / 100.,
                leading: 0.,
                rise: 0.,
                tm: Transform2D::identity(),
            },
            fill_color: Vec::new(),
            fill_colorspace: ColorSpace::DeviceGray,
            stroke_color: Vec::new(),
            stroke_colorspace: ColorSpace::DeviceGray,
            line_width: 1.,
            ctm: Transform2D::identity(),
            smask: None,
        };

        let mut gs_stack = Vec::new();
        let mut mc_stack = Vec::new();
        let mut tlm = Transform2D::<f64, Space, Space>::identity();
        let mut path = Path::new();
        let flip_ctm = Transform2D::<f64, Space, Space>::row_major(
            1.,
            0.,
            0.,
            -1.,
            0.,
            media_box.ury - media_box.lly,
        );
        dlog!("MediaBox {:?}", media_box);
        let mut layout_analyzer = LayoutAnalyzer::new(media_box.ury - media_box.lly);

        // Add to Processor struct
        let mut last_vertical_gap: Option<f64> = None;

        for operation in &content.operations {
            match operation.operator.as_ref() {
                "BT" => {
                    tlm = Transform2D::identity();
                    gs.ts.tm = tlm;
                }
                "ET" => {
                    tlm = Transform2D::identity();
                    gs.ts.tm = tlm;
                }
                "cm" => {
                    assert!(operation.operands.len() == 6);
                    let m = Transform2D::row_major(
                        as_num(&operation.operands[0]),
                        as_num(&operation.operands[1]),
                        as_num(&operation.operands[2]),
                        as_num(&operation.operands[3]),
                        as_num(&operation.operands[4]),
                        as_num(&operation.operands[5]),
                    );
                    gs.ctm = gs.ctm.pre_transform(&m);
                    dlog!("matrix {:?}", gs.ctm);
                }
                "CS" => {
                    let name = operation.operands[0].as_name().unwrap();
                    gs.stroke_colorspace = make_colorspace(doc, name, resources);
                }
                "cs" => {
                    let name = operation.operands[0].as_name().unwrap();
                    gs.fill_colorspace = make_colorspace(doc, name, resources);
                }
                "SC" | "SCN" => {
                    gs.stroke_color = match gs.stroke_colorspace {
                        ColorSpace::Pattern => {
                            dlog!("unhandled pattern color");
                            Vec::new()
                        }
                        _ => operation.operands.iter().map(|x| as_num(x)).collect(),
                    };
                }
                "sc" | "scn" => {
                    gs.fill_color = match gs.fill_colorspace {
                        ColorSpace::Pattern => {
                            dlog!("unhandled pattern color");
                            Vec::new()
                        }
                        _ => operation.operands.iter().map(|x| as_num(x)).collect(),
                    };
                }
                "rg" | "g" => {
                    current_color = if operation.operator == "rg" {
                        (
                            as_num(&operation.operands[0]),
                            as_num(&operation.operands[1]),
                            as_num(&operation.operands[2]),
                        )
                    } else {
                        let gray = as_num(&operation.operands[0]);
                        (gray, gray, gray)
                    };
                }
                "G" | "g" | "RG" | "rg" | "K" | "k" => {
                    dlog!("unhandled color operation {:?}", operation);
                }
                "Tf" => {
                    let fonts: &Dictionary = get(&doc, resources, b"Font");
                    let name = operation.operands[0].as_name().unwrap();
                    let font = make_font(doc, get::<&Dictionary>(doc, fonts, name));

                    let new_font_size =
                        (as_num(&operation.operands[1]) * 100.0_f64).round() / 100.0;
                    let new_font_name = String::from_utf8_lossy(name).into_owned();
                    let font_info = FontInfo::from_font_name(&new_font_name);
                    
                    // Enhanced bold detection: first try font name analysis, then fallback to font object debug
                    let mut new_is_bold = font_info.weight.is_bold();
                    if !new_is_bold {
                        // Fallback to checking the font object's debug representation
                        // This catches cases where bold info is in font descriptors but not in the name
                        let font_debug = format!("{:?}", font).to_lowercase();
                        new_is_bold = font_debug.contains("bold");
                        
                        // Debug output to help diagnose bold detection issues
                        if new_is_bold {
                            eprintln!("Bold detected via font debug for font: {} (weight: {:?})", new_font_name, font_info.weight);
                        }
                    }
                    
                    let new_font_weight = font_info.weight.clone();
                    let new_is_italic = font_info.is_italic;

                    // let is_add =
                    //     self.is_visible_text(&tlm, current_font_size, &current_color, media_box);

                   if (new_is_bold != current_is_bold || new_font_size != current_font_size || new_font_name != current_font || new_font_weight != current_font_weight || new_is_italic != current_is_italic)
    && !current_line.trim().is_empty()
{
    // Only cut a new text segment if we are actually on a new line.
    // Here we check if the vertical difference is significant compared to the font size.
   
        if current_x > 0.0 && current_y > 0.0 {
            // Process the fill color as before.
           
            // Push the current line as a new TextSegment.
            let content_str = current_line.clone().trim().to_string();
            let char_count = content_str.chars().count();
            // Calculate approximate width - for multi-line segments, use average line width
            let avg_chars_per_line = 80.0;
            let lines = (char_count as f64 / avg_chars_per_line).max(1.0);
            let approx_width = if lines > 1.0 {
                avg_chars_per_line * current_font_size * 0.6
            } else {
                char_count as f64 * current_font_size * 0.6
            };
            text_segments.push(TextSegment {
                content: content_str,
                font_size: current_font_size,
                transformed_font_size: current_transformed_font_size,
                x: current_x,
                y: current_y,
                is_bold: current_is_bold,
                font_name: current_font.clone(),
                font_weight: current_font_weight.clone(),
                is_italic: current_is_italic,
                page_num: page_num,
                cutat: "Tj".to_string(),
                fill_color: None,
                stroke_color: None,
                char_start: current_segment_start,
                char_end: page_char_counter,
                width: approx_width,
                height: current_font_size,
            });
            current_segment_start = page_char_counter;
            current_line.clear();
        }
   
    // Whether or not we pushed a new segment, update our current font properties
    // so that subsequent text uses the new style.
    current_is_bold = new_is_bold;
    current_font = new_font_name;
    current_font_size = new_font_size;
    current_font_weight = new_font_weight;
    current_is_italic = new_is_italic;
}

                    gs.ts.font = Some(font.clone());

                    gs.ts.font_size = as_num(&operation.operands[1]);
                    // println!("font size: {}", gs.ts.font_size);

                    dlog!(
                        "font {} size: {} {:?}",
                        pdf_to_utf8(name),
                        current_font_size,
                        operation
                    );
                }
                "TJ" => match operation.operands[0] {
                    Object::Array(ref array) => {
                        // current_line.push_str("*");
                        for e in array {
                            match e {
                                &Object::String(ref s, _) => {
                                    // let ts = &mut gs.ts;
                                    let font: &Rc<dyn PdfFont> = gs.ts.font.as_ref().unwrap();
                                    // let font_text = format!("{:#?}", font);
                                    first_char = true;

                                    for (c, length) in font.char_codes(s) {
                                        let w0 = font.get_width(c) / 1000.;
                                        let mut spacing = gs.ts.character_spacing;
                                        let is_space = c == 32 && length == 1;
                                        if is_space {
                                            spacing += gs.ts.word_spacing;
                                        }

                                        let char = font.decode_char(c);

                                        let position = gs.ts.tm.post_transform(&flip_ctm);
                                        let (x, y) = (position.m31, position.m32);
                                        let transformed_font_size_vec = gs.ts.tm.transform_vector(
                                            vec2(gs.ts.font_size, gs.ts.font_size),
                                        );
                                        let transformed_font_size = ((transformed_font_size_vec.x
                                            * transformed_font_size_vec.y)
                                            .sqrt()
                                            * 100.0)
                                            .round()
                                            / 100.0;
                                        let is_add = self.is_visible_text(
                                            &tlm,
                                            current_font_size,
                                            &current_color,
                                            media_box,
                                        );

                                        if transformed_font_size != current_transformed_font_size
                                            && !transformed_font_size.is_nan()
                                            && !current_transformed_font_size.is_nan()
                                        {
                                            if !current_line.trim().is_empty() {
                                                // also check if current x and current y are positive and that y is greater than font size, sometimes text seems to be hidden in the pdf which we should avoid
                                                if current_x > 0.0 && current_y > 0.0
                                                //  && is_add
                                                //     && current_y < self.current_font_size
                                                {
                                                    let processed_fill_color = (
                                                        (current_font_color.0 * 255.0) as u8,
                                                        (current_font_color.1 * 255.0) as u8,
                                                        (current_font_color.2 * 255.0) as u8,
                                                    );
                                                        // if is_visible_text(
                                                        //     Some(processed_fill_color),
                                                        //     (255, 255, 255),
                                                        // ) {
                                                        let content_str = current_line.clone().trim().to_string();
                                                        text_segments.push(Self::create_text_segment(
                                                            content_str,
                                                            current_font_size,
                                                            current_transformed_font_size,
                                                            current_x,
                                                            current_y,
                                                            current_is_bold,
                                                            current_font.clone(),
                                                            current_font_weight.clone(),
                                                            current_is_italic,
                                                            page_num,
                                                            "Tj".to_string(),
                                                            Some(processed_fill_color),
                                                            None,
                                                            current_segment_start,
                                                            page_char_counter,
                                                        ));
                                                        current_segment_start = page_char_counter;
                                                    // }
                                                }
                                                current_line.clear();
                                            }

                                            current_transformed_font_size = transformed_font_size;
                                        }

                                        if first_char {
                                            // Check for paragraph break (large vertical gap)
                                            if is_paragraph_break(y, last_y, transformed_font_size) {
                                                current_line.push('\n');
                                                page_char_counter += 1;
                                            }
                                            // Check for new line (moved down and back to left)
                                            else if is_new_line(x, last_end, y, last_y, transformed_font_size) {
                                                current_line.push('\n');
                                                page_char_counter += 1;
                                            }
                                            // Check for space between words on same line
                                            else if should_insert_space(x, last_end, transformed_font_size) {
                                                current_line.push(' ');
                                                page_char_counter += 1;
                                            }

                                            current_x = (x * 100.00).round() / 100.0;
                                            current_y = (y * 100.00).round() / 100.0;
                                        }

                                        if is_add {
                                            current_line.push_str(&char);
                                            page_char_counter += char.chars().count();
                                        }
                                        first_char = false;

                                        last_end = x + w0 * transformed_font_size;
                                        last_y = y;

                                        let tx = gs.ts.horizontal_scaling
                                            * ((w0 - 0. / 1000.) * gs.ts.font_size + spacing);
                                        gs.ts.tm = gs.ts.tm.pre_transform(
                                            &Transform2D::create_translation(tx, 0.),
                                        );
                                    }
                                }
                                &Object::Integer(i) => {
                                    let ts = &mut gs.ts;
                                    let w0 = 0.;
                                    let tj = i as f64;
                                    let ty = 0.;
                                    let tx =
                                        ts.horizontal_scaling * ((w0 - tj / 1000.) * ts.font_size);
                                    ts.tm = ts
                                        .tm
                                        .pre_transform(&Transform2D::create_translation(tx, ty));
                                    dlog!("adjust text by: {} {:?}", i, ts.tm);
                                }
                                &Object::Real(i) => {
                                    let ts = &mut gs.ts;
                                    let w0 = 0.;
                                    let tj = i as f64;
                                    let ty = 0.;
                                    let tx =
                                        ts.horizontal_scaling * ((w0 - tj / 1000.) * ts.font_size);
                                    ts.tm = ts
                                        .tm
                                        .pre_transform(&Transform2D::create_translation(tx, ty));
                                    dlog!("adjust text by: {} {:?}", i, ts.tm);
                                }
                                _ => {
                                    dlog!("kind of {:?}", e);
                                }
                            }
                        }
                    }
                    _ => {}
                },
                "Tj" => match operation.operands[0] {
                    Object::String(ref s, _) => {
                        let ts = &mut gs.ts;
                        let font: &Rc<dyn PdfFont> = gs.ts.font.as_ref().unwrap();
                        // let font_text = format!("{:#?}", font);
                        first_char = true;

                        for (c, length) in font.char_codes(s) {
                            let w0 = font.get_width(c) / 1000.;
                            let mut spacing = gs.ts.character_spacing;
                            let is_space = c == 32 && length == 1;
                            if is_space {
                                spacing += gs.ts.word_spacing;
                            }

                            let char = font.decode_char(c);

                            let position = gs.ts.tm.post_transform(&flip_ctm);
                            let (x, y) = (position.m31, position.m32);
                            let transformed_font_size_vec = gs
                                .ts
                                .tm
                                .transform_vector(vec2(gs.ts.font_size, gs.ts.font_size));
                            let transformed_font_size = ((transformed_font_size_vec.x
                                * transformed_font_size_vec.y)
                                .sqrt()
                                * 100.0)
                                .round()
                                / 100.0;

                            let is_add = self.is_visible_text(
                                &tlm,
                                current_font_size,
                                &current_color,
                                media_box,
                            );
                            if transformed_font_size != current_transformed_font_size
                                && !transformed_font_size.is_nan()
                                && !current_transformed_font_size.is_nan()
                            {
                                if !current_line.trim().is_empty() {
                                    // also check if current x and current y are positive and that y is greater than font size, sometimes text seems to be hidden in the pdf which we should avoid
                                    if current_x > 0.0 && current_y > 0.0
                                    //     && current_y < self.current_font_size
                                    {
                                        // Convert gs.fill_color (Vec<f64>) to a (u8, u8, u8) tuple:
                                        let processed_fill_color = (
                                            (current_font_color.0 * 255.0) as u8,
                                            (current_font_color.1 * 255.0) as u8,
                                            (current_font_color.2 * 255.0) as u8,
                                        );
                                        // if is_visible_text(
                                        //     Some(processed_fill_color),
                                        //     (255, 255, 255),
                                        // ) {
                                            let content_str = current_line.clone().trim().to_string();
                                            text_segments.push(Self::create_text_segment(
                                                content_str,
                                                current_font_size,
                                                current_transformed_font_size,
                                                current_x,
                                                current_y,
                                                current_is_bold,
                                                current_font.clone(),
                                                current_font_weight.clone(),
                                                current_is_italic,
                                                page_num,
                                                "Tj".to_string(),
                                                Some(processed_fill_color),
                                                None,
                                                current_segment_start,
                                                page_char_counter,
                                            ));
                                            current_segment_start = page_char_counter;
                                        // }
                                    }
                                    current_line.clear();
                                }

                                current_transformed_font_size = transformed_font_size;
                            }

                            if first_char {
                                // Check for paragraph break (large vertical gap)
                                if is_paragraph_break(y, last_y, transformed_font_size) {
                                    current_line.push('\n')
                                }
                                // Check for new line (moved down and back to left)
                                else if is_new_line(x, last_end, y, last_y, transformed_font_size) {
                                    current_line.push('\n')
                                }
                                // Check for space between words on same line
                                else if should_insert_space(x, last_end, transformed_font_size) {
                                    current_line.push(' ')
                                }

                                current_x = x;
                                current_y = y;
                                // current_transformed_font_size = transformed_font_size;
                                // self.current_font_name =
                            }
                            if is_add {
                                current_line.push_str(&char);
                            }
                            first_char = false;

                            last_end = x + w0 * transformed_font_size;
                            last_y = y;

                            let tx = gs.ts.horizontal_scaling
                                * ((w0 - 0. / 1000.) * gs.ts.font_size + spacing);
                            gs.ts.tm = gs
                                .ts
                                .tm
                                .pre_transform(&Transform2D::create_translation(tx, 0.));
                        }
                    }
                    _ => {
                        panic!("unexpected Tj operand {:?}", operation)
                    }
                },
                "Tc" => {
                    gs.ts.character_spacing = as_num(&operation.operands[0]);
                }
                "Tw" => {
                    gs.ts.word_spacing = as_num(&operation.operands[0]);
                }
                "Tz" => {
                    gs.ts.horizontal_scaling = as_num(&operation.operands[0]) / 100.;
                }
                "TL" => {
                    gs.ts.leading = as_num(&operation.operands[0]);
                }
                "Ts" => {
                    gs.ts.rise = as_num(&operation.operands[0]);
                    text.push(' ');
                    current_line.push(' ');
                }
                "Tm" => {
                    assert!(operation.operands.len() == 6);
                    tlm = Transform2D::row_major(
                        as_num(&operation.operands[0]),
                        as_num(&operation.operands[1]),
                        as_num(&operation.operands[2]),
                        as_num(&operation.operands[3]),
                        as_num(&operation.operands[4]),
                        as_num(&operation.operands[5]),
                    );
                    gs.ts.tm = tlm;
                    dlog!("Tm: matrix {:?}", gs.ts.tm);
                }
                "Td" => {
                    assert!(operation.operands.len() == 2);
                    let tx = as_num(&operation.operands[0]);
                    let ty = as_num(&operation.operands[1]);
                    dlog!("translation: {} {}", tx, ty);

                    tlm = tlm.pre_transform(&Transform2D::create_translation(tx, ty));
                    gs.ts.tm = tlm;
                    dlog!("Td matrix {:?}", gs.ts.tm);
                }
                "TD" => {
                    assert!(operation.operands.len() == 2);
                    let tx = as_num(&operation.operands[0]);
                    let ty = as_num(&operation.operands[1]);
                    dlog!("translation: {} {}", tx, ty);
                    gs.ts.leading = -ty;

                    tlm = tlm.pre_transform(&Transform2D::create_translation(tx, ty));
                    gs.ts.tm = tlm;
                    dlog!("TD matrix {:?}", gs.ts.tm);
                }
                "T*" => {
                    let tx = 0.0;
                    let ty = -gs.ts.leading;

                    tlm = tlm.pre_transform(&Transform2D::create_translation(tx, ty));
                    gs.ts.tm = tlm;
                    dlog!("T* matrix {:?}", gs.ts.tm);
                }
                "q" => {
                    gs_stack.push(gs.clone());
                }
                "Q" => {
                    let s = gs_stack.pop();
                    if let Some(s) = s {
                        gs = s;
                    } else {
                        println!("No state to pop");
                    }
                }
                "gs" => {
                    let ext_gstate: &Dictionary = get(doc, resources, b"ExtGState");
                    let name = operation.operands[0].as_name().unwrap();
                    let state: &Dictionary = get(doc, ext_gstate, name);
                    apply_state(doc, &mut gs, state);
                }
                "i" => {
                    dlog!(
                        "unhandled graphics state flattness operator {:?}",
                        operation
                    );
                }
                "w" => {
                    gs.line_width = as_num(&operation.operands[0]);
                }
                "J" | "j" | "M" | "d" | "ri" => {
                    dlog!("unknown graphics state operator {:?}", operation);
                }
                "m" => path.ops.push(PathOp::MoveTo(
                    as_num(&operation.operands[0]),
                    as_num(&operation.operands[1]),
                )),
                "l" => path.ops.push(PathOp::LineTo(
                    as_num(&operation.operands[0]),
                    as_num(&operation.operands[1]),
                )),
                "c" => path.ops.push(PathOp::CurveTo(
                    as_num(&operation.operands[0]),
                    as_num(&operation.operands[1]),
                    as_num(&operation.operands[2]),
                    as_num(&operation.operands[3]),
                    as_num(&operation.operands[4]),
                    as_num(&operation.operands[5]),
                )),
                "v" => {
                    let (x, y) = path.current_point();
                    path.ops.push(PathOp::CurveTo(
                        x,
                        y,
                        as_num(&operation.operands[0]),
                        as_num(&operation.operands[1]),
                        as_num(&operation.operands[2]),
                        as_num(&operation.operands[3]),
                    ))
                }
                "y" => path.ops.push(PathOp::CurveTo(
                    as_num(&operation.operands[0]),
                    as_num(&operation.operands[1]),
                    as_num(&operation.operands[2]),
                    as_num(&operation.operands[3]),
                    as_num(&operation.operands[2]),
                    as_num(&operation.operands[3]),
                )),
                "h" => path.ops.push(PathOp::Close),
                "re" => path.ops.push(PathOp::Rect(
                    as_num(&operation.operands[0]),
                    as_num(&operation.operands[1]),
                    as_num(&operation.operands[2]),
                    as_num(&operation.operands[3]),
                )),
                "s" | "f*" | "B" | "B*" | "b" => {
                    dlog!("unhandled path op {:?}", operation);
                }
                "S" => {
                    path.ops.clear();
                }
                "F" | "f" => {
                    path.ops.clear();
                }
                "W" | "w*" => {
                    dlog!("unhandled clipping operation {:?}", operation);
                }
                "n" => {
                    dlog!("discard {:?}", path);
                    path.ops.clear();
                }
                "BMC" | "BDC" => {
                    mc_stack.push(operation);
                }
                "EMC" => {
                    mc_stack.pop();
                }
                "Do" => {
                    let xobject: &Dictionary = get(&doc, resources, b"XObject");
                    let name = operation.operands[0].as_name().unwrap();
                    let xf: &Stream = get(&doc, xobject, name);
                    
                    // Only process XObject if OCR handler is available
                    if let Some(handler) = ocr_handler {
                        let position = gs.ts.tm.post_transform(&flip_ctm);
                        let (x, y) = (position.m31, position.m32);
                        
                        // Extract dimensions if this is an image
                        let size = if let Ok(subtype) = xf.dict.get(b"Subtype") {
                            if let Ok(subtype_name) = subtype.as_name() {
                                if subtype_name == b"Image" {
                                    if let (Ok(width), Ok(height)) = (
                                        xf.dict.get(b"Width").and_then(|w| w.as_i64()),
                                        xf.dict.get(b"Height").and_then(|h| h.as_i64())
                                    ) {
                                        (width as f64, height as f64)
                                    } else {
                                        (100.0, 100.0) // Fallback if dimensions not found
                                    }
                                } else {
                                    (100.0, 100.0) // Not an image
                                }
                            } else {
                                (100.0, 100.0) // Invalid subtype
                            }
                        } else {
                            (100.0, 100.0) // No subtype
                        };

                        if let Err(e) = process_xobject(
                            &doc,
                            resources,
                            Some(handler),
                            text_segments,
                            (x, y),
                            size,
                            page_num,
                            current_font_size,
                            current_transformed_font_size,
                            &mut page_char_counter,
                        ) {
                            // Log error but continue processing
                            eprintln!("Failed to process image in PDF: {}", e);
                        }
                    }

                    // Continue with normal processing regardless of OCR result
                    let resources = maybe_get_obj(&doc, &xf.dict, b"Resources")
                        .and_then(|n| n.as_dict().ok())
                        .unwrap_or(resources);
                    let contents = get_contents(xf);
                    self.process_stream(
                        &doc,
                        ocr_handler,
                        contents,
                        resources,
                        &media_box,
                        page_num,
                        text_segments,
                    )?;
                }
                _ => {
                    dlog!("unknown operation {:?}", operation);
                }
            }
        }
        // if !current_word.is_empty() {
        //     current_line.push_str(&current_word);
        //     current_word.clear();
        // }
        if !current_line.is_empty() {
            // let processed_fill_color = if gs.fill_color.len() >= 3 {
            //     (
            //         (gs.fill_color[0] * 255.0).round() as u8,
            //         (gs.fill_color[1] * 255.0).round() as u8,
            //         (gs.fill_color[2] * 255.0).round() as u8,
            //     )
            // } else {
            //     (0, 0, 0) // Fallback to black if not enough color values are provided
            // };

            // if is_visible_text(Some(processed_fill_color), (255, 255, 255)) {
                let content_str = current_line.clone().trim().to_string();
                text_segments.push(Self::create_text_segment(
                    content_str,
                    current_font_size,
                    current_transformed_font_size,
                    current_x,
                    current_y,
                    current_is_bold,
                    current_font.clone(),
                    current_font_weight.clone(),
                    current_is_italic,
                    page_num,
                    "Tj".to_string(),
                    None,
                    None,
                    current_segment_start,
                    page_char_counter,
                ));
            // }
            current_line.clear();
        }

        // Update layout analyzer
        // layout_analyzer.update_threshold(current_font_size);
        // let is_section_break = layout_analyzer.is_section_break(current_y);

        // if is_section_break {
            // Add special marker for section boundaries
            // text_segments.push(TextSegment {
            //     content: "SECTION_BREAK".into(),
            //     font_size: current_font_size,
            //     transformed_font_size: current_transformed_font_size,
            //     x: current_x,
            //     y: current_y,
            //     is_bold: false,
            //     font_name: current_font.clone(),
            //     page_num: page_num,
            //     cutat: "SectionMarker".into(),
            //     fill_color: None,
            //     stroke_color: None,
            // });
        // }

        Ok(())
    }
}

pub trait OutputDev {
    fn begin_page(
        &mut self,
        page_num: u32,
        media_box: &MediaBox,
        art_box: Option<(f64, f64, f64, f64)>,
    ) -> Result<(), OutputError>;
    fn end_page(&mut self) -> Result<(), OutputError>;
    fn output_character(
        &mut self,
        trm: &Transform,
        width: f64,
        spacing: f64,
        font_size: f64,
        char: &str,
    ) -> Result<(), OutputError>;
    fn begin_word(&mut self) -> Result<(), OutputError>;
    fn end_word(&mut self) -> Result<(), OutputError>;
    fn end_line(&mut self) -> Result<(), OutputError>;
    fn stroke(
        &mut self,
        _ctm: &Transform,
        _colorspace: &ColorSpace,
        _color: &[f64],
        _path: &Path,
    ) -> Result<(), OutputError> {
        Ok(())
    }
    fn fill(
        &mut self,
        _ctm: &Transform,
        _colorspace: &ColorSpace,
        _color: &[f64],
        _path: &Path,
    ) -> Result<(), OutputError> {
        Ok(())
    }
}

pub struct HTMLOutput<'a> {
    file: &'a mut dyn std::io::Write,
    flip_ctm: Transform,
    last_ctm: Transform,
    buf_ctm: Transform,
    buf_font_size: f64,
    buf: String,
}

fn insert_nbsp(input: &str) -> String {
    let mut result = String::new();
    let mut word_end = false;
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c == ' ' {
            if !word_end || chars.peek().filter(|x| **x != ' ').is_none() {
                result += "&nbsp;";
            } else {
                result += " ";
            }
            word_end = false;
        } else {
            word_end = true;
            result.push(c);
        }
    }
    result
}

impl<'a> HTMLOutput<'a> {
    pub fn new(file: &mut dyn std::io::Write) -> HTMLOutput {
        HTMLOutput {
            file,
            flip_ctm: Transform2D::identity(),
            last_ctm: Transform2D::identity(),
            buf_ctm: Transform2D::identity(),
            buf: String::new(),
            buf_font_size: 0.,
        }
    }
    fn flush_string(&mut self) -> Result<(), OutputError> {
        if self.buf.len() != 0 {
            let position = self.buf_ctm.post_transform(&self.flip_ctm);
            let transformed_font_size_vec = self
                .buf_ctm
                .transform_vector(vec2(self.buf_font_size, self.buf_font_size));
            // get the length of one sized of the square with the same area with a rectangle of size (x, y)
            let transformed_font_size =
                (transformed_font_size_vec.x * transformed_font_size_vec.y).sqrt();
            let (x, y) = (position.m31, position.m32);
            println!("flush {} {:?}", self.buf, (x, y));

            write!(self.file, "<div style='position: absolute; left: {}px; top: {}px; font-size: {}px'>{}</div>\n",
                   x, y, transformed_font_size, insert_nbsp(&self.buf))?;
        }
        Ok(())
    }
}

type ArtBox = (f64, f64, f64, f64);

impl<'a> OutputDev for HTMLOutput<'a> {
    fn begin_page(
        &mut self,
        page_num: u32,
        media_box: &MediaBox,
        _: Option<ArtBox>,
    ) -> Result<(), OutputError> {
        write!(self.file, "<meta charset='utf-8' /> ")?;
        write!(self.file, "<!-- page {} -->", page_num)?;
        write!(self.file, "<div id='page{}' style='position: relative; height: {}px; width: {}px; border: 1px black solid'>", page_num, media_box.ury - media_box.lly, media_box.urx - media_box.llx)?;
        self.flip_ctm = Transform::row_major(1., 0., 0., -1., 0., media_box.ury - media_box.lly);
        Ok(())
    }
    fn end_page(&mut self) -> Result<(), OutputError> {
        self.flush_string()?;
        self.buf = String::new();
        self.last_ctm = Transform::identity();
        write!(self.file, "</div>")?;
        Ok(())
    }
    fn output_character(
        &mut self,
        trm: &Transform,
        width: f64,
        spacing: f64,
        font_size: f64,
        char: &str,
    ) -> Result<(), OutputError> {
        if trm.approx_eq(&self.last_ctm) {
            let position = trm.post_transform(&self.flip_ctm);
            let (x, y) = (position.m31, position.m32);

            println!("accum {} {:?}", char, (x, y));
            self.buf += char;
        } else {
            println!(
                "flush {} {:?} {:?} {} {} {}",
                char, trm, self.last_ctm, width, font_size, spacing
            );
            self.flush_string()?;
            self.buf = char.to_owned();
            self.buf_font_size = font_size;
            self.buf_ctm = *trm;
        }
        let position = trm.post_transform(&self.flip_ctm);
        let transformed_font_size_vec = trm.transform_vector(vec2(font_size, font_size));
        // get the length of one sized of the square with the same area with a rectangle of size (x, y)
        let transformed_font_size =
            (transformed_font_size_vec.x * transformed_font_size_vec.y).sqrt();
        let (x, y) = (position.m31, position.m32);
        write!(self.file, "<div style='position: absolute; color: red; left: {}px; top: {}px; font-size: {}px'>{}</div>",
               x, y, transformed_font_size, char)?;
        self.last_ctm = trm.pre_transform(&Transform2D::create_translation(
            width * font_size + spacing,
            0.,
        ));

        Ok(())
    }
    fn begin_word(&mut self) -> Result<(), OutputError> {
        Ok(())
    }
    fn end_word(&mut self) -> Result<(), OutputError> {
        Ok(())
    }
    fn end_line(&mut self) -> Result<(), OutputError> {
        Ok(())
    }
}

pub struct SVGOutput<'a> {
    file: &'a mut dyn std::io::Write,
}
impl<'a> SVGOutput<'a> {
    pub fn new(file: &mut dyn std::io::Write) -> SVGOutput {
        SVGOutput { file }
    }
}

impl<'a> OutputDev for SVGOutput<'a> {
    fn begin_page(
        &mut self,
        _page_num: u32,
        media_box: &MediaBox,
        art_box: Option<(f64, f64, f64, f64)>,
    ) -> Result<(), OutputError> {
        let ver = 1.1;
        write!(self.file, "<?xml version=\"1.0\" encoding=\"UTF-8\" ?>\n")?;
        if ver == 1.1 {
            write!(
                self.file,
                r#"<!DOCTYPE svg PUBLIC "-//W3C//DTD SVG 1.1//EN" "http://www.w3.org/Graphics/SVG/1.1/DTD/svg11.dtd">"#
            )?;
        } else {
            write!(
                self.file,
                r#"<!DOCTYPE svg PUBLIC "-//W3C//DTD SVG 1.0//EN" "http://www.w3.org/TR/2001/REC-SVG-20010904/DTD/svg10.dtd">"#
            )?;
        }
        if let Some(art_box) = art_box {
            let width = art_box.2 - art_box.0;
            let height = art_box.3 - art_box.1;
            let y = media_box.ury - art_box.1 - height;
            write!(self.file, "<svg width=\"{}\" height=\"{}\" xmlns=\"http://www.w3.org/2000/svg\" version=\"{}\" viewBox='{} {} {} {}'>", width, height, ver, art_box.0, y, width, height)?;
        } else {
            let width = media_box.urx - media_box.llx;
            let height = media_box.ury - media_box.lly;
            write!(self.file, "<svg width=\"{}\" height=\"{}\" xmlns=\"http://www.w3.org/2000/svg\" version=\"{}\" viewBox='{} {} {} {}'>", width, height, ver, media_box.llx, media_box.lly, width, height)?;
        }
        write!(self.file, "\n")?;
        type Mat = Transform;

        let ctm = Mat::create_scale(1., -1.).post_translate(vec2(0., media_box.ury));
        write!(
            self.file,
            "<g transform='matrix({}, {}, {}, {}, {}, {})'>\n",
            ctm.m11, ctm.m12, ctm.m21, ctm.m22, ctm.m31, ctm.m32,
        )?;
        Ok(())
    }
    fn end_page(&mut self) -> Result<(), OutputError> {
        write!(self.file, "</g>\n")?;
        write!(self.file, "</svg>")?;
        Ok(())
    }
    fn output_character(
        &mut self,
        _trm: &Transform,
        _width: f64,
        _spacing: f64,
        _font_size: f64,
        _char: &str,
    ) -> Result<(), OutputError> {
        Ok(())
    }
    fn begin_word(&mut self) -> Result<(), OutputError> {
        Ok(())
    }
    fn end_word(&mut self) -> Result<(), OutputError> {
        Ok(())
    }
    fn end_line(&mut self) -> Result<(), OutputError> {
        Ok(())
    }
    fn fill(
        &mut self,
        ctm: &Transform,
        _colorspace: &ColorSpace,
        _color: &[f64],
        path: &Path,
    ) -> Result<(), OutputError> {
        write!(
            self.file,
            "<g transform='matrix({}, {}, {}, {}, {}, {})'>",
            ctm.m11, ctm.m12, ctm.m21, ctm.m22, ctm.m31, ctm.m32,
        )?;

        /*if path.ops.len() == 1 {
            if let PathOp::Rect(x, y, width, height) = path.ops[0] {
                write!(self.file, "<rect x={} y={} width={} height={} />\n", x, y, width, height);
                write!(self.file, "</g>");
                return;
            }
        }*/
        let mut d = Vec::new();
        for op in &path.ops {
            match op {
                &PathOp::MoveTo(x, y) => d.push(format!("M{} {}", x, y)),
                &PathOp::LineTo(x, y) => d.push(format!("L{} {}", x, y)),
                &PathOp::CurveTo(x1, y1, x2, y2, x, y) => {
                    d.push(format!("C{} {} {} {} {} {}", x1, y1, x2, y2, x, y))
                }
                &PathOp::Close => d.push(format!("Z")),
                &PathOp::Rect(x, y, width, height) => {
                    d.push(format!("M{} {}", x, y));
                    d.push(format!("L{} {}", x + width, y));
                    d.push(format!("L{} {}", x + width, y + height));
                    d.push(format!("L{} {}", x, y + height));
                    d.push(format!("Z"));
                }
            }
        }
        write!(self.file, "<path d='{}' />", d.join(" "))?;
        write!(self.file, "</g>")?;
        write!(self.file, "\n")?;
        Ok(())
    }
}

/*
File doesn't implement std::fmt::Write so we have
to do some gymnastics to accept a File or String
See https://github.com/rust-lang/rust/issues/51305
*/

pub trait ConvertToFmt {
    type Writer: std::fmt::Write;
    fn convert(self) -> Self::Writer;
}

impl<'a> ConvertToFmt for &'a mut String {
    type Writer = &'a mut String;
    fn convert(self) -> Self::Writer {
        self
    }
}

pub struct WriteAdapter<W> {
    f: W,
}

impl<W: std::io::Write> std::fmt::Write for WriteAdapter<W> {
    fn write_str(&mut self, s: &str) -> Result<(), std::fmt::Error> {
        self.f.write_all(s.as_bytes()).map_err(|_| fmt::Error)
    }
}

impl<'a> ConvertToFmt for &'a mut dyn std::io::Write {
    type Writer = WriteAdapter<Self>;
    fn convert(self) -> Self::Writer {
        WriteAdapter { f: self }
    }
}

impl<'a> ConvertToFmt for &'a mut File {
    type Writer = WriteAdapter<Self>;
    fn convert(self) -> Self::Writer {
        WriteAdapter { f: self }
    }
}

pub struct PlainTextOutput<W: ConvertToFmt> {
    writer: W::Writer,
    last_end: f64,
    last_y: f64,
    first_char: bool,
    flip_ctm: Transform,
}

impl<W: ConvertToFmt> PlainTextOutput<W> {
    pub fn new(writer: W) -> PlainTextOutput<W> {
        PlainTextOutput {
            writer: writer.convert(),
            last_end: 100000.,
            first_char: false,
            last_y: 0.,
            flip_ctm: Transform2D::identity(),
        }
    }
}

/* There are some structural hints that PDFs can use to signal word and line endings:
 * however relying on these is not likely to be sufficient. */
impl<W: ConvertToFmt> OutputDev for PlainTextOutput<W> {
    fn begin_page(
        &mut self,
        _page_num: u32,
        media_box: &MediaBox,
        _: Option<ArtBox>,
    ) -> Result<(), OutputError> {
        self.flip_ctm = Transform2D::row_major(1., 0., 0., -1., 0., media_box.ury - media_box.lly);
        Ok(())
    }
    fn end_page(&mut self) -> Result<(), OutputError> {
        Ok(())
    }
    fn output_character(
        &mut self,
        trm: &Transform,
        width: f64,
        _spacing: f64,
        font_size: f64,
        char: &str,
    ) -> Result<(), OutputError> {
        let position = trm.post_transform(&self.flip_ctm);
        let transformed_font_size_vec = trm.transform_vector(vec2(font_size, font_size));
        // get the length of one sized of the square with the same area with a rectangle of size (x, y)
        let transformed_font_size =
            (transformed_font_size_vec.x * transformed_font_size_vec.y).sqrt();
        let (x, y) = (position.m31, position.m32);
        use std::fmt::Write;
        //dlog!("last_end: {} x: {}, width: {}", self.last_end, x, width);
        if self.first_char {
            // Check for paragraph break
            if is_paragraph_break(y, self.last_y, transformed_font_size) {
                write!(self.writer, "\n")?;
            }

            // we've moved to the left and down (new line)
            if is_new_line(x, self.last_end, y, self.last_y, transformed_font_size) {
                write!(self.writer, " ")?;
            }

            if should_insert_space(x, self.last_end, transformed_font_size) {
                dlog!(
                    "width: {}, space: {}, thresh: {}",
                    width,
                    x - self.last_end,
                    if transformed_font_size < 10.0 { 
                        transformed_font_size * MIN_SPACE_GAP 
                    } else { 
                        transformed_font_size * SPACE_THRESHOLD_RATIO 
                    }
                );
                write!(self.writer, " ")?;
            }
        }
        //let norm = unicode_normalization::UnicodeNormalization::nfkc(char);
        write!(self.writer, "{}", char)?;
        self.first_char = false;
        self.last_y = y;
        self.last_end = x + width * transformed_font_size;
        Ok(())
    }
    fn begin_word(&mut self) -> Result<(), OutputError> {
        self.first_char = true;
        Ok(())
    }
    fn end_word(&mut self) -> Result<(), OutputError> {
        Ok(())
    }
    fn end_line(&mut self) -> Result<(), OutputError> {
        //write!(self.file, "\n");
        Ok(())
    }
}

pub fn print_metadata(doc: &Document) {
    dlog!("Version: {}", doc.version);
    if let Some(ref info) = get_info(&doc) {
        for (k, v) in *info {
            match v {
                &Object::String(ref s, StringFormat::Literal) => {
                    dlog!("{}: {}", pdf_to_utf8(k), pdf_to_utf8(s));
                }
                _ => {}
            }
        }
    }
    dlog!(
        "Page count: {}",
        get::<i64>(&doc, &get_pages(&doc), b"Count")
    );
    dlog!("Pages: {:?}", get_pages(&doc));
    dlog!(
        "Type: {:?}",
        get_pages(&doc)
            .get(b"Type")
            .and_then(|x| x.as_name())
            .unwrap()
    );
}

/// Extract the text from a pdf at `path` and return a `String` with the results
pub fn extract_text<P: std::convert::AsRef<std::path::Path>>(
    path: P,
    ocr_handler: Option<&OcrHandler>,
) -> Result<String, OutputError> {
    let mut s = String::new();
    {
        // let mut output = PlainTextOutput::new(&mut s);
        let mut doc = Document::load(path)?;
        maybe_decrypt(&mut doc)?;
        output_doc(&doc, ocr_handler).map_err(|e| OutputError::Other(e.to_string()));
    }
    Ok(s)
}

fn maybe_decrypt(doc: &mut Document) -> Result<(), OutputError> {
    if !doc.is_encrypted() {
        return Ok(());
    }

    if let Err(e) = doc.decrypt("") {
        if let Error::Decryption(DecryptionError::IncorrectPassword) = e {
            eprintln!("Encrypted documents must be decrypted with a password using {{extract_text|extract_text_from_mem|output_doc}}_encrypted")
        }

        return Err(OutputError::PdfError(e));
    }

    Ok(())
}

// pub fn extract_text_encrypted<P: std::convert::AsRef<std::path::Path>, PW: AsRef<[u8]>>(
//     path: P,
//     password: PW,
// ) -> Result<String, OutputError> {
//     let mut s = String::new();
//     {
//         // let mut output = PlainTextOutput::new(&mut s);
//         let mut doc = Document::load(path)?;
//         output_doc_encrypted(&mut doc, password)?;
//     }
//     Ok(s)
// }

pub fn extract_text_from_mem(
    buffer: &[u8],
    ocr_handler: Option<&OcrHandler>,
) -> Result<String, OutputError> {
    let mut s = String::new();
    {
        // let mut output = PlainTextOutput::new(&mut s);
        let mut doc = Document::load_mem(buffer)?;
        maybe_decrypt(&mut doc)?;
        output_doc(&doc, ocr_handler).map_err(|e| OutputError::Other(e.to_string()));
    }
    Ok(s)
}

// pub fn extract_text_from_mem_encrypted<PW: AsRef<[u8]>>(
//     buffer: &[u8],
//     password: PW,
// ) -> Result<String, OutputError> {
//     let mut s = String::new();
//     {
//         // let mut output = PlainTextOutput::new(&mut s);
//         let mut doc = Document::load_mem(buffer)?;
//         output_doc_encrypted(&mut doc, password)?;
//     }
//     Ok(s)
// }

fn get_inherited<'a, T: FromObj<'a>>(
    doc: &'a Document,
    dict: &'a Dictionary,
    key: &[u8],
) -> Option<T> {
    let o: Option<T> = get(doc, dict, key);
    if let Some(o) = o {
        Some(o)
    } else {
        let parent = dict
            .get(b"Parent")
            .and_then(|parent| parent.as_reference())
            .and_then(|id| doc.get_dictionary(id))
            .ok()?;
        get_inherited(doc, parent, key)
    }
}

// pub fn output_doc_encrypted<PW: AsRef<[u8]>>(
//     doc: &mut Document,
//     // output: &mut dyn OutputDev,
//     password: PW,
// ) -> Result<Vec<ContentOutput>, OutputError> {
//     doc.decrypt(password)?;
//     output_doc(doc, ocr, detection_model, recognition_model)
// }

// #[derive(Debug)]
// pub struct ContentOutput {
//     pub headings: Vec<String>,
//     pub paragraph: String,
//     pub page: u32,
// }
/// Parse a given document and output it to `output`

fn calculate_document_stats(lines: &[TextSegment]) -> DocumentStats {
    let mut font_stats: HashMap<String, FontStats> = HashMap::new();

    // Collect statistics

    // Collect statistics
    for segment in lines {
        let font_stat = font_stats
            .entry(segment.font_name.clone())
            .or_insert_with(|| FontStats {
                sizes: HashMap::new(),
                transformed_sizes: HashMap::new(),
                size_to_transformed: HashMap::new(),
                total_chars: 0,
            });

        *font_stat
            .sizes
            .entry(OrderedFloat(segment.font_size))
            .or_insert(0) += segment.content.len();
        *font_stat
            .transformed_sizes
            .entry(OrderedFloat(segment.transformed_font_size))
            .or_insert(0) += segment.content.len();
    }
    let (body_font, body_size, body_transformed_size) = font_stats
        .iter()
        .flat_map(|(font_name, stats)| {
            stats
                .transformed_sizes
                .iter()
                .map(move |(&size, &count)| (font_name, size, count))
        })
        .max_by_key(|&(_, _, count)| count)
        .map(|(font, size, _)| (font.clone(), size, size))
        .unwrap_or((
            "".to_string(),
            ordered_float::OrderedFloat(0.0),
            ordered_float::OrderedFloat(0.0),
        ));

    // Find the body size (most common size across all fonts)

    println!("body_transformed_size: {}", body_transformed_size);
    // Calculate six threshold levels based on transformed sizes
    let all_transformed_sizes: Vec<f64> = font_stats
        .values()
        .flat_map(|stats| stats.transformed_sizes.keys().cloned())
        .map(|ordered_float| ordered_float.into_inner())
        .collect();
    let transformed_thresholds =
        calculate_heading_thresholds(&all_transformed_sizes, *body_transformed_size);
    println!("transformed_thresholdsa: {:?}", transformed_thresholds);

    // Calculate font-specific thresholds
    let mut font_heading_thresholds = HashMap::new();
    for (font_name, stats) in font_stats.iter() {
        let font_thresholds: Vec<f64> = transformed_thresholds
            .iter()
            .filter_map(|&transformed_size| {
                stats
                    .size_to_transformed
                    .iter()
                    .find(|&(_, &t)| (t - transformed_size).abs() < 0.01)
                    .map(|(&original_size, _)| original_size.into_inner())
            })
            .collect();
        font_heading_thresholds.insert(font_name.clone(), font_thresholds);
    }

    DocumentStats {
        font_stats,
        body_font,
        body_size: *body_size,
        body_transformed_size: *body_transformed_size,
        transformed_thresholds,
        font_heading_thresholds,
    }
}

#[derive(Debug)]
struct FontStats {
    sizes: HashMap<OrderedFloat<f64>, usize>,
    transformed_sizes: HashMap<OrderedFloat<f64>, usize>,
    size_to_transformed: HashMap<OrderedFloat<f64>, f64>,

    total_chars: usize,
}

#[derive(Debug)]
struct DocumentStats {
    font_stats: HashMap<String, FontStats>,
    body_font: String,
    body_size: f64,
    body_transformed_size: f64,
    transformed_thresholds: Vec<f64>,
    font_heading_thresholds: HashMap<String, Vec<f64>>,
}

fn calculate_mode(numbers: &[f64]) -> f64 {
    let mut counts = HashMap::new();
    for &num in numbers {
        *counts.entry((num * 100.0).round() as i64).or_insert(0) += 1;
    }
    let mode = counts
        .into_iter()
        .max_by_key(|&(_, count)| count)
        .unwrap()
        .0;
    mode as f64 / 100.0
}

fn calculate_heading_thresholds(sizes: &[f64], body_size: f64) -> Vec<f64> {
    let mut thresholds: Vec<f64> = sizes
        .iter()
        .filter(|&&size| size > body_size * 1.1)
        .cloned()
        .collect();

    thresholds.sort_by(|a, b| b.partial_cmp(a).unwrap()); // Sort in descending order
    thresholds.dedup(); // Remove duplicates

    if thresholds.len() <= 6 {
        thresholds
    } else {
        // Group into 6 levels
        let step = thresholds.len() / 6;
        let mut result: Vec<f64> = (0..6).map(|i| thresholds[i * step]).collect();
        result.dedup(); // Remove any potential duplicates after grouping
        result
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
enum TextLevel {
    H1,
    H2,
    H3,
    H4,
    H5,
    H6,
    Body,
    SubBody,
}
fn is_heading(segment: &TextSegment, doc_stats: &DocumentStats) -> TextLevel {
    // let fg_color = segment.fill_color.unwrap_or((0, 0, 0)); // default black
    // let bg_color = segment.stroke_color.unwrap_or((255, 255, 255)); // default white

    // // Calculate contrast ratio
    // let cr = contrast_ratio(fg_color, bg_color);

    // // Filter low-contrast text (adjust threshold as needed)
    // if cr < 1.5 {
    //     // Catches near-invisible text but allows light gray
    //     return TextLevel::Body;
    // }

    // Additional heading checks

    // if segment.content first char is lower case then it is not a heading
   


    let is_short = segment.content.split_whitespace().count() < 10;
    // let not_short = segment.content.len() > 4;
    let starts_with_alphanum = segment
        .content
        .trim()
        .chars()
        .next()
        .map_or(false, |c| c.is_alphanumeric());

    let is_valid_short_content = |content: &str| {
        let trimmed = content.trim();
        if trimmed.len() >= 4 {
            true
        } else {
            // Check if it's a number or Roman numeral
            trimmed.parse::<u32>().is_ok() || is_roman_numeral(trimmed)
        }
    };

    // let is_short = segment.content.split_whitespace().count() < 10 && is_valid_short_content(&segment.content);

    fn is_roman_numeral(s: &str) -> bool {
        let valid_chars = ['I', 'V', 'X'];
        !s.is_empty() && s.chars().all(|c| valid_chars.contains(&c))
        
    }

    let is_numbered = NUMBERED_HEADING.is_match(&segment.content);

    let is_potential_heading =
        is_short && (starts_with_alphanum && is_valid_short_content(&segment.content) || is_numbered);
        

    // Check if the segment matches the body font and size
    if segment.font_name == doc_stats.body_font
        && (segment.transformed_font_size - doc_stats.body_transformed_size).abs() < 0.1
    {
        // If it matches body font and size, check if it's bold
        if segment.is_bold && is_potential_heading {
            return TextLevel::H6;
        } else {
            return TextLevel::Body;
        }
    }
    // If it's smaller than the body text, consider it sub-body
    else if segment.transformed_font_size < doc_stats.body_transformed_size {
        return TextLevel::SubBody;
    } else 
    if (segment.transformed_font_size - doc_stats.body_transformed_size).abs() < 0.1 {
        if segment.is_bold  {
            if !segment.content.chars().next().unwrap().is_lowercase() && is_potential_heading {
                return TextLevel::H6;
            } else {
                return TextLevel::Body;
            }
        } else {
            return TextLevel::Body;
        }
    }
    // Find the closest heading level
    else  
    if is_potential_heading {
    let closest_threshold = match doc_stats.transformed_thresholds.iter().min_by(|&&a, &&b| {
    (a - segment.transformed_font_size)
        .abs()
        .partial_cmp(&(b - segment.transformed_font_size).abs())
        .unwrap()
}) {
    Some(threshold) => threshold,
    None => return TextLevel::Body,
};

        let index = doc_stats
            .transformed_thresholds
            .iter()
            .position(|&r| r == *closest_threshold)
            .unwrap();

        return match index {
            0 => TextLevel::H1,
            1 => TextLevel::H2,
            2 => TextLevel::H3,
            3 => TextLevel::H4,
            4 => TextLevel::H5,
            _ => TextLevel::H6,
        };
    } else {
        // If we can't determine a specific level, return Body as fallback
        return TextLevel::Body;
    }

    // if is_numbered {
    //     return TextLevel::H1;
    // } else {
    //     return TextLevel::Body;
    // }
}

pub fn output_doc(
    doc: &Document,
    ocr_handler: Option<&OcrHandler>,
) -> Result<Vec<ContentOutput>, Box<dyn std::error::Error>> {
    let mut document_structure: Vec<ContentOutput> = Vec::new();

    // println!("Shaata");
    if doc.is_encrypted() {
        eprintln!("Encrypted documents must be decrypted with a password using {{extract_text|extract_text_from_mem|output_doc}}_encrypted");
    }
    let empty_resources = &Dictionary::new();

    let pages = doc.get_pages();
    // trim to only first page
    // let pages: BTreeMap<u32, (u32, u16)> = pages
    //     .iter()
    //     .filter(|(&k, _)| k >= 1 && k <= 3)
    //     .map(|(&k, &v)| (k, v))
    //     .collect();
    // let toc = doc.get_toc();

    let form = match form_fields(&doc, &mut document_structure) {
        Ok(form) => form,
        Err(e) => {
            println!("Error: {:#?}", e);
            // None
        }
    };

    // Process pages in parallel
     // Change from flat_map to map to preserve page boundaries
    let page_results: Vec<(u32, Vec<TextSegment>)> = pages
        .par_iter()
        .map(|dict| {
            let page_num = dict.0;
            let page_dict = doc.get_object(*dict.1).unwrap().as_dict().unwrap();
            let resources = get_inherited(doc, page_dict, b"Resources").unwrap_or(empty_resources);
            
            let media_box: Vec<f64> = get_inherited(doc, page_dict, b"MediaBox").expect("MediaBox");
            let media_box = MediaBox {
                llx: media_box[0],
                lly: media_box[1],
                urx: media_box[2],
                ury: media_box[3],
            };

            let mut p = Processor::new();
            let mut page_segments = Vec::new();
            
            p.process_stream(
                &doc,
                ocr_handler,
                doc.get_page_content(*dict.1).unwrap(),
                resources,
                &media_box,
                *page_num,
                &mut page_segments,
            )
            .unwrap();

            (*dict.0, page_segments)  // Return tuple of (page_num, segments)
        })
        .collect();

    // Sort by page number and flatten while maintaining order
    let text_segments: Vec<TextSegment> = page_results
        .into_iter()
        .sorted_by_key(|(page_num, _)| *page_num)
        .flat_map(|(_, segments)| segments)
        .collect();
    
    // Create header/footer detector and analyze the document
    let mut header_footer_detector = HeaderFooterDetector::new(pages.len());
    
    // Feed all text segments to the detector
    for segment in &text_segments {
        // Get the page height from the media box (this is approximate)
        let page_height = 800.0; // Default page height, could be extracted from media box
        header_footer_detector.add_occurrence(
            &segment.content,
            segment.page_num,
            segment.y,
            segment.font_size,
            page_height
        );
    }
    
    // Get the detected headers and footers
    let headers_footers = header_footer_detector.analyze();
    
    // Filter out headers and footers from text segments
    let text_segments: Vec<TextSegment> = text_segments
        .into_iter()
        .filter(|seg| !headers_footers.contains(&seg.content))
        .collect();

    // The rest of the function remains sequential to ensure that the document structure is created in the correct order
    // The rest of the function remains sequential to ensure that the document structure is created in the correct order
    // let doc_stats = calculate_document_stats(text_segments.clone());

    let mut current_headings: Vec<String> = vec![String::new(); 8];
    let mut current_paragraph = String::new();
    let mut current_segments: Vec<TextSegment> = Vec::new(); // Track segments that form current paragraph

    // let footer_threshold = doc_stats.median_line_height * 0.8; // Adjust this value as needed
    let mut last_page_num = 0;
    let mut last_y = 0.0;
    let mut last_end = 0.0;
    let doc_stats = calculate_document_stats(&text_segments.clone());

    // for segment in text_segments.clone() {
    //     println!("{:#?}", segment);
    // }
    // let processed_segments = process_document(text_segments);

    // println!("processed_segments: {:#?}", processed_segments);

    // Add these constants at the top level
    const MIN_CONTRAST_RATIO: f64 = 4.5; // WCAG AA standard
    const MIN_COLOR_DIFF: u8 = 30; // Minimum RGB difference

    // Add these helper functions
    fn calculate_luminance(color: (u8, u8, u8)) -> f64 {
        let srgb = |c: u8| {
            let c = c as f64 / 255.0;
            if c <= 0.03928 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };

        0.2126 * srgb(color.0) + 0.7152 * srgb(color.1) + 0.0722 * srgb(color.2)
    }

    fn contrast_ratio(fg: (u8, u8, u8), bg: (u8, u8, u8)) -> f64 {
        let l1 = calculate_luminance(fg);
        let l2 = calculate_luminance(bg);
        let (lighter, darker) = if l1 > l2 { (l1, l2) } else { (l2, l1) };
        (lighter + 0.05) / (darker + 0.05)
    }

    fn color_difference(c1: (u8, u8, u8), c2: (u8, u8, u8)) -> u8 {
        ((c1.0 as i16 - c2.0 as i16).abs()
            + (c1.1 as i16 - c2.1 as i16).abs()
            + (c1.2 as i16 - c2.2 as i16).abs()) as u8
    }

    fn is_visible_text(fill_color: Option<(u8, u8, u8)>, bg_color: (u8, u8, u8)) -> bool {
        let fg_color = fill_color.unwrap_or((0, 0, 0)); // Default to black if no color specified

        // Check if it's a cyan-like color (commonly used for masking)
        let is_cyan_like = fg_color.0 < 50 && fg_color.1 < 50 && fg_color.2 > 100;
        if is_cyan_like {
            return false;
        }

        // Check contrast ratio
        let cr = contrast_ratio(fg_color, bg_color);
        if cr < MIN_CONTRAST_RATIO {
            return false;
        }

        // Check absolute color difference
        let cd = color_difference(fg_color, bg_color);
        if cd < MIN_COLOR_DIFF {
            return false;
        }

        true
    }

    // let mut current_numbering: Vec<String> = vec![String::new(); 8]; // Track numbering per level

    for segment in text_segments {
        let level = is_heading(&segment, &doc_stats);
        
        // Debug output for heading detection
        // if level != TextLevel::Body && level != TextLevel::SubBody {
        //     eprintln!("DEBUG: Detected heading level {:?} for text: {} (bold: {}, size: {})", 
        //         level, &segment.content.chars().take(50).collect::<String>(), 
        //         segment.is_bold, segment.transformed_font_size);
        // }
        
        match level {
            TextLevel::H1
            | TextLevel::H2
            | TextLevel::H3
            | TextLevel::H4
            | TextLevel::H5
            | TextLevel::H6
         => {
                // NEW SINGLE HEADING MODE:
                // If there's already some paragraph content, flush it out with the last heading.
                if !current_paragraph.is_empty() && current_paragraph.trim().len() > 35 {
                    let last_heading = if current_headings[0].is_empty() {
                        "".to_string()
                    } else {
                        current_headings[0].clone()
                    };
                    
                    // Calculate position data from segments with proper multi-page handling
                    let (start_page, end_page, page_char_start, page_char_end, bbox, page_positions) = 
                        if !current_segments.is_empty() {
                            let first_seg = &current_segments[0];
                            let last_seg = current_segments.last().unwrap();
                            
                            // Get the actual start and end pages from content
                            let start_page = first_seg.page_num;
                            let end_page = if last_seg.page_num != start_page { 
                                Some(last_seg.page_num) 
                            } else { 
                                None 
                            };
                            
                            // Get character positions (page-relative)
                            let char_start = first_seg.char_start;
                            let char_end = last_seg.char_end;
                            
                            // Build per-page position info
                            let mut page_positions = Vec::new();
                            let mut current_page = first_seg.page_num;
                            let mut page_segments = Vec::new();
                            
                            for seg in &current_segments {
                                if seg.page_num != current_page {
                                    // Process segments for the previous page
                                    if !page_segments.is_empty() {
                                        let page_bbox = calculate_bbox_for_segments(&page_segments);
                                        page_positions.push(PagePosition {
                                            page: current_page,
                                            char_start: page_segments[0].char_start,
                                            char_end: page_segments.last().unwrap().char_end,
                                            bbox: page_bbox,
                                        });
                                    }
                                    // Start new page
                                    current_page = seg.page_num;
                                    page_segments.clear();
                                }
                                page_segments.push(seg.clone());
                            }
                            
                            // Don't forget the last page
                            if !page_segments.is_empty() {
                                let page_bbox = calculate_bbox_for_segments(&page_segments);
                                page_positions.push(PagePosition {
                                    page: current_page,
                                    char_start: page_segments[0].char_start,
                                    char_end: page_segments.last().unwrap().char_end,
                                    bbox: page_bbox,
                                });
                            }
                            
                            // Overall bounding box (for single-page content)
                            let bbox = if end_page.is_none() {
                                Some(calculate_bbox_for_segments(&current_segments))
                            } else {
                                None // Multi-page content doesn't have a single bbox
                            };
                            
                            (start_page, end_page, Some(char_start), Some(char_end), bbox, page_positions)
                        } else {
                            // eprintln!("DEBUG: No segments for position tracking");
                            (segment.page_num, None, None, None, None, Vec::new())
                        };
                    
                    let output = ContentOutput {
                        headings: if last_heading.is_empty() { vec![] } else { vec![last_heading] },
                        paragraph: current_paragraph.trim().to_string(),
                        page: start_page,
                        end_page,
                        page_char_start,
                        page_char_end,
                        bbox,
                        page_positions,
                    };
                    eprintln!("DEBUG: Creating ContentOutput with positions - start: {:?}, end: {:?}, bbox: {:?}", 
                        output.page_char_start, output.page_char_end, output.bbox);
                    document_structure.push(output);
                    current_paragraph.clear();
                    current_segments.clear();
                }

                // Clear all headings so that we retain only the current one.
                for h in current_headings.iter_mut() {
                    *h = "".to_string();
                }
                current_headings[0] = segment.content.trim().to_string();

                // -------------------------------------------------------------------------
                // The code below implements the old multi-heading (stacked) behavior.
                // Uncomment it if you need to revert to the multi-level headings later.
                /*
                let heading_level = match level {
                    TextLevel::H1 => 1,
                    TextLevel::H2 => 2,
                    TextLevel::H3 => 3,
                    TextLevel::H4 => 4,
                    TextLevel::H5 => 5,
                    TextLevel::H6 => 6,
                    TextLevel::H7 => 7,
                    _ => unreachable!(),
                };

                // Clear all lower heading levels immediately
                for i in (heading_level + 1)..=7 {
                    if i < current_headings.len() {
                        current_headings[i].clear();
                    }
                }

                // Only propagate heading if it's a new section starter
                if !current_paragraph.is_empty() && current_paragraph.trim().len() > 35 {
                    document_structure.push(ContentOutput {
                        headings: current_headings
                            .iter()
                            .take(heading_level + 1)
                            .filter(|h| !h.is_empty())
                            .cloned()
                            .collect(),
                        paragraph: current_paragraph.trim().to_string(),
                        page: segment.page_num,
                    });
                    current_paragraph.clear();
                    current_headings[heading_level] = segment.content.trim().to_string();
                } else {
                    // Merge with existing heading if same level
                    if current_headings[heading_level].is_empty() {
                        current_headings[heading_level] = segment.content.trim().to_string();
                    } else {
                        current_headings[heading_level] = format!(
                            "{} {}",
                            current_headings[heading_level],
                            segment.content.trim()
                        );
                    }
                }

                // Clear any higher levels if this is a new top-level heading
                if heading_level
                    < current_headings
                        .iter()
                        .rposition(|h| !h.is_empty())
                        .unwrap_or(0)
                {
                    for i in 0..heading_level {
                        current_headings[i].clear();
                    }
                }
                let is_numbered = NUMBERED_HEADING.is_match(&segment.content);
                // Update numbering context
                if is_numbered {
                    current_numbering[heading_level] = extract_numbering(&segment.content);
                    // Reset subordinate levels
                    for i in (heading_level + 1)..current_numbering.len() {
                        current_numbering[i].clear();
                    }
                }

                // Add numbering to heading text
                if !current_numbering[heading_level].is_empty() {
                    current_headings[heading_level] = format!(
                        "{} {}",
                        current_numbering[heading_level],
                        segment.content.trim()
                    );
                }
                */
            }
            TextLevel::Body | TextLevel::SubBody => {
                if !current_paragraph.is_empty() {
                    current_paragraph.push(' ');
                }
                current_paragraph.push_str(&segment.content);
                
                // Track this segment for position calculation
                // eprintln!("DEBUG: Adding segment - char_start: {}, char_end: {}, x: {}, y: {}", 
                //     segment.char_start, segment.char_end, segment.x, segment.y);
                current_segments.push(segment.clone());

                // Check if the paragraph length exceeds 1000 characters
                // if current_paragraph.len() > 20000 {
                //     document_structure.push(ContentOutput {
                //         headings: current_headings
                //             .clone()
                //             .into_iter()
                //             .filter(|h| !h.is_empty())
                //             .collect(),
                //         paragraph: current_paragraph.trim().to_string(),
                //         page: last_page_num,
                //     });
                //     current_paragraph.clear();
                // }

                last_page_num = segment.page_num;
            }
        }
    }

    // Push the last paragraph if it's not empty
    if !current_paragraph.is_empty() {
        // Calculate position data from segments with proper multi-page handling
        let (start_page, end_page, page_char_start, page_char_end, bbox, page_positions) = 
            if !current_segments.is_empty() {
                let first_seg = &current_segments[0];
                let last_seg = current_segments.last().unwrap();
                
                // Get the actual start and end pages from content
                let start_page = first_seg.page_num;
                let end_page = if last_seg.page_num != start_page { 
                    Some(last_seg.page_num) 
                } else { 
                    None 
                };
                
                // Get character positions (page-relative)
                let char_start = first_seg.char_start;
                let char_end = last_seg.char_end;
                
                // Build per-page position info
                let mut page_positions = Vec::new();
                let mut current_page = first_seg.page_num;
                let mut page_segments = Vec::new();
                
                for seg in &current_segments {
                    if seg.page_num != current_page {
                        // Process segments for the previous page
                        if !page_segments.is_empty() {
                            let page_bbox = calculate_bbox_for_segments(&page_segments);
                            page_positions.push(PagePosition {
                                page: current_page,
                                char_start: page_segments[0].char_start,
                                char_end: page_segments.last().unwrap().char_end,
                                bbox: page_bbox,
                            });
                        }
                        // Start new page
                        current_page = seg.page_num;
                        page_segments.clear();
                    }
                    page_segments.push(seg.clone());
                }
                
                // Don't forget the last page
                if !page_segments.is_empty() {
                    let page_bbox = calculate_bbox_for_segments(&page_segments);
                    page_positions.push(PagePosition {
                        page: current_page,
                        char_start: page_segments[0].char_start,
                        char_end: page_segments.last().unwrap().char_end,
                        bbox: page_bbox,
                    });
                }
                
                // Overall bounding box (for single-page content)
                let bbox = if end_page.is_none() {
                    Some(calculate_bbox_for_segments(&current_segments))
                } else {
                    None // Multi-page content doesn't have a single bbox
                };
                
                (start_page, end_page, Some(char_start), Some(char_end), bbox, page_positions)
            } else {
                (last_page_num, None, None, None, None, Vec::new())
            };
        
        document_structure.push(ContentOutput {
            headings: current_headings
                .clone()
                .into_iter()
                .filter(|h| !h.is_empty())
                .collect(),
            paragraph: current_paragraph.trim().to_string(),
            page: start_page,
            end_page,
            page_char_start,
            page_char_end,
            bbox,
            page_positions,
        });
    }

    Ok(document_structure)
}

pub fn parse_pdf(
    file: &str,
    ocr: Option<bool>,
    detection_model: Option<String>,
    recognition_model: Option<String>,
) -> Result<Vec<ContentOutput>, OutputError> {
    let path = path::Path::new(&file);

    let doc = Document::load(path)?;

    if ocr.unwrap_or(false) {
        println!("ocr");
        let ocr_config = OcrConfig {
            detection_model,
            recognition_model,
        };
        let ocr_handler = OcrHandler::new(&ocr_config).map_err(|e| {
            println!("ocr_handler error: {}", e);
            OutputError::Other(e.to_string())
        })?;

        output_doc(&doc, Some(&ocr_handler)).map_err(|e| OutputError::Other(e.to_string()))
    } else {
        output_doc(&doc, None).map_err(|e| OutputError::Other(e.to_string()))
    }
}

// // Add near document processing logic
// fn calculate_luminance(color: (u8, u8, u8)) -> f64 {
//     let srgb = |c: u8| {
//         let c = c as f64 / 255.0;
//         if c <= 0.03928 {
//             c / 12.92
//         } else {
//             ((c + 0.055) / 1.055).powf(2.4)
//         }
//     };

//     0.2126 * srgb(color.0) + 0.7152 * srgb(color.1) + 0.0722 * srgb(color.2)
// }

// fn contrast_ratio(fg: (u8, u8, u8), bg: (u8, u8, u8)) -> f64 {
//     let lum1 = calculate_luminance(fg);
//     let lum2 = calculate_luminance(bg);
//     let (lighter, darker) = if lum1 > lum2 {
//         (lum1, lum2)
//     } else {
//         (lum2, lum1)
//     };

//     (lighter + 0.05) / (darker + 0.05)
// }

// Add these constants at the top level
const MIN_CONTRAST_RATIO: f64 = 4.5; // WCAG AA standard
const MIN_COLOR_DIFF: u8 = 30; // Minimum RGB difference

// Add these helper functions
fn calculate_luminance(color: (u8, u8, u8)) -> f64 {
    let srgb = |c: u8| {
        let c = c as f64 / 255.0;
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };

    0.2126 * srgb(color.0) + 0.7152 * srgb(color.1) + 0.0722 * srgb(color.2)
}

fn contrast_ratio(fg: (u8, u8, u8), bg: (u8, u8, u8)) -> f64 {
    let l1 = calculate_luminance(fg);
    let l2 = calculate_luminance(bg);
    let (lighter, darker) = if l1 > l2 { (l1, l2) } else { (l2, l1) };
    (lighter + 0.05) / (darker + 0.05)
}

fn color_difference(c1: (u8, u8, u8), c2: (u8, u8, u8)) -> u8 {
    ((c1.0 as i16 - c2.0 as i16).abs()
        + (c1.1 as i16 - c2.1 as i16).abs()
        + (c1.2 as i16 - c2.2 as i16).abs()) as u8
}

fn is_visible_text(fill_color: Option<(u8, u8, u8)>, bg_color: (u8, u8, u8)) -> bool {
    let fg_color = fill_color.unwrap_or((0, 0, 0)); // Default to black if no color specified

    // Check if it's a cyan-like color (commonly used for masking)
    let is_cyan_like = fg_color.0 < 50 && fg_color.1 < 50 && fg_color.2 > 100;
    if is_cyan_like {
        return false;
    }

    // Check contrast ratio
    let cr = contrast_ratio(fg_color, bg_color);
    if cr < MIN_CONTRAST_RATIO {
        return false;
    }

    // Check absolute color difference
    let cd = color_difference(fg_color, bg_color);
    if cd < MIN_COLOR_DIFF {
        return false;
    }

    true
}

// Modify your text segment processing to use this check
// In your text processing loop:
// let processed_fill_color = if gs.fill_color.len() >= 3 {
//     (
//         (gs.fill_color[0] * 255.0).round() as u8,
//         (gs.fill_color[1] * 255.0).round() as u8,
//         (gs.fill_color[2] * 255.0).round() as u8,
//     )
// } else {
//     (0, 0, 0) // Default to black
// };

// // Only process text if it's visible
// if is_visible_text(Some(processed_fill_color), (255, 255, 255)) {  // Assuming white background
//     text_segments.push(TextSegment {
//         content: current_line.clone().trim().to_string(),
//         font_size: current_font_size,
//         transformed_font_size: current_transformed_font_size,
//         x: current_x,
//         y: current_y,
//         is_bold: current_is_bold,
//         font_name: current_font.clone(),
//         page_num: page_num,
//         cutat: "Tj".to_string(),
//         fill_color: Some(processed_fill_color),
//         stroke_color: None,
//     });
// }

#[derive(Clone)]
struct LayoutAnalyzer {
    last_y: f64,
    y_gap_threshold: f64,
    page_height: f64,
}

impl LayoutAnalyzer {
    fn new(page_height: f64) -> Self {
        Self {
            last_y: f64::MAX,
            y_gap_threshold: 0.0,
            page_height,
        }
    }

    fn update_threshold(&mut self, font_size: f64) {
        // Dynamic gap threshold based on font size and page dimensions
        self.y_gap_threshold = (font_size * 1.5).max(self.page_height * 0.03);
    }

    fn is_section_break(&mut self, current_y: f64) -> bool {
        let abs_gap = (current_y - self.last_y).abs();
        let is_large_gap = abs_gap > self.y_gap_threshold * 2.5;
        self.last_y = current_y;
        is_large_gap
    }
}

fn is_valid_heading(text: &str, position: (f64, f64), page_width: f64) -> bool {
    // 1. Positional checks
    let (x_pos, y_pos) = position;
    let x_center = page_width / 2.0;
    let is_centered = (x_pos - x_center).abs() < (page_width * 0.15);

    // 2. Text pattern checks
    let is_short = text.len() <= 60;
    let has_no_punctuation = !text.ends_with(&['.', '!', '?', ';', ',']);
    let has_numbering = text.starts_with(|c: char| c.is_numeric() || c == '#');

    // 3. Line isolation check (implemented later in layout analysis)

    is_centered && is_short && has_no_punctuation && (has_numbering || text.to_uppercase() == text)
}

// Add helper function to extract numbering
fn extract_numbering(text: &str) -> String {
    use regex::Regex;

    let re = Regex::new(r"^((?:\d+\.)+\d+|[IVXLCDM]+\.|(?:Section|Article|Chapter)\s+[A-Z0-9]+)")
        .unwrap();
    re.captures(text)
        .and_then(|caps| caps.get(1))
        .map(|m| m.as_str().trim().to_string())
        .unwrap_or_default()
}

#[derive(Clone, Debug)]
struct PageText {
    segments: Vec<TextSegment>,
    page_num: u32,
    media_box: MediaBox,
}

struct PostProcessor {
    header_threshold: f64,
    footer_threshold: f64,
    continuation_threshold: f64,
    header_footer_detector: Option<HeaderFooterDetector>,
}

impl PostProcessor {
    fn new() -> Self {
        PostProcessor {
            header_threshold: 0.85,  // Top 15% of page
            footer_threshold: 0.15,   // Bottom 15% of page
            continuation_threshold: 0.9, // Top/Bottom 10% of page for continuation detection
            header_footer_detector: None,
        }
    }

    fn with_header_footer_detector(mut self, detector: HeaderFooterDetector) -> Self {
        self.header_footer_detector = Some(detector);
        self
    }

    fn process(&self, pages: Vec<PageText>) -> Vec<ContentOutput> {
        // Sort pages and their segments
        let sorted_pages = self.sort_and_filter(pages);
        
        // Filter out detected headers/footers if detector is available
        let sorted_pages = if let Some(detector) = &self.header_footer_detector {
            let headers_footers = detector.analyze();
            self.filter_headers_footers(sorted_pages, headers_footers)
        } else {
            sorted_pages
        };
        
        let mut document_structure = Vec::new();
        let mut current_chunk = String::new();
        let mut current_headings = Vec::new();
        // Initialize with first page number if available
        let mut current_page_start = sorted_pages.first().map(|p| p.page_num).unwrap_or(1);
        let mut last_y = f64::MAX;
        
        // Track position data for multi-page sections
        let mut section_segments: Vec<TextSegment> = Vec::new();
        let mut section_start_char_pos: Option<usize> = None;
        let mut section_end_char_pos: Option<usize> = None;

        for (idx, page) in sorted_pages.clone().iter().enumerate() {
            for segment in &page.segments {
                // Detect continued text
                let is_continuation = if idx > 0 && !sorted_pages[idx-1].segments.is_empty() {
                    let prev_page = &sorted_pages[idx-1];
                    let prev_segment = prev_page.segments.last().unwrap();
                    
                    // Note: Y coordinates are flipped (0 at top, increases downward)
                    // Check if current segment is at TOP of page (small Y value)
                    let at_top_of_page = segment.y < page.media_box.ury * (1.0 - self.continuation_threshold);
                    // Check if previous segment was at BOTTOM of previous page (large Y value)
                    let prev_at_bottom = prev_segment.y > prev_page.media_box.ury * self.continuation_threshold;
                    
                    at_top_of_page &&
                    prev_at_bottom &&
                    !ends_with_terminal_punctuation(&prev_segment.content) &&
                    !segment.content.starts_with(|c: char| c.is_uppercase())
                } else {
                    false
                };

                if is_continuation {
                    current_chunk.push_str(" ");
                    current_chunk.push_str(&segment.content);
                    section_segments.push(segment.clone());
                    if section_end_char_pos.is_some() || segment.char_end > 0 {
                        section_end_char_pos = Some(segment.char_end);
                    }
                } else {
                    if !current_chunk.is_empty() {
                        // Calculate bounding box from segments on the starting page only
                        let bbox = if !section_segments.is_empty() {
                            // Filter segments to only include those from the starting page
                            let start_page_segments: Vec<&TextSegment> = section_segments.iter()
                                .filter(|s| s.page_num == current_page_start)
                                .collect();
                            
                            if !start_page_segments.is_empty() {
                                let min_x = start_page_segments.iter().map(|s| s.x).fold(f64::INFINITY, f64::min);
                                let max_x = start_page_segments.iter().map(|s| s.x + s.width).fold(f64::NEG_INFINITY, f64::max);
                                let min_y = start_page_segments.iter().map(|s| s.y).fold(f64::INFINITY, f64::min);
                                let max_y = start_page_segments.iter().map(|s| s.y + s.height).fold(f64::NEG_INFINITY, f64::max);
                                
                                Some(BoundingBox {
                                    x: min_x,
                                    y: min_y,
                                    width: max_x - min_x,
                                    height: max_y - min_y,
                                })
                            } else {
                                None
                            }
                        } else {
                            None
                        };
                        
                        document_structure.push(ContentOutput {
                            headings: current_headings.clone(),
                            paragraph: current_chunk.trim().to_string(),
                            page: current_page_start,
                            end_page: if sorted_pages[idx-1].page_num != current_page_start {
                                Some(sorted_pages[idx-1].page_num)
                            } else {
                                None
                            },
                            page_char_start: section_start_char_pos,
                            page_char_end: section_end_char_pos,
                            bbox,
                            page_positions: vec![], // TODO: Implement for PostProcessor
                        });
                        current_chunk.clear();
                        section_segments.clear();
                    }
                    current_chunk = segment.content.clone();
                    current_page_start = page.page_num;
                    section_segments = vec![segment.clone()];
                    section_start_char_pos = Some(segment.char_start);
                    section_end_char_pos = Some(segment.char_end);
                }
                last_y = segment.y;
            }
        }

        // Add final chunk
        if !current_chunk.is_empty() {
            // Calculate bounding box from segments on the starting page only
            let bbox = if !section_segments.is_empty() {
                // Filter segments to only include those from the starting page
                let start_page_segments: Vec<&TextSegment> = section_segments.iter()
                    .filter(|s| s.page_num == current_page_start)
                    .collect();
                
                if !start_page_segments.is_empty() {
                    let min_x = start_page_segments.iter().map(|s| s.x).fold(f64::INFINITY, f64::min);
                    let max_x = start_page_segments.iter().map(|s| s.x + s.width).fold(f64::NEG_INFINITY, f64::max);
                    let min_y = start_page_segments.iter().map(|s| s.y).fold(f64::INFINITY, f64::min);
                    let max_y = start_page_segments.iter().map(|s| s.y + s.height).fold(f64::NEG_INFINITY, f64::max);
                    
                    Some(BoundingBox {
                        x: min_x,
                        y: min_y,
                        width: max_x - min_x,
                        height: max_y - min_y,
                    })
                } else {
                    None
                }
            } else {
                None
            };
            
            let last_page = sorted_pages.last().unwrap().page_num;
            document_structure.push(ContentOutput {
                headings: current_headings,
                paragraph: current_chunk.trim().to_string(),
                page: current_page_start,
                end_page: if last_page != current_page_start {
                    Some(last_page)
                } else {
                    None
                },
                page_char_start: section_start_char_pos,
                page_char_end: section_end_char_pos,
                bbox,
                page_positions: vec![], // TODO: Implement for PostProcessor
            });
        }

        document_structure
    }

    fn sort_and_filter(&self, mut pages: Vec<PageText>) -> Vec<PageText> {
        // Sort pages by page number
        pages.sort_by_key(|p| p.page_num);

        // First pass to detect common headers/footers
        let mut header_counts = HashMap::new();
        let mut footer_counts = HashMap::new();

        for page in &pages {
            if let Some(header) = self.find_header_candidate(page) {
                *header_counts.entry(header).or_insert(0) += 1;
            }
            if let Some(footer) = self.find_footer_candidate(page) {
                *footer_counts.entry(footer).or_insert(0) += 1;
            }
        }

        // Find headers/footers that appear on >50% of pages
        let total_pages = pages.len() as f64;
        let common_headers: HashSet<_> = header_counts.iter()
            .filter(|(_, &count)| count as f64 / total_pages > 0.5)
            .map(|(k, _)| k.clone())
            .collect();
        
        let common_footers: HashSet<_> = footer_counts.iter()
            .filter(|(_, &count)| count as f64 / total_pages > 0.5)
            .map(|(k, _)| k.clone())
            .collect();

        // Filter out headers/footers from all pages
        for page in &mut pages {
            page.segments.retain(|seg| {
                !common_headers.contains(&seg.content) &&
                !common_footers.contains(&seg.content)
            });
        }

        pages
    }

    fn filter_headers_footers(&self, mut pages: Vec<PageText>, headers_footers: HashSet<String>) -> Vec<PageText> {
        for page in &mut pages {
            page.segments.retain(|seg| {
                !headers_footers.contains(&seg.content)
            });
        }
        pages
    }

    fn find_header_candidate(&self, page: &PageText) -> Option<String> {
        page.segments.iter()
            .find(|seg| seg.y > page.media_box.ury * self.header_threshold)
            .map(|seg| seg.content.clone())
    }

    fn find_footer_candidate(&self, page: &PageText) -> Option<String> {
        page.segments.iter()
            .find(|seg| seg.y < page.media_box.lly * self.footer_threshold)
            .map(|seg| seg.content.clone())
    }
}

fn ends_with_terminal_punctuation(s: &str) -> bool {
    s.trim().ends_with(|c: char| c == '.' || c == '!' || c == '?')
}

// Improved font weight detection
#[derive(Debug, Clone, PartialEq)]
enum FontWeight {
    Thin,       // 100
    ExtraLight, // 200
    Light,      // 300
    Regular,    // 400
    Medium,     // 500
    SemiBold,   // 600
    Bold,       // 700
    ExtraBold,  // 800
    Black,      // 900
}

impl FontWeight {
    fn from_font_name(font_name: &str) -> Self {
        let name_lower = font_name.to_lowercase();
        
        // Check for weight keywords in order of specificity
        if name_lower.contains("thin") || name_lower.contains("hairline") {
            FontWeight::Thin
        } else if name_lower.contains("extralight") || name_lower.contains("ultralight") {
            FontWeight::ExtraLight
        } else if name_lower.contains("light") {
            FontWeight::Light
        } else if name_lower.contains("black") || name_lower.contains("heavy") {
            FontWeight::Black
        } else if name_lower.contains("extrabold") || name_lower.contains("ultrabold") {
            FontWeight::ExtraBold
        } else if name_lower.contains("bold") || name_lower.contains("demi") {
            FontWeight::Bold
        } else if name_lower.contains("semibold") || name_lower.contains("demibold") {
            FontWeight::SemiBold
        } else if name_lower.contains("medium") {
            FontWeight::Medium
        } else if name_lower.contains("regular") || name_lower.contains("normal") || name_lower.contains("book") {
            FontWeight::Regular
        } else {
            // Default to regular if no weight indicator found
            FontWeight::Regular
        }
    }
    
    fn to_numeric(&self) -> u16 {
        match self {
            FontWeight::Thin => 100,
            FontWeight::ExtraLight => 200,
            FontWeight::Light => 300,
            FontWeight::Regular => 400,
            FontWeight::Medium => 500,
            FontWeight::SemiBold => 600,
            FontWeight::Bold => 700,
            FontWeight::ExtraBold => 800,
            FontWeight::Black => 900,
        }
    }
    
    fn is_bold(&self) -> bool {
        self.to_numeric() >= 600
    }
}

// Font analysis structures
#[derive(Debug, Clone)]
struct FontInfo {
    name: String,
    family: String,
    weight: FontWeight,
    is_italic: bool,
}

impl FontInfo {
    fn from_font_name(font_name: &str) -> Self {
        let weight = FontWeight::from_font_name(font_name);
        let is_italic = font_name.to_lowercase().contains("italic") || 
                       font_name.to_lowercase().contains("oblique");
        
        // Try to extract font family by removing weight and style indicators
        let family = Self::extract_font_family(font_name);
        
        FontInfo {
            name: font_name.to_string(),
            family,
            weight,
            is_italic,
        }
    }
    
    fn extract_font_family(font_name: &str) -> String {
        // Remove common weight and style suffixes
        let suffixes = [
            "-Bold", "-Regular", "-Light", "-Medium", "-Heavy", "-Black",
            "-Italic", "-Oblique", "-Roman", "-Book", "-Demi", "-Semi",
            "Bold", "Regular", "Light", "Medium", "Heavy", "Black",
            "Italic", "Oblique", "Roman", "Book", "Demi", "Semi"
        ];
        
        let mut family = font_name.to_string();
        for suffix in &suffixes {
            if family.ends_with(suffix) {
                family = family[..family.len() - suffix.len()].to_string();
                family = family.trim_end_matches('-').to_string();
            }
        }
        
        family
    }
}

// Improved header/footer detection structures
#[derive(Debug, Clone)]
struct HeaderFooterPattern {
    regex: Regex,
    pattern_type: HeaderFooterType,
    confidence: f64,
}

#[derive(Debug, Clone, PartialEq)]
enum HeaderFooterType {
    PageNumber,
    Date,
    Title,
    Copyright,
    ChapterTitle,
    Other,
}

struct HeaderFooterDetector {
    patterns: Vec<HeaderFooterPattern>,
    occurrence_map: HashMap<String, Vec<PageOccurrence>>,
    page_count: usize,
}

#[derive(Debug, Clone)]
struct PageOccurrence {
    page_num: u32,
    position: f64,
    font_size: f64,
    is_top: bool,
}

impl HeaderFooterDetector {
    fn new(page_count: usize) -> Self {
        let patterns = vec![
            HeaderFooterPattern {
                regex: Regex::new(r"^(?:Page\s+)?(\d+)(?:\s+of\s+\d+)?$").unwrap(),
                pattern_type: HeaderFooterType::PageNumber,
                confidence: 0.9,
            },
            HeaderFooterPattern {
                regex: Regex::new(r"^-\s*\d+\s*-$").unwrap(),
                pattern_type: HeaderFooterType::PageNumber,
                confidence: 0.9,
            },
            HeaderFooterPattern {
                regex: Regex::new(r"^\[\d+\]$").unwrap(),
                pattern_type: HeaderFooterType::PageNumber,
                confidence: 0.9,
            },
            HeaderFooterPattern {
                regex: Regex::new(r"^\d{1,2}[/-]\d{1,2}[/-]\d{2,4}$").unwrap(),
                pattern_type: HeaderFooterType::Date,
                confidence: 0.8,
            },
            HeaderFooterPattern {
                regex: Regex::new(r"^(?:Chapter|Section|Part)\s+\d+").unwrap(),
                pattern_type: HeaderFooterType::ChapterTitle,
                confidence: 0.85,
            },
            HeaderFooterPattern {
                regex: Regex::new(r"(?i)^©|copyright\s+\d{4}").unwrap(),
                pattern_type: HeaderFooterType::Copyright,
                confidence: 0.9,
            },
        ];

        HeaderFooterDetector {
            patterns,
            occurrence_map: HashMap::new(),
            page_count,
        }
    }

    fn add_occurrence(&mut self, text: &str, page_num: u32, position: f64, font_size: f64, page_height: f64) {
        let is_top = position > page_height * 0.85;
        let occurrence = PageOccurrence {
            page_num,
            position,
            font_size,
            is_top,
        };

        self.occurrence_map
            .entry(text.to_string())
            .or_insert_with(Vec::new)
            .push(occurrence);
    }

    fn analyze(&self) -> HashSet<String> {
        let mut headers_footers = HashSet::new();
        let min_occurrence_ratio = 0.5;

        for (text, occurrences) in &self.occurrence_map {
            let occurrence_count = occurrences.len();
            let occurrence_ratio = occurrence_count as f64 / self.page_count as f64;

            if occurrence_ratio >= min_occurrence_ratio {
                // Check if positions are consistent
                let positions: Vec<f64> = occurrences.iter().map(|o| o.position).collect();
                let avg_position = positions.iter().sum::<f64>() / positions.len() as f64;
                let position_variance = positions.iter()
                    .map(|p| (p - avg_position).powi(2))
                    .sum::<f64>() / positions.len() as f64;
                
                // Low variance means consistent positioning
                if position_variance < 100.0 {
                    // Check against patterns for higher confidence
                    let mut pattern_matched = false;
                    for pattern in &self.patterns {
                        if pattern.regex.is_match(text) {
                            pattern_matched = true;
                            break;
                        }
                    }

                    if pattern_matched || occurrence_ratio >= 0.8 {
                        headers_footers.insert(text.clone());
                    }
                }
            }
        }

        headers_footers
    }
}

// Helper function to calculate bounding box for a set of segments
fn calculate_bbox_for_segments(segments: &[TextSegment]) -> BoundingBox {
    let min_x = segments.iter().map(|s| s.x).fold(f64::INFINITY, f64::min);
    let max_x = segments.iter().map(|s| s.x + s.width).fold(f64::NEG_INFINITY, f64::max);
    let min_y = segments.iter().map(|s| s.y).fold(f64::INFINITY, f64::min);
    let max_y = segments.iter().map(|s| s.y + s.height).fold(f64::NEG_INFINITY, f64::max);
    
    BoundingBox {
        x: min_x,
        y: min_y,
        width: max_x - min_x,
        height: max_y - min_y,
    }
}

// Modified ContentOutput struct with position tracking
#[derive(Debug)]
pub struct ContentOutput {
    pub headings: Vec<String>,
    pub paragraph: String,
    pub page: u32,
    pub end_page: Option<u32>,  // None means single page
    // New position tracking fields
    pub page_char_start: Option<usize>,  // Character position from start of first page
    pub page_char_end: Option<usize>,    // Character position end on last page
    pub bbox: Option<BoundingBox>,       // Bounding box for visual highlighting
    // Enhanced multi-page tracking
    pub page_positions: Vec<PagePosition>, // Per-page position info for multi-page content
}

#[derive(Debug, Clone)]
pub struct PagePosition {
    pub page: u32,
    pub char_start: usize,
    pub char_end: usize,
    pub bbox: BoundingBox,
}

#[derive(Debug, Clone)]
pub struct BoundingBox {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

// Updated output_doc function
// pub fn output_doc(
//     doc: &Document,
//     ocr_handler: Option<&OcrHandler>,
// ) -> Result<Vec<ContentOutput>, Box<dyn std::error::Error>> {
//     let pages = doc.get_pages();
//     let empty_resources = &Dictionary::new();

//     // Parallel processing of individual pages
//     let page_texts: Vec<PageText> = pages.par_iter()
//         .map(|(page_num, page_id)| {
//             let page_dict = doc.get_object(*page_id).unwrap().as_dict().unwrap();
//             let resources = get_inherited(doc, page_dict, b"Resources").unwrap_or(empty_resources);
//             let media_box: Vec<f64> = get_inherited(doc, page_dict, b"MediaBox").expect("MediaBox");
//             let media_box = MediaBox {
//                 llx: media_box[0],
//                 lly: media_box[1],
//                 urx: media_box[2],
//                 ury: media_box[3],
//             };

//             let mut page_segments = Vec::new();
//             let mut processor = Processor::new();
//             processor.process_stream(
//                 doc,
//                 ocr_handler,
//                 doc.get_page_content(*page_id).unwrap(),
//                 resources,
//                 &media_box,
//                 page_dict.get(b"Parent").and_then(|p| p.as_reference()).map(|x| x.0).unwrap_or(0),
//                 &mut page_segments,
//             ).unwrap();

//             PageText {
//                 segments: page_segments,
//                 page_num: *page_num,
//                 media_box,
//             }
//         })
//         .collect();

//     // Sequential post-processing
//     let post_processor = PostProcessor::new();
//     let results = post_processor.process(page_texts);
    
//     Ok(results)
// }
