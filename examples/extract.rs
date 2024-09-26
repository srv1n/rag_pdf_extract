use pdf_extract::*;

fn main() {
    //let output_kind = "svg";
    let file = "2.pdf";

    // let output_kind = env::args().nth(2).unwrap_or_else(|| "txt".to_owned());
    let docs = parse_pdf(file).unwrap();
    let muta = docs.len();
    for item in docs {
        println!("\n\nHeadings: {:#?}", item.headings);
        println!("\n{}", item.paragraph.replace("\\n", "\n"));
        println!("\nPage: {:#?}", item.page);
        println!("{}", item.paragraph.len());
    }
    // println!("Length of shata: {}", muta);
}
