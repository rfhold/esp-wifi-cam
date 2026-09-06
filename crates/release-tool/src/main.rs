fn main() {
    match release_tool::parse_arguments(std::env::args_os())
        .and_then(|arguments| release_tool::generate_manifest(&arguments))
    {
        Ok(path) => println!("wrote {}", path.display()),
        Err(error) => {
            eprintln!("release-tool: {error}");
            std::process::exit(1);
        }
    }
}
