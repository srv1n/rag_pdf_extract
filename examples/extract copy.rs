use pdf_extract::*;

fn main() {
    //let output_kind = "svg";
    let file = "1.pdf";

    // let output_kind = env::args().nth(2).unwrap_or_else(|| "txt".to_owned());
    let shata = parse_pdf(file).unwrap();
    let muta = shata.len();
    for item in shata {
        println!("\n\nHeadings: {:#?}", item.headings);
        println!("\n{}", item.paragraph.replace("\\n", "\n"));
        println!("\nPage: {:#?}", item.page);
    }
    println!("Length of shata: {}", muta);
}
