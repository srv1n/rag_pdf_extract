use std::{collections::VecDeque, fmt::format};

use lopdf::{Document, Object, ObjectId};
use std::str;

use crate::{ContentOutput, OutputError};
#[derive(Debug)]
/// Errors that may occur while loading a PDF
pub enum LoadError {
    /// An Lopdf Error
    LopdfError(lopdf::Error),
    /// The reference `ObjectId` did not point to any values
    NoSuchReference(ObjectId),
    /// An element that was expected to be a reference was not a reference
    NotAReference,
}

/// Errors That may occur while setting values in a form
#[derive(Debug)]
pub enum ValueError {
    /// The method used to set the state is incompatible with the type of the field
    TypeMismatch,
    /// One or more selected values are not valid choices
    InvalidSelection,
    /// Multiple values were selected when only one was allowed
    TooManySelected,
    /// Readonly field cannot be edited
    Readonly,
}

#[derive(Debug)]
pub enum FieldType {
    Button,
    Radio,
    CheckBox,
    ListBox,
    ComboBox,
    Text,
    Unknown,
}

#[derive(Debug)]
pub enum FieldState {
    Button,
    Radio {
        selected: String,
        options: Vec<String>,
        readonly: bool,
        required: bool,
    },
    CheckBox {
        is_checked: bool,
        readonly: bool,
        required: bool,
    },
    ListBox {
        selected: Vec<String>,
        options: Vec<String>,
        multiselect: bool,
        readonly: bool,
        required: bool,
    },
    ComboBox {
        selected: Vec<String>,
        options: Vec<String>,
        editable: bool,
        readonly: bool,
        required: bool,
    },
    Text {
        text: String,
        readonly: bool,
        required: bool,
    },
    Unknown,
}

pub struct Form {
    doc: Document,
    form_ids: Vec<ObjectId>,
}

trait PdfObjectDeref {
    fn deref<'a>(&self, doc: &'a Document) -> Result<&'a Object, LoadError>;
}

impl PdfObjectDeref for Object {
    fn deref<'a>(&self, doc: &'a Document) -> Result<&'a Object, LoadError> {
        match *self {
            Object::Reference(oid) => doc.objects.get(&oid).ok_or(LoadError::NoSuchReference(oid)),
            _ => Err(LoadError::NotAReference),
        }
    }
}

impl Form {
    pub fn load_doc(mut doc: Document) -> Result<Self, lopdf::Error> {
        // println!("camer here1");
        // doc.decompress();
        let mut form_ids = Vec::new();
        let mut queue = VecDeque::new();

        let acroform = doc
            .objects
            .get_mut(
                &doc.trailer
                    .get(b"Root")?
                    .deref(&doc)
                    .unwrap()
                    .as_dict()?
                    .get(b"AcroForm")?
                    .as_reference()?,
            )
            .ok_or(LoadError::NotAReference)
            .unwrap()
            .as_dict_mut()?;

        let fields_list = acroform.get(b"Fields")?.as_array()?;
        queue.append(&mut VecDeque::from(fields_list.clone()));
        // println!("camer here");
        while let Some(objref) = queue.pop_front() {
            let obj = doc.get_object(objref.as_reference()?)?;
            if let Object::Dictionary(ref dict) = *obj {
                if dict.get(b"FT").is_ok() {
                    form_ids.push(objref.as_reference()?);
                }
                if let Ok(&Object::Array(ref kids)) = dict.get(b"Kids") {
                    queue.append(&mut VecDeque::from(kids.clone()));
                }
            }
        }

        Ok(Form { doc, form_ids })
    }

    pub fn len(&self) -> usize {
        self.form_ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn get_type(&self, n: usize) -> FieldType {
        let field = self
            .doc
            .get_object(self.form_ids[n])
            .unwrap()
            .as_dict()
            .unwrap();
        let type_str = field.get(b"FT").unwrap().as_name().unwrap();
        match type_str {
            b"Btn" => {
                let flags = get_field_flags(field);
                if flags & 32768 != 0 || flags & 65536 != 0 {
                    FieldType::Radio
                } else if flags & 65536 != 0 {
                    FieldType::Button
                } else {
                    FieldType::CheckBox
                }
            }
            b"Ch" => {
                let flags = get_field_flags(field);
                if flags & 131072 != 0 {
                    FieldType::ComboBox
                } else {
                    FieldType::ListBox
                }
            }
            b"Tx" => FieldType::Text,
            _ => FieldType::Unknown,
        }
    }

    pub fn get_name(&self, n: usize) -> Option<String> {
        let field = self
            .doc
            .get_object(self.form_ids[n])
            .unwrap()
            .as_dict()
            .unwrap();
        field.get(b"T").ok().and_then(|obj| {
            if let Object::String(data, _) = obj {
                String::from_utf8(data.clone()).ok()
            } else {
                None
            }
        })
    }

    pub fn get_state(&self, n: usize) -> FieldState {
        let field = self
            .doc
            .get_object(self.form_ids[n])
            .unwrap()
            .as_dict()
            .unwrap();
        match self.get_type(n) {
            FieldType::Button => FieldState::Button,
            FieldType::Radio => FieldState::Radio {
                selected: get_field_value(field).unwrap_or_default(),
                options: self.get_possibilities(self.form_ids[n]),
                readonly: is_read_only(field),
                required: is_required(field),
            },
            FieldType::CheckBox => FieldState::CheckBox {
                is_checked: get_field_value(field).unwrap_or_default() == "Yes",
                readonly: is_read_only(field),
                required: is_required(field),
            },
            FieldType::ListBox => FieldState::ListBox {
                selected: get_field_values(field),
                options: get_field_options(field),
                multiselect: get_field_flags(field) & 2097152 != 0,
                readonly: is_read_only(field),
                required: is_required(field),
            },
            FieldType::ComboBox => FieldState::ComboBox {
                selected: get_field_values(field),
                options: get_field_options(field),
                editable: get_field_flags(field) & 1048576 != 0,
                readonly: is_read_only(field),
                required: is_required(field),
            },
            FieldType::Text => FieldState::Text {
                text: get_field_value(field).unwrap_or_default(),
                readonly: is_read_only(field),
                required: is_required(field),
            },
            FieldType::Unknown => FieldState::Unknown,
        }
    }

    fn get_possibilities(&self, id: ObjectId) -> Vec<String> {
        let field = self.doc.get_object(id).unwrap().as_dict().unwrap();
        get_field_options(field)
    }
}

fn get_field_flags(field: &lopdf::Dictionary) -> i64 {
    field.get(b"Ff").and_then(|f| f.as_i64()).unwrap_or(0)
}

fn is_read_only(field: &lopdf::Dictionary) -> bool {
    get_field_flags(field) & 1 != 0
}

fn is_required(field: &lopdf::Dictionary) -> bool {
    get_field_flags(field) & 2 != 0
}

fn get_field_value(field: &lopdf::Dictionary) -> Option<String> {
    match field
        .get(b"V")
        .and_then(|v| Ok(v.as_string().ok()))
        .map(|s| s.unwrap().into_owned())
    {
        Ok(s) => Some(s),
        Err(e) => None,
    }
}

fn get_field_values(field: &lopdf::Dictionary) -> Vec<String> {
    field
        .get(b"V")
        .map(|v| match v {
            Object::String(s, _) => vec![str::from_utf8(s).unwrap().to_owned()],
            Object::Array(arr) => arr
                .iter()
                .filter_map(|obj| {
                    if let Object::String(s, _) = obj {
                        Some(str::from_utf8(s).unwrap().to_owned())
                    } else {
                        None
                    }
                })
                .collect(),
            _ => Vec::new(),
        })
        .unwrap_or_default()
}

fn get_field_options(field: &lopdf::Dictionary) -> Vec<String> {
    field
        .get(b"Opt")
        .and_then(|opt| opt.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|obj| match obj {
                    Object::String(s, _) => Some(str::from_utf8(s).unwrap().to_owned()),
                    Object::Array(inner_arr) if inner_arr.len() > 1 => {
                        if let Object::String(s, _) = &inner_arr[1] {
                            Some(str::from_utf8(s).unwrap().to_owned())
                        } else {
                            None
                        }
                    }
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn form_fields(
    doc: &Document,
    document_structure: &mut Vec<ContentOutput>,
) -> Result<(), OutputError> {
    let form = Form::load_doc(doc.clone())?;
    println!("form len: {}", form.len());
    let mut text = String::new();
    for i in 0..form.len() {
        // let page = form.doc.;
        let field_type = form.get_type(i);
        let field_name = form.get_name(i);
        let field_state = form.get_state(i);
        let field_value = match field_state {
            FieldState::Button => "Button".to_string(),
            FieldState::Radio {
                selected,
                options,
                readonly,
                required,
            } => {
                format!(
                    "Radio: Selected: {}, Options: [{}]",
                    selected,
                    options.join(", "),
                )
            }
            FieldState::CheckBox {
                is_checked,
                readonly,
                required,
            } => {
                format!("CheckBox: Checked: {}", is_checked)
            }
            FieldState::ListBox {
                selected,
                options,
                multiselect,
                readonly,
                required,
            } => {
                format!(
                    "ListBox: Selected: [{}], Options: [{}], Multiselect: {}",
                    selected.join(", "),
                    options.join(", "),
                    multiselect,
                    // readonly,
                    // required
                )
            }
            FieldState::ComboBox {
                selected,
                options,
                editable,
                readonly,
                required,
            } => {
                format!(
                    "ComboBox: Selected: [{}], Options: [{}], Editable: {}",
                    selected.join(", "),
                    options.join(", "),
                    editable,
                    // readonly,
                    // required
                )
            }
            FieldState::Text {
                text,
                readonly,
                required,
            } => {
                format!("{}", text)
            }
            FieldState::Unknown => "Unknown".to_string(),
        };

        text.push_str(&format!(
            "{}: {}",
            field_name.unwrap_or("".to_string()),
            field_value
        ));
    }
    document_structure.push(ContentOutput {
        headings: vec![],
        paragraph: text,
        page: 0,
    });
    Ok(())
}
