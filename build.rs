fn main() {
    #[cfg(feature = "ocr")]
    {
        let eng_traineddata = reqwest::blocking::get(
            "https://github.com/tesseract-ocr/tessdata_best/raw/refs/tags/4.1.0/eng.traineddata",
        )
        .expect("failed to download the english model for Tesseract")
        .bytes()
        .expect("failed to download the english model for Tesseract")
        .to_vec();
        std::fs::write(
            std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("eng.traineddata"),
            eng_traineddata,
        )
        .expect("failed to save the model");
    }

    lalrpop::Configuration::new()
        .emit_rerun_directives(true)
        .force_build(false)
        .process_current_dir()
        .unwrap();
}
