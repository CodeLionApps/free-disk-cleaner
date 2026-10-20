use std::fmt::Write;

fn main() {
    // Rule rationales/hints are translation keys owned by core; expose them to Slint as @tr() literals.
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let mut src = String::from(
        "export global RuleTexts {\n    public pure function text(key: string) -> string {\n",
    );
    for k in fdc_core::rules::all_translation_keys() {
        writeln!(src, "        if key == \"{k}\" {{ return @tr(\"{k}\"); }}").unwrap();
    }
    src.push_str("        return \"\";\n    }\n}\n");
    std::fs::write(out.join("rule_texts.slint"), src).unwrap();

    let config = slint_build::CompilerConfiguration::new()
        .with_style("cupertino".into())
        // Element introspection for headless UI tests and the dev MCP server; never in release.
        .with_debug_info(std::env::var("PROFILE").as_deref() != Ok("release"))
        .with_include_paths(vec![out])
        .with_default_translation_context(slint_build::DefaultTranslationContext::None)
        .with_bundled_translations(concat!(env!("CARGO_MANIFEST_DIR"), "/translations"));
    slint_build::compile_with_config("ui/app.slint", config).expect("slint build failed");
}
