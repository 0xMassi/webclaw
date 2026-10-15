fn main() {
    let path = std::env::args().nth(1).expect("usage: pdf_text FILE");
    let bytes = std::fs::read(&path).expect("readable file");
    match webclaw_pdf::extract_pdf(&bytes, webclaw_pdf::PdfMode::Fast) {
        Ok(read) => println!("{}\n{}", read.page_count, read.text),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}
