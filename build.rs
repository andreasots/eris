fn main() {
    lalrpop::Configuration::new()
        .emit_rerun_directives(true)
        .force_build(false)
        .process_current_dir()
        .unwrap();
}
