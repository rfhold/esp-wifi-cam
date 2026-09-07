fn main() {
    let arguments: Vec<_> = std::env::args_os().collect();
    let result = if arguments
        .get(1)
        .is_some_and(|value| value == "ignore-record")
    {
        release_tool::parse_ignore_record_arguments(arguments)
            .and_then(|arguments| release_tool::generate_ignore_record(&arguments))
    } else {
        release_tool::parse_arguments(arguments)
            .and_then(|arguments| release_tool::generate_manifest(&arguments))
    };
    match result {
        Ok(path) => println!("wrote {}", path.display()),
        Err(error) => {
            eprintln!("release-tool: {error}");
            std::process::exit(1);
        }
    }
}
